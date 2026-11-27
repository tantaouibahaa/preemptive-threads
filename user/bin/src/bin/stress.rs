//! `stress [N]`: spawn and reap N short-lived processes (8 at a time) and
//! check that every physical frame comes back.
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::SpawnAttr;

ulib::entry!(main);

const BATCH: usize = 8;

fn run_batch(k: usize) -> Result<(), Errno> {
    let attr = SpawnAttr::new().fd(0, 0).fd(1, 1).fd(2, 2);
    let mut pids = Vec::with_capacity(k);
    for _ in 0..k {
        pids.push(ulib::spawn_path("/bin/stress", &["stress", "-c"], Some(&attr))?);
    }
    for pid in pids {
        let s = ulib::wait_pid(pid)?;
        if s != 0 {
            eprintln!("stress: child {} {}", pid, ulib::status_text(s));
        }
    }
    Ok(())
}

fn main(args: Args) -> i32 {
    if args.str(1) == Some("-c") {
        return 0; // a child: exit right away
    }
    let n = args.get(1).and_then(ulib::parse_u64).unwrap_or(100) as usize;
    // Warm up, so our own heap has grown before we take the baseline.
    if let Err(e) = run_batch(1) {
        eprintln!("stress: {}", e);
        return 1;
    }
    let before = ulib::sysinfo().frames_free;
    let t0 = ulib::uptime_ns();
    let mut done = 0;
    while done < n {
        let k = BATCH.min(n - done);
        if let Err(e) = run_batch(k) {
            eprintln!("stress: after {} processes: {}", done, e);
            return 1;
        }
        done += k;
    }
    let ms = ((ulib::uptime_ns() - t0) / 1_000_000).max(1);
    let after = ulib::sysinfo().frames_free;
    println!(
        "stress: {} spawn/exit cycles in {} ms ({} per second); free frames before {}, after {} ({:+})",
        n,
        ms,
        n as u64 * 1000 / ms,
        before,
        after,
        after as i64 - before as i64
    );
    if before == after {
        0
    } else {
        1
    }
}
