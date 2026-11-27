//! `spin <ms>`: burn CPU for <ms> milliseconds, then exit with the mask of
//! cores we ran on.
#![no_std]
#![no_main]

use ulib::Args;

ulib::entry!(main);

fn main(args: Args) -> i32 {
    let ms = args.get(1).and_then(ulib::parse_u64).unwrap_or(20);
    let end = ulib::uptime_ns() + ms * 1_000_000;
    let mut mask = 0i32;
    let mut x = 1u64;
    while ulib::uptime_ns() < end {
        mask |= 1 << ulib::cpu();
        for i in 0..2000u64 {
            x = core::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(i));
        }
    }
    mask
}
