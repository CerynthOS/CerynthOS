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
use clap::Parser;
use libbpf_rs::OpenObject;
use scx_utils::UserExitInfo;
use scx_utils::libbpf_clap_opts::LibbpfOpts;


mod profile;
use profile::Profile;

mod policy;
mod error;
mod status;

const MAX_BATCH: usize = 64;

#[derive(Parser, Debug)]
#[command(name = "cerynth-scx", about = "CerynthOS sched_ext scheduler")]
struct Cli {
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

    fn dispatch_tasks(&mut self){
        let mut tasks = Vec::new();
        loop {
            if tasks.len()>=MAX_BATCH {
                break;
            }
            match self.bpf.dequeue_task() {
                Ok(Some(task)) => tasks.push(task),
                Ok(None) => break,
                Err(errno) =>{
                    eprintln!("cerynth-scx: {}", error::SchedError::Dequeue(errno));
                    break;
                }
            }
        }
        for task in policy::order_tasks(self.profile, tasks) {
            let dispatched_task = DispatchedTask::new(&task);
            if let Err(e) = self.bpf.dispatch_task(&dispatched_task) {
                eprintln!("cerynth-scx: {}",error::SchedError::Dispatch(e));
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
    let slice_ns: u64 = cli.profile.slice_ns();

    println!(
        "cerynth-scx: starting (profile = {:?}, time slice = {} ns)",
        cli.profile, slice_ns
    );

    let mut open_object = MaybeUninit::uninit();
    loop {
        let mut sched = Scheduler::init(&mut open_object, slice_ns, cli.profile)?;
        if let Err(e) = status::SchedulerStatus::running(cli.profile).write() {
            eprintln!("cerynth-scx: failed to write status file: {e}");
        }
        if !sched.run()?.should_restart() {
            break;
        }
    }
    if let Err(e) = status::SchedulerStatus::stopped().write() {
        eprintln!("cerynth-scx: failed to write status file: {e}");
    }
    Ok(())
}