//! `crash [read|write|exec|undef]`: deliberately fault (default: write
//! through a null pointer). The kernel kills this process and nothing
//! else.
#![no_std]
#![no_main]

use ulib::prelude::*;

ulib::entry!(main);

fn main(args: Args) -> i32 {
    let how = args.str(1).unwrap_or("write");
    println!("crash: {} through a bad pointer...", how);
    // SAFETY (all arms): deliberately invalid accesses; the point of this
    // program is to be killed by the kernel.
    unsafe {
        match how {
            "read" => {
                let v = core::ptr::read_volatile(0x10 as *const u64);
                println!("crash: read {} (should not get here)", v);
            }
            "exec" => {
                let f: extern "C" fn() = core::mem::transmute(0x1000usize);
                f();
            }
            "undef" => core::arch::asm!("udf #0"),
            // black_box hides the null address from the optimizer, so the
            // store really happens (and faults).
            _ => core::ptr::write_volatile(core::hint::black_box(0usize) as *mut u64, 0xdead),
        }
    }
    println!("crash: survived?!");
    1
}
