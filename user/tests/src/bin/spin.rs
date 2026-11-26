//! `spin <ms>`: burn CPU for <ms> milliseconds, then exit with the mask of
//! cores we ran on.
#![no_std]
#![no_main]

use ut::Args;

#[no_mangle]
fn ut_main(args: Args) -> i32 {
    let ms = args.get(1).and_then(ut::parse_u64).unwrap_or(20);
    let end = ut::uptime_ns() + ms * 1_000_000;
    let mut mask = 0i32;
    let mut x = 1u64;
    while ut::uptime_ns() < end {
        mask |= 1 << ut::cpu();
        for i in 0..2000u64 {
            x = core::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(i));
        }
    }
    mask
}
