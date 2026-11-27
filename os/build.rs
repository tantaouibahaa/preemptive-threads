//! Links pt-os with the core crate's linker script and builds the initrd.
//!
//! The initrd is a cpio (newc) archive with
//! - `/bin/<name>` for every `user/bin/src/bin/<name>.rs` (the userland),
//! - everything under `user/rootfs/` (`/etc/motd`, `/tmp`, ...),
//!
//! and a second archive for the test kernel with `/test/<name>` for every
//! `user/tests/src/bin/<name>.rs`. The programs must have been built by
//! `make user` (into target/user, or `$PT_USER_DIR`).

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

struct Cpio {
    out: Vec<u8>,
    ino: u32,
}

impl Cpio {
    fn new() -> Self {
        Cpio { out: Vec::new(), ino: 1 }
    }

    fn entry(&mut self, name: &str, mode: u32, data: &[u8]) {
        let mut h = String::from("070701");
        let fields = [self.ino, mode, 0, 0, 1, 0, data.len() as u32, 0, 0, 0, 0, name.len() as u32 + 1, 0];
        for f in fields {
            write!(h, "{:08x}", f).unwrap();
        }
        self.ino += 1;
        self.out.extend_from_slice(h.as_bytes());
        self.out.extend_from_slice(name.as_bytes());
        self.out.push(0);
        self.pad();
        self.out.extend_from_slice(data);
        self.pad();
    }

    fn pad(&mut self) {
        while !self.out.len().is_multiple_of(4) {
            self.out.push(0);
        }
    }

    fn dir(&mut self, name: &str) {
        self.entry(name, 0o040755, &[]);
    }

    fn file(&mut self, name: &str, mode: u32, data: &[u8]) {
        self.entry(name, 0o100000 | mode, data);
    }

    fn finish(mut self) -> Vec<u8> {
        self.entry("TRAILER!!!", 0, &[]);
        self.out
    }
}

fn program_names(src_bin: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(src_bin)
        .unwrap_or_else(|e| panic!("{}: {}", src_bin.display(), e))
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".rs").map(String::from))
        .collect();
    names.sort();
    println!("cargo:rerun-if-changed={}", src_bin.display());
    names
}

fn add_programs(cpio: &mut Cpio, dir: &str, src_bin: &Path, user_dir: &Path) {
    cpio.dir(dir);
    for n in program_names(src_bin) {
        let elf = user_dir.join(&n);
        let data = fs::read(&elf).unwrap_or_else(|_| {
            panic!("user program `{}` not found at {}: run `make user` first (or set PT_USER_DIR)", n, elf.display())
        });
        println!("cargo:rerun-if-changed={}", elf.display());
        cpio.file(&format!("{}/{}", dir, n), 0o755, &data);
    }
}

fn add_tree(cpio: &mut Cpio, root: &Path, rel: &str) {
    let dir = if rel.is_empty() { root.to_path_buf() } else { root.join(rel) };
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_str().unwrap().to_string();
        if name == ".keep" {
            continue;
        }
        let path = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
        if e.file_type().unwrap().is_dir() {
            cpio.dir(&path);
            add_tree(cpio, root, &path);
        } else {
            println!("cargo:rerun-if-changed={}", e.path().display());
            cpio.file(&path, 0o644, &fs::read(e.path()).unwrap());
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap().to_path_buf();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let script = if env::var_os("CARGO_FEATURE_QEMU_VIRT").is_some() { "qemu_virt.ld" } else { "rpi0w2.ld" };
    if env::var("TARGET").map(|t| t.ends_with("-none")).unwrap_or(false) {
        println!("cargo:rustc-link-arg=-T{}", root.join(script).display());
    }
    println!("cargo:rerun-if-changed={}", root.join("qemu_virt.ld").display());
    println!("cargo:rerun-if-changed={}", root.join("rpi0w2.ld").display());

    let user_dir = env::var_os("PT_USER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/user/aarch64-unknown-none/release"));

    let mut base = Cpio::new();
    add_programs(&mut base, "bin", &root.join("user/bin/src/bin"), &user_dir);
    add_tree(&mut base, &root.join("user/rootfs"), "");
    fs::write(out_dir.join("initrd.cpio"), base.finish()).unwrap();

    let mut test = Cpio::new();
    add_programs(&mut test, "test", &root.join("user/tests/src/bin"), &user_dir);
    fs::write(out_dir.join("initrd-test.cpio"), test.finish()).unwrap();

    let src = "/// The initrd: `/bin`, `/etc`, `/tmp` (cpio newc).\n\
               pub static ARCHIVE: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/initrd.cpio\"));\n\
               /// The kernel-test programs, under `/test` (cpio newc).\n\
               pub static TEST_ARCHIVE: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/initrd-test.cpio\"));\n";
    fs::write(out_dir.join("initrd.rs"), src).unwrap();
    println!("cargo:rerun-if-env-changed=PT_USER_DIR");
    println!("cargo:rerun-if-changed=build.rs");
}
