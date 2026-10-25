// SPDX-License-Identifier: GPL-2.0
fn main() {
    // Compile the reviewed, repository-owned sources. RustLandBuilder copies
    // dependency templates over main.bpf.c and src/bpf.rs, discarding fixes.
    scx_cargo::BpfBuilder::new()
        .unwrap()
        .enable_intf("intf.h", "bpf_intf.rs")
        .enable_skel("main.bpf.c", "bpf")
        .build()
        .unwrap();
    println!("cargo:rerun-if-changed=dispatch.h");
}
