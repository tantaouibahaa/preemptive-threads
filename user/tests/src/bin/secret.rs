//! Puts a secret on its heap, announces it (unless run as `secret quiet`),
//! and sleeps until killed.
#![no_std]
#![no_main]

use ut::{println, Args};

pub const SECRET: u64 = 0x5ec2_e75e_c2e7_5ec2;

#[no_mangle]
fn ut_main(args: Args) -> i32 {
    let heap = ut::sbrk(4096);
    if heap <= 0 {
        return 1;
    }
    // SAFETY: the page was just added to our heap.
    unsafe { (heap as *mut u64).write_volatile(SECRET) };
    if args.get(1) != Some(b"quiet") {
        println!("secret: stored at {:#x}", heap);
    }
    loop {
        ut::sleep_ns(1_000_000_000);
    }
}
