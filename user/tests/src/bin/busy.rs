//! Loops forever without ever making a system call: only timer preemption
//! lets anything else run on its core, and only `kill` stops it.
#![no_std]
#![no_main]

use ulib::Args;

ulib::entry!(main);

fn main(_args: Args) -> i32 {
    let mut x = 1u64;
    loop {
        x = core::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
    }
}
