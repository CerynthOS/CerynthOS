//! Filesystem reporting is isolated from the scheduling loop. A blocked
//! reporter must never prevent the parent from dispatching its lock owners.
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixDatagram;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::profile::Profile;
use crate::status::{SchedulerStats, SchedulerStatus};
use serde::{Deserialize, Serialize};

const MAX_SNAPSHOT: usize = 8192;

#[derive(Deserialize)]
struct Snapshot {
    status: SchedulerStatus,
    stats: SchedulerStats,
}

#[derive(Serialize)]
struct SnapshotRef<'a> {
    status: SchedulerStatus,
    stats: &'a SchedulerStats,
}

pub struct Reporter {
    sender: UnixDatagram,
    child: Child,
}

fn send_snapshot(sender: &UnixDatagram, bytes: &[u8]) -> io::Result<bool> {
    if bytes.len() > MAX_SNAPSHOT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized scheduler snapshot",
        ));
    }
    match sender.send(bytes) {
        Ok(size) if size == bytes.len() => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "partial scheduler datagram",
        )),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

impl Reporter {
    /// Called before BPF attachment: executable lookup, spawn and fd setup
    /// may block and therefore do not belong in the active scheduler loop.
    pub fn start() -> io::Result<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--status-reporter")
            .arg("--reporter-parent")
            .arg(std::process::id().to_string());
        Self::with_command(command)
    }

    fn with_command(mut command: Command) -> io::Result<Self> {
        let (sender, receiver) = UnixDatagram::pair()?;
        sender.set_nonblocking(true)?;
        let input: OwnedFd = receiver.into();
        let child = command
            .stdin(Stdio::from(input))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self { sender, child })
    }

    /// No filesystem operations, waits, or logging. Backpressure drops the
    /// observation, not runnable tasks. Freshness/progress monitors fail closed.
    pub fn publish(
        &self,
        profile: Profile,
        started_at_ms: u64,
        stats: &SchedulerStats,
    ) -> io::Result<bool> {
        let bytes = serde_json::to_vec(&SnapshotRef {
            status: SchedulerStatus::running(profile, started_at_ms),
            stats,
        })?;
        send_snapshot(&self.sender, &bytes)
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        // Scheduler's BPF field is dropped first, so kernel scheduling is
        // restored before waiting for a reporter blocked on filesystem I/O.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn run(parent: u32) -> io::Result<()> {
    // The child execs this mode before any BPF/allocator initialization.
    // Kernel parent-death signaling also covers parent SIGKILL/panic.
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::getppid() as u32 != parent {
            return Ok(());
        }
    }
    let receiver = unsafe { UnixDatagram::from_raw_fd(0) };
    receiver.set_nonblocking(true)?;
    let mut buffer = [0u8; MAX_SNAPSHOT];
    while unsafe { libc::getppid() as u32 } == parent {
        // Drain queued frames; publish only the latest parent observation.
        let mut latest = None;
        loop {
            match receiver.recv(&mut buffer) {
                Ok(size) => latest = Some(serde_json::from_slice::<Snapshot>(&buffer[..size])?),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if let Some(mut snapshot) = latest {
            if snapshot.status.pid != parent || snapshot.stats.pid != parent {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "snapshot PID mismatch",
                ));
            }
            // Preserve parent heartbeat/progress timestamps; reporter activity
            // must never manufacture scheduler liveness.
            snapshot.status.sched_ext_state =
                std::fs::read_to_string("/sys/kernel/sched_ext/state")?
                    .trim()
                    .to_string();
            snapshot.status.healthy = snapshot.status.sched_ext_state == "enabled";
            snapshot.status.write()?;
            snapshot.stats.write()?;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn stalled_reporter_cannot_block_snapshot_publication() {
        let (sender, _stalled_receiver) = UnixDatagram::pair().unwrap();
        sender.set_nonblocking(true).unwrap();
        let bytes = [42; 1024];
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut dropped = 0;
        for _ in 0..10000 {
            if !send_snapshot(&sender, &bytes).unwrap() {
                dropped += 1;
            }
            assert!(
                Instant::now() < deadline,
                "snapshot sender blocked behind reporter"
            );
        }
        assert!(dropped > 0, "test must fill the socket queue");
    }

    #[test]
    fn blocked_reporter_process_is_killed_and_reaped() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let reporter = Reporter::with_command(command).unwrap();
        let pid = reporter.child.id();
        let stats = SchedulerStats::default();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut dropped = 0;
        for _ in 0..1000 {
            if !reporter.publish(Profile::Balanced, 1000, &stats).unwrap() {
                dropped += 1;
            }
            assert!(Instant::now() < deadline);
        }
        assert!(dropped > 0);
        drop(reporter);
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn reporter_snapshot_preserves_scheduler_identity_and_heartbeat() {
        let stats = SchedulerStats {
            pid: 123,
            heartbeat: 10,
            progress_sequence: 55,
            ..Default::default()
        };
        let mut status = SchedulerStatus::running(Profile::Balanced, 1000);
        status.pid = 123;
        status.heartbeat = 10;
        let bytes = serde_json::to_vec(&SnapshotRef {
            status,
            stats: &stats,
        })
        .unwrap();
        let decoded: Snapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.status.pid, 123);
        assert_eq!(decoded.status.heartbeat, 10);
        assert_eq!(decoded.stats.progress_sequence, 55);
    }

    #[test]
    fn closed_reporter_is_a_failed_observation_not_a_blocking_wait() {
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        sender.set_nonblocking(true).unwrap();
        drop(receiver);
        assert!(send_snapshot(&sender, b"snapshot").is_err());
    }
}
