//! Prints a greeting and exits with argv[1] (default 0).
#![no_std]
#![no_main]

use ut::{println, Args};

#[no_mangle]
fn ut_main(args: Args) -> i32 {
    println!("hello from EL0 (pid {}, cpu {}, argc {})", ut::getpid(), ut::cpu(), args.len());
    args.get(1).and_then(ut::parse_u64).unwrap_or(0) as i32
}
