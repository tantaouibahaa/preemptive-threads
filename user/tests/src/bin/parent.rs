//! Spawns, waits for and kills children from user mode. Exits 0 on success
//! or the number of the failed check.
#![no_std]
#![no_main]

use ulib::{errno, println, Args};

ulib::entry!(main);

fn main(_args: Args) -> i32 {
    let child = ulib::spawn(b"hello\0", &[b"hello\0", b"7\0"]);
    if child <= 0 {
        return 1;
    }
    let (pid, status) = ulib::wait(child);
    if pid != child || status != 7 {
        println!("parent: wait -> {} status {}", pid, status);
        return 2;
    }
    let busy = ulib::spawn(b"/test/busy\0", &[b"busy\0"]);
    if busy <= 0 {
        return 3;
    }
    ulib::sleep_ns(5_000_000);
    if ulib::kill(busy) != 0 {
        return 4;
    }
    let (pid, status) = ulib::wait(-1);
    if pid != busy || status != -9 {
        println!("parent: wait(busy) -> {} status {}", pid, status);
        return 5;
    }
    if ulib::spawn(b"no-such-program\0", &[b"x\0"]) != -errno::ENOENT {
        return 6;
    }
    if ulib::wait(-1).0 != -errno::ECHILD {
        return 7;
    }
    if ulib::kill(busy) != -errno::ESRCH {
        return 8;
    }
    println!("parent ok: child {} exited 7, busy child {} killed", child, busy);
    0
}
