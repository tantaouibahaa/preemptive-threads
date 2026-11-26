//! Spawns, waits for and kills children from user mode. Exits 0 on success
//! or the number of the failed check.
#![no_std]
#![no_main]

use ut::{errno, println, Args};

#[no_mangle]
fn ut_main(_args: Args) -> i32 {
    let child = ut::spawn(b"hello\0", &[b"hello\0", b"7\0"]);
    if child <= 0 {
        return 1;
    }
    let (pid, status) = ut::wait(child);
    if pid != child || status != 7 {
        println!("parent: wait -> {} status {}", pid, status);
        return 2;
    }
    let busy = ut::spawn(b"busy\0", &[b"busy\0"]);
    if busy <= 0 {
        return 3;
    }
    ut::sleep_ns(5_000_000);
    if ut::kill(busy) != 0 {
        return 4;
    }
    let (pid, status) = ut::wait(-1);
    if pid != busy || status != -9 {
        println!("parent: wait(busy) -> {} status {}", pid, status);
        return 5;
    }
    if ut::spawn(b"no-such-program\0", &[b"x\0"]) != -errno::ENOENT {
        return 6;
    }
    if ut::wait(-1).0 != -errno::ECHILD {
        return 7;
    }
    if ut::kill(busy) != -errno::ESRCH {
        return 8;
    }
    println!("parent ok: child {} exited 7, busy child {} killed", child, busy);
    0
}
