fn main() {
    let target = std::env::var("TARGET").expect("Cargo supplies TARGET");
    assert_eq!(target, "x86_64-unknown-none", "use cargo xtask build");
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies manifest path");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rustc-link-arg=-T{root}/linker.ld");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-zmax-page-size=0x1000");
}
