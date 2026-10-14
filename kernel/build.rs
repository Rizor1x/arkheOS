fn main() {
    let target = std::env::var("TARGET").expect("Cargo supplies TARGET");
    assert_eq!(target, "x86_64-unknown-none", "use cargo xtask build");
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies manifest path");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-env-changed=ARKHE_TEST_CASE");
    let case = std::env::var("ARKHE_TEST_CASE").unwrap_or_else(|_| "boot".into());
    assert!(matches!(
        case.as_str(),
        "boot"
            | "scheduler"
            | "idle"
            | "user-fpu"
            | "divide"
            | "page-fault"
            | "double-fault"
            | "user-kernel-read"
            | "user-cli"
            | "user-write-code"
            | "user-exec-stack"
    ));
    println!("cargo:rustc-env=ARKHE_TEST_CASE={case}");
    println!("cargo:rustc-link-arg=-T{root}/linker.ld");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-zmax-page-size=0x1000");
}
