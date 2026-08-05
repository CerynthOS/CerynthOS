// cerynth-scx: CerynthOS's first sched_ext scheduler.
//
// Built on scx_rustland_core (the same framework scx_rustland and
// scx_rlfifo use). The BPF plumbing (main.bpf.c, bpf.rs, bpf_intf.rs,
// bpf_skel.rs) is shared, generic infrastructure. This file is the
// actual scheduling *policy*.

mod bpf_skel;
pub use bpf_skel::*;
pub mod bpf_intf;

#[rustfmt::skip]
mod bpf;
use std::mem::MaybeUninit;

use anyhow::Result;
use bpf::*;
use libbpf_rs::OpenObject;
use scx_utils::libbpf_clap_opts::LibbpfOpts;
use scx_utils::UserExitInfo;
use clap::Parser;

#[derive(Parser,Debug)]
#[command(name = "cerynth-scx", about = "CerynthOS sched_ext scheduler")]
struct Cli{
    /// Time slice, in microseconds, given to each task before it's re-enqueued.
    #[arg(long, default_value_t = 5000)]
    slice_us: u64,
}

struct Scheduler<'a> {
    bpf: BpfScheduler<'a>,
}

impl<'a> Scheduler<'a> {
    fn init(open_object: &'a mut MaybeUninit<OpenObject>, slice_ns: u64) -> Result<Self> {
        let open_opts = LibbpfOpts::default();
        let bpf = BpfScheduler::init(
            open_object,
            open_opts.clone().into_bpf_open_opts(),
            0,
            false,
            false,
            true,
            slice_ns,
            "cerynth_scx",
        )?;
        Ok(Self { bpf })
    }

    fn dispatch_tasks(&mut self) {
        while let Ok(Some(task)) = self.bpf.dequeue_task() {
            let dispatched_task = DispatchedTask::new(&task);
            self.bpf.dispatch_task(&dispatched_task).unwrap();
        }
        self.bpf.notify_complete(0);
    }

    fn run(&mut self) -> Result<UserExitInfo> {
        while !self.bpf.exited() {
            self.dispatch_tasks();
        }
        println!("cerynth-scx: shutting down, handing control back to the kernel...");
        self.bpf.shutdown_and_report()
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let slice_ns = cli.slice_us * 1000;

    println!(
        "cerynth-scx: starting (time slice = {} us / {} ns)",
        cli.slice_us, slice_ns
    );

    let mut open_object = MaybeUninit::uninit();
    loop {
        let mut sched = Scheduler::init(&mut open_object, slice_ns)?;
        if !sched.run()?.should_restart() {
            break;
        }
    }
    Ok(())
}
