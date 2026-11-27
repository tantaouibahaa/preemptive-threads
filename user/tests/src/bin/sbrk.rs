//! Grows and shrinks the heap with sbrk and checks the memory. Exits 0 on
//! success, or the number of the failed step. `sbrk fault` additionally
//! touches memory that was given back, which must kill it.
#![no_std]
#![no_main]

use ulib::{println, Args};

const SMALL: i64 = 10_000;
const BIG: i64 = 1 << 20;

fn pattern(i: u64) -> u8 {
    (i.wrapping_mul(31) ^ (i >> 7)) as u8
}

ulib::entry!(main);

fn main(args: Args) -> i32 {
    let base = ulib::sbrk(0);
    if base <= 0 || base % 4096 != 0 {
        return 1;
    }
    if ulib::sbrk(SMALL) != base {
        return 2;
    }
    let p = base as *mut u8;
    // SAFETY: [base, base + SMALL) was just given to us.
    unsafe {
        for i in 0..SMALL as u64 {
            p.add(i as usize).write_volatile(pattern(i));
        }
    }
    if ulib::sbrk(0) != base + SMALL {
        return 3;
    }
    if ulib::sbrk(BIG) != base + SMALL {
        return 4;
    }
    // SAFETY: the heap now extends to base + SMALL + BIG.
    unsafe {
        let mut a = base + SMALL;
        while a < base + SMALL + BIG {
            (a as *mut u8).write_volatile(0x5a);
            a += 4096;
        }
        for i in 0..SMALL as u64 {
            if p.add(i as usize).read_volatile() != pattern(i) {
                return 5;
            }
        }
    }
    if ulib::sbrk(-BIG) != base + SMALL + BIG || ulib::sbrk(0) != base + SMALL {
        return 6;
    }
    if ulib::sbrk(1 << 40) != -ulib::errno::ENOMEM {
        return 7;
    }
    println!("sbrk ok: heap at {:#x} grew to {} KiB and shrank back", base, (SMALL + BIG) / 1024);
    if args.get(1) == Some(b"fault") {
        // SAFETY: deliberately touches a page that was unmapped by the shrink.
        let v = unsafe { ((base + SMALL + BIG / 2) as *const u8).read_volatile() };
        println!("sbrk: read freed heap = {} (survived)", v);
    }
    0
}
