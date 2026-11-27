//! `regs <tag>`: sets TPIDR_EL0 to <tag>, then for 60 ms checks that it
//! and floating-point results survive preemption and system calls (it
//! never yields, so only the timer moves it off the core).
//! Exits 0, or 1 (TPIDR_EL0 changed) / 2 (FP result changed).
#![no_std]
#![no_main]

use core::arch::asm;
use ulib::{println, Args};

fn series(seed: f64, n: u32) -> f64 {
    let mut acc = seed;
    for i in 1..=n {
        let x = core::hint::black_box(i as f64 + seed);
        acc = acc * 0.999_9 + 1.0 / (x * 1.000_1 + acc);
    }
    acc
}

ulib::entry!(main);

fn main(args: Args) -> i32 {
    let tag = args.get(1).and_then(ulib::parse_u64).unwrap_or(0x7a6);
    // SAFETY: TPIDR_EL0 is ours to use.
    unsafe { asm!("msr tpidr_el0, {}", in(reg) tag) };
    let seed = (tag % 1000) as f64 + 0.25;
    let expected = series(seed, 500);
    let start = ulib::uptime_ns();
    let end = start + 60_000_000;
    let mut rounds = 0u32;
    while ulib::uptime_ns() < end {
        if series(seed, 500).to_bits() != expected.to_bits() {
            return 2;
        }
        let t: u64;
        // SAFETY: reading TPIDR_EL0 has no side effects.
        unsafe { asm!("mrs {}, tpidr_el0", out(reg) t) };
        if t != tag {
            return 1;
        }
        rounds += 1;
    }
    println!("regs ok: tag {:#x}, {} rounds on cpu {} from {} us to {} us", tag, rounds, ulib::cpu(), start / 1000, ulib::uptime_ns() / 1000);
    0
}
