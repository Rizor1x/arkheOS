fn main() {
    assert_eq!(
        std::env::var("TARGET").expect("Cargo supplies TARGET"),
        "x86_64-unknown-none"
    );
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies manifest path");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-env-changed=ARKHE_TEST_CASE");
    let case = std::env::var("ARKHE_TEST_CASE").unwrap_or_else(|_| "boot".into());
    println!("cargo:rustc-env=ARKHE_TEST_CASE={case}");
    println!("cargo:rustc-link-arg=-T{root}/linker.ld");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-zmax-page-size=0x1000");
}
