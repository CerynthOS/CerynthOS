use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

#[allow(clippy::unnecessary_debug_formatting)] // Rust path attributes require escaped string literals.
fn main() {
    let package = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let baseline = package.parent().unwrap().join("cerynth-scx");
    let source = fs::read_to_string(baseline.join("src/main.rs")).unwrap();
    // Deliberately pin the exact baseline main, so an upstream policy change
    // requires an explicit experimental rebase and another qualification.
    let expected = include_str!("baseline-main.sha256").trim();
    assert_eq!(format!("{:x}", Sha256::digest(source.as_bytes())), expected);
    let mut generated = source;
    for name in [
        "bpf_skel",
        "bpf_intf",
        "bpf",
        "profile",
        "error",
        "initialization",
        "process",
        "reporter",
        "stats",
        "status",
    ] {
        let visibility = if name == "bpf_intf" { "pub " } else { "" };
        let needle = format!("{visibility}mod {name};");
        assert_eq!(generated.matches(&needle).count(), 1);
        // Preserve the baseline reporter byte-for-byte. These existing style
        // warnings are outside the experimental policy; its own code is strict.
        let lint_scope = if name == "reporter" {
            "#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap, clippy::needless_continue, clippy::similar_names)]"
        } else {
            ""
        };
        generated = generated.replace(
            &needle,
            &format!(
                "{lint_scope}\n#[path = {:?}] {visibility}mod {name};",
                baseline.join(format!("src/{name}.rs"))
            ),
        );
    }
    generated = generated.replace(
        "mod policy;",
        &format!("#[path = {:?}] mod policy;", package.join("src/policy.rs")),
    );
    let needle = "    let mut open_object = MaybeUninit::uninit();";
    assert_eq!(generated.matches(needle).count(), 1);
    generated = generated.replace(needle, &format!("    policy::initialize()?;\n{needle}"));
    let needle = "        while !self.bpf.exited() {";
    assert_eq!(generated.matches(needle).count(), 1);
    generated = generated.replace(needle,
        "        let phase_started = Instant::now();\n        while !self.bpf.exited() {\n            if phase_started.elapsed() >= Duration::from_secs(60) { break; }");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("phase_experiment.rs"),
        generated,
    )
    .unwrap();
    scx_cargo::BpfBuilder::new()
        .unwrap()
        .enable_intf(baseline.join("intf.h").to_str().unwrap(), "bpf_intf.rs")
        .enable_skel(baseline.join("main.bpf.c").to_str().unwrap(), "bpf")
        .build()
        .unwrap();
    println!("cargo:rerun-if-changed={}", baseline.display());
    println!("cargo:rerun-if-changed=src/policy.rs");
    println!("cargo:rerun-if-changed=baseline-main.sha256");
}
