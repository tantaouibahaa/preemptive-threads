use std::env;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    if env::var("TARGET").map(|t| t.ends_with("-none")).unwrap_or(false) {
        println!("cargo:rustc-link-arg-bins=-T{}", dir.join("uart0.ld").display());
    }
    println!("cargo:rerun-if-changed=uart0.ld");
    println!("cargo:rerun-if-changed=build.rs");
}
