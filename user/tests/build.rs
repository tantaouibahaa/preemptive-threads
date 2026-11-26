use std::env;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    if env::var("TARGET").map(|t| t.ends_with("-none")).unwrap_or(false) {
        println!("cargo:rustc-link-arg-bins=-T{}", dir.join("user.ld").display());
        // Segments are page aligned in memory; no need for 64 KiB file padding.
        println!("cargo:rustc-link-arg-bins=-zmax-page-size=4096");
    }
    println!("cargo:rerun-if-changed=user.ld");
    println!("cargo:rerun-if-changed=build.rs");
}
