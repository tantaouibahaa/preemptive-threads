//! pt-os kernel. For now it boots, runs a couple of the embedded user
//! programs and reports their exit status; the shell comes later.

#![no_std]
#![no_main]

use preemptive_threads::{self as pt, println, Config};
use pt_os::{Image, SpawnOptions};

#[no_mangle]
fn kernel_main() -> ! {
    println!();
    println!("pt-os ({}, entered at EL{})", pt::platform::NAME, pt::boot_el());
    pt::start(Config::default(), main)
}

fn main() {
    let names: alloc::vec::Vec<&str> = pt_os::programs::names().collect();
    println!("[pt-os] {} embedded programs: {:?}", names.len(), names);
    for argv in [&[&b"hello"[..]][..], &[b"parent"]] {
        let name = core::str::from_utf8(argv[0]).unwrap();
        match pt_os::spawn(Image::Named(name), argv, SpawnOptions::default()) {
            Ok(pid) => {
                let (_, status) = pt_os::wait(Some(pid)).expect("wait");
                println!("[pt-os] pid {} ({}) exited with status {}", pid, name, status);
            }
            Err(e) => println!("[pt-os] spawn {} failed: {:?}", name, e),
        }
    }
    let s = pt_os::stats();
    println!("[pt-os] {} of {} frames free, {} ASIDs free; idle", s.frames_free, s.frames_total, s.asids_free);
}

extern crate alloc;
