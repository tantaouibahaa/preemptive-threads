//! Links pt-os with the core crate's linker script and embeds the EL0 test
//! programs (built by `make user` into target/user, or `$PT_USER_DIR`).

use std::env;
use std::fmt::Write;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap().to_path_buf();

    let script = if env::var_os("CARGO_FEATURE_QEMU_VIRT").is_some() { "qemu_virt.ld" } else { "rpi0w2.ld" };
    if env::var("TARGET").map(|t| t.ends_with("-none")).unwrap_or(false) {
        println!("cargo:rustc-link-arg=-T{}", root.join(script).display());
    }
    println!("cargo:rerun-if-changed={}", root.join("qemu_virt.ld").display());
    println!("cargo:rerun-if-changed={}", root.join("rpi0w2.ld").display());

    // Every user/tests/src/bin/<name>.rs becomes an embedded program <name>.
    let user_dir = env::var_os("PT_USER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/user/aarch64-unknown-none/release"));
    let src_bin = root.join("user/tests/src/bin");
    let mut names: Vec<String> = fs::read_dir(&src_bin)
        .expect("user/tests/src/bin")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".rs").map(String::from))
        .collect();
    names.sort();
    let mut out = String::from("/// Embedded EL0 programs: (name, ELF image).\npub static PROGRAMS: &[(&str, &[u8])] = &[\n");
    for n in &names {
        let elf = user_dir.join(n);
        if !elf.exists() {
            panic!("user program `{}` not found at {}: run `make user` first (or set PT_USER_DIR)", n, elf.display());
        }
        writeln!(out, "    ({:?}, include_bytes!({:?})),", n, elf.display().to_string()).unwrap();
        println!("cargo:rerun-if-changed={}", elf.display());
    }
    out.push_str("];\n");
    fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("programs.rs"), out).unwrap();
    println!("cargo:rerun-if-changed={}", src_bin.display());
    println!("cargo:rerun-if-env-changed=PT_USER_DIR");
    println!("cargo:rerun-if-changed=build.rs");
}
