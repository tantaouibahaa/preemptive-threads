//! pt-os kernel: unpack the initrd, start the console, print the message
//! of the day and run `/bin/sh` on the console, starting a new one
//! whenever it exits.

#![no_std]
#![no_main]

extern crate alloc;

use preemptive_threads::{self as pt, println, Config, Duration};
use pt_os::process::{self, describe_status};
use pt_os::vfs::FdTable;
use pt_os::{console, initrd, vfs, Image, SpawnOptions};

#[no_mangle]
fn kernel_main() -> ! {
    println!();
    println!("pt-os ({}, entered at EL{})", pt::platform::NAME, pt::boot_el());
    pt::start(Config::default(), main)
}

fn main() {
    match initrd::mount() {
        Ok(n) => println!("[pt-os] initrd: {} files", n),
        Err(e) => println!("[pt-os] initrd: corrupt archive ({:?})", e),
    }
    console::init();
    if let Ok(motd) = vfs::read_file("/etc/motd") {
        console::write(&motd);
    }
    loop {
        let opts = SpawnOptions { files: Some(FdTable::console()), ..Default::default() };
        match pt_os::spawn(Image::Named("/bin/sh"), &[b"sh"], opts) {
            Ok(pid) => {
                let (_, status) = process::wait(Some(pid)).expect("wait for sh");
                console::set_foreground(0);
                println!("[pt-os] sh (pid {}) {}; starting a new one", pid, describe_status(status));
            }
            Err(e) => {
                println!("[pt-os] cannot start /bin/sh: {:?}", e);
                pt::sleep(Duration::from_secs(5));
            }
        }
    }
}
