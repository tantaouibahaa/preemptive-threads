//! `hello [code]`: prints a greeting and exits with `code` (default 0).
#![no_std]
#![no_main]

use ulib::prelude::*;

ulib::entry!(main);

fn main(args: Args) -> i32 {
    println!("hello from EL0 (pid {}, cpu {}, argc {})", ulib::getpid(), ulib::cpu(), args.len());
    args.get(1).and_then(ulib::parse_u64).unwrap_or(0) as i32
}
