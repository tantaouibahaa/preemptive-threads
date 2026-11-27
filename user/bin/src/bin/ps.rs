//! `ps`: list processes.
#![no_std]
#![no_main]

use ulib::prelude::*;

ulib::entry!(main);

fn cores(mask: u8) -> String {
    (0..4).map(|i| if mask & (1 << i) != 0 { char::from(b'0' + i) } else { '.' }).collect()
}

fn main(_args: Args) -> i32 {
    println!("  PID  PPID  PGID S CPU RAN      TIME  MEM CMD");
    for p in ulib::procinfo() {
        let ms = p.cpu_time_ns / 1_000_000;
        println!(
            "{:>5} {:>5} {:>5} {} {:>3} {} {:>5}.{:03}s {:>3}K {}",
            p.pid,
            p.ppid,
            p.pgid,
            p.state,
            p.cpu,
            cores(p.cpus_ran),
            ms / 1000,
            ms % 1000,
            p.pages * 4,
            p.name
        );
    }
    0
}
