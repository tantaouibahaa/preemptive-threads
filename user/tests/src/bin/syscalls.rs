//! Exercises the simple system calls and the kernel's pointer checks.
//! Exits 0 on success or the number of the failed check.
#![no_std]
#![no_main]

use ut::{errno, nr, println, Args};

#[no_mangle]
fn ut_main(_args: Args) -> i32 {
    let pid = ut::getpid();
    if pid <= 0 {
        return 1;
    }
    let cpu = ut::cpu();
    if !(0..4).contains(&cpu) {
        return 2;
    }
    let t0 = ut::uptime_ns();
    if ut::sleep_ns(20_000_000) != 0 {
        return 3;
    }
    let slept = ut::uptime_ns() - t0;
    if slept < 20_000_000 {
        return 4;
    }
    if ut::yield_now() != 0 {
        return 5;
    }
    // SAFETY (all calls below): bad pointers on purpose; the kernel must
    // reject them with EFAULT instead of touching them.
    // A kernel address (the boot area / RAM of both boards).
    if unsafe { ut::syscall(nr::WRITE, 1, 0x8_0000, 8) } != -errno::EFAULT {
        return 6;
    }
    if unsafe { ut::syscall(nr::WRITE, 1, 0x4008_0000, 8) } != -errno::EFAULT {
        return 7;
    }
    // The unmapped start of the user window.
    if unsafe { ut::syscall(nr::WRITE, 1, 0x40_0000_0000, 8) } != -errno::EFAULT {
        return 8;
    }
    // A range that wraps around the address space.
    if unsafe { ut::syscall(nr::WRITE, 1, u64::MAX - 4, 16) } != -errno::EFAULT {
        return 9;
    }
    if ut::write(7, b"x") != -errno::EBADF {
        return 10;
    }
    if unsafe { ut::syscall(999, 0, 0, 0) } != -errno::ENOSYS {
        return 11;
    }
    // wait() with a status pointer into kernel memory, then with no child.
    if unsafe { ut::syscall(nr::WAIT, u64::MAX, 0x8_0000, 0) } != -errno::EFAULT {
        return 12;
    }
    if ut::wait(-1).0 != -errno::ECHILD {
        return 13;
    }
    println!("syscalls ok: pid={} cpu={} slept {} us", pid, cpu, slept / 1000);
    0
}
