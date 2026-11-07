//! Process-level hardening: what keeps a dying scheduler from holding the
//! machine up.
//!
//! The kernel only unloads the BPF half once this *process* is completely
//! gone. Anything that delays the process's death therefore delays the
//! fallback to the kernel's default scheduler, and while we're dying nobody
//! is dispatching tasks.

// Three thin libc calls with no safe wrapper in our dependencies.
#![allow(unsafe_code)]

use std::fs;
use std::io;

/// Don't write a core dump if this process crashes.
///
/// A core dump is produced by a helper program the kernel starts while the
/// crashed process is still half-alive. That helper needs CPU time, which it
/// would have to get from the very scheduler that just crashed. The machine
/// would sit stalled until the kernel's 5s watchdog ejects us. A crash
/// should end the process immediately instead; the panic message on stderr
/// is the diagnostic.
pub fn disable_core_dumps() -> io::Result<()> {
    // SAFETY: PR_SET_DUMPABLE takes one integer argument and only changes a
    // flag on this process.
    let rc = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Move every thread of this process except the calling one out of
/// `sched_ext`, into the kernel's real-time class. Returns how many threads
/// were moved.
///
/// The calling thread runs the dispatch loop and is placed on a CPU by the
/// BPF half directly. Any *other* thread of ours (the signal-handling thread
/// the `ctrlc` crate starts) is an ordinary task: to run, it has to be
/// dispatched by the dispatch loop. When the process is killed, that helper
/// thread must run one last time to exit, but the dispatch loop is already
/// dead. With every CPU busy it waits forever, the process can't finish
/// dying, the BPF half stays loaded, and the machine stalls until the
/// watchdog fires 5-9s later. `scripts/scx/stress-lifecycle.sh` saw exactly
/// that in 84 of 99 SIGKILLs.
///
/// A real-time thread is scheduled by the kernel itself, never by us, so it
/// can always exit. These threads sleep until a signal arrives, so their
/// priority costs nothing.
pub fn shield_helper_threads() -> io::Result<usize> {
    // SAFETY: gettid has no arguments and cannot fail.
    let own_tid = unsafe { libc::gettid() };
    let mut shielded = 0;

    for entry in fs::read_dir("/proc/self/task")? {
        let name = entry?.file_name();
        let Some(tid) = name.to_str().and_then(|s| s.parse::<libc::pid_t>().ok()) else {
            continue;
        };
        if tid == own_tid {
            continue;
        }

        // SAFETY: sched_param is plain integers; all-zero is a valid value.
        let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
        param.sched_priority = 1;
        // SAFETY: `param` is a valid sched_param that outlives the call.
        let rc = unsafe { libc::sched_setscheduler(tid, libc::SCHED_FIFO, &raw const param) };
        if rc == 0 {
            shielded += 1;
            continue;
        }
        let err = io::Error::last_os_error();
        // The thread exited between listing it and changing it: nothing
        // left to shield.
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(err);
        }
    }

    Ok(shielded)
}
