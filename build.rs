use std::env;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let script = if env::var_os("CARGO_FEATURE_QEMU_VIRT").is_some() {
        "qemu_virt.ld"
    } else {
        "rpi0w2.ld"
    };
    // Only bare-metal targets get our linker script.
    if env::var("TARGET").map(|t| t.ends_with("-none")).unwrap_or(false) {
        println!("cargo:rustc-link-arg=-T{}", dir.join(script).display());
    }
    println!("cargo:rerun-if-changed=qemu_virt.ld");
    println!("cargo:rerun-if-changed=rpi0w2.ld");
    println!("cargo:rerun-if-changed=build.rs");
}
