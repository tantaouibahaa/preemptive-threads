//! `primes [N]`: count the primes below N (default 1000000) with a sieve
//! of Eratosthenes. CPU heavy on purpose.
#![no_std]
#![no_main]

use ulib::prelude::*;

ulib::entry!(main);

fn main(args: Args) -> i32 {
    let n = args.get(1).and_then(ulib::parse_u64).unwrap_or(1_000_000) as usize;
    let t0 = ulib::uptime_ns();
    let mut composite = vec![false; n.max(2)];
    let mut count = 0usize;
    let mut i = 2;
    while i < n {
        if !composite[i] {
            count += 1;
            let mut j = i * i;
            while j < n {
                composite[j] = true;
                j += i;
            }
        }
        i += 1;
    }
    let ms = (ulib::uptime_ns() - t0) / 1_000_000;
    println!("primes below {}: {} ({} ms, pid {} on cpu{})", n, count, ms, ulib::getpid(), ulib::cpu());
    0
}
