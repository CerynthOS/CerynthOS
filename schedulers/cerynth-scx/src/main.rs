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

#[derive(clap::ValueEnum, Clone, Debug)]
enum Profile{
    Balanced,
    Interactive,
}

#[derive(Parser,Debug)]
#[command(name = "cerynth-scx", about = "CerynthOS sched_ext scheduler")]
struct Cli{
    /// Which schedulinh profile to run.
    #[arg(long, value_enum, default_value_t = Profile::Balanced)]
    profile: Profile,
}

struct Scheduler<'a> {
    bpf: BpfScheduler<'a>,
    profile: Profile,
}

impl<'a> Scheduler<'a> {
    fn init(
        open_object: &'a mut MaybeUninit<OpenObject>,
        slice_ns: u64,
        profile: Profile,
    ) -> Result<Self> {
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
        Ok(Self { bpf, profile })
    }

    fn dispatch_tasks(&mut self) {
        match self.profile {
            Profile::Balanced => {
                // FIFO: dispatch tasks in whatever order the kernel handed them to us.
                while let Ok(Some(task)) = self.bpf.dequeue_task() {
                    let dispatched_task = DispatchedTask::new(&task);
                    self.bpf.dispatch_task(&dispatched_task).unwrap();
                }
            }
            Profile::Interactive => {
                // Collect the whole waiting batch first...
                let mut tasks = Vec::new();
                while let Ok(Some(task)) = self.bpf.dequeue_task() {
                    tasks.push(task);
                }
                // ...then run short-burst tasks (low exec_runtime = they just
                // woke up and haven't used much CPU) before long-running ones,
                // so interactive work stays snappy under load.
                tasks.sort_by_key(|task| task.exec_runtime);
                for task in tasks {
                    let dispatched_task = DispatchedTask::new(&task);
                    self.bpf.dispatch_task(&dispatched_task).unwrap();
                }
            }
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
    let slice_ns: u64 = match cli.profile {
        Profile::Balanced => 5_000_000,    // 5ms
        Profile::Interactive => 2_000_000, // 2ms - shorter, more responsive
    };

    println!(
        "cerynth-scx: starting (profile = {:?}, time slice = {} ns)",
        cli.profile, slice_ns
    );

    let mut open_object = MaybeUninit::uninit();
    loop {
        let mut sched = Scheduler::init(&mut open_object, slice_ns, cli.profile.clone())?;
        if !sched.run()?.should_restart() {
            break;
        }
    }
    Ok(())
}
