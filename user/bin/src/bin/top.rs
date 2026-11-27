//! `top [-n N] [-d ms]`: a refreshing per-CPU view: how busy each core
//! is, which process runs on it, and every process with its state, core
//! and CPU time. Quits on any key, or after N refreshes with `-n N`.
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::{ProcInfo, SysInfo};

ulib::entry!(main);

fn bar(pct: u64) -> String {
    let n = (pct.min(100) / 5) as usize;
    let mut s = String::from("[");
    for i in 0..20 {
        s.push(if i < n { '#' } else { ' ' });
    }
    s.push(']');
    s
}

fn render(prev: &SysInfo, now: &SysInfo, prev_procs: &[ProcInfo], procs: &[ProcInfo], tty: bool) -> String {
    let mut out = String::new();
    if tty {
        out.push_str("\x1b[H\x1b[2J");
    }
    let up = now.uptime_ns / 1_000_000;
    out.push_str(&format!(
        "top - up {}.{:01}s, {} processes, {} cpus, {} of {} frames free\n",
        up / 1000,
        (up % 1000) / 100,
        procs.len(),
        now.cpus_online,
        now.frames_free,
        now.frames_total
    ));
    for (i, (c, p)) in now.cpus.iter().zip(prev.cpus.iter()).enumerate() {
        if !c.online {
            continue;
        }
        let ticks = c.ticks.saturating_sub(p.ticks).max(1);
        let idle = c.idle_ticks.saturating_sub(p.idle_ticks).min(ticks);
        let busy = 100 * (ticks - idle) / ticks;
        let running: Vec<String> =
            procs.iter().filter(|q| q.state == 'R' && q.cpu as usize == i).map(|q| format!("pid {} {}", q.pid, q.name)).collect();
        let what = if running.is_empty() { String::from("idle") } else { running.join(", ") };
        out.push_str(&format!("cpu{} {} {:>3}%  {}\n", i, bar(busy), busy, what));
    }
    let wall = now.uptime_ns.saturating_sub(prev.uptime_ns).max(1);
    let mut rows: Vec<(u64, &ProcInfo)> = procs
        .iter()
        .map(|q| {
            let before = prev_procs.iter().find(|o| o.pid == q.pid).map_or(0, |o| o.cpu_time_ns);
            (100 * q.cpu_time_ns.saturating_sub(before) / wall, q)
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.pid.cmp(&b.1.pid)));
    out.push_str("\n  PID S CPU %CPU     TIME CMD\n");
    for (pct, q) in rows {
        let ms = q.cpu_time_ns / 1_000_000;
        out.push_str(&format!(
            "{:>5} {} {:>3} {:>4} {:>4}.{:03}s {}\n",
            q.pid,
            q.state,
            q.cpu,
            pct,
            ms / 1000,
            ms % 1000,
            q.name
        ));
    }
    out
}

fn main(args: Args) -> i32 {
    let mut iterations: Option<u64> = None;
    let mut delay_ms = 1000u64;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a {
            "-n" => iterations = it.next().and_then(|v| v.parse().ok()),
            "-d" => delay_ms = it.next().and_then(|v| v.parse().ok()).unwrap_or(1000).max(50),
            _ => {
                eprintln!("usage: top [-n N] [-d ms]");
                return 2;
            }
        }
    }
    let tty = ulib::is_console(0);
    if tty {
        ulib::console_raw(0, true);
    }
    let mut prev = ulib::sysinfo();
    let mut prev_procs = ulib::procinfo();
    ulib::sleep_ms(delay_ms.min(200));
    let mut n = 0u64;
    'outer: loop {
        let now = ulib::sysinfo();
        let procs = ulib::procinfo();
        print!("{}", render(&prev, &now, &prev_procs, &procs, tty));
        n += 1;
        if iterations.is_some_and(|max| n >= max) {
            break;
        }
        if tty {
            println!("(press any key to quit)");
        }
        prev = now;
        prev_procs = procs;
        let mut waited = 0;
        while waited < delay_ms {
            if tty && ulib::input_available(0) > 0 {
                let mut b = [0u8; 16];
                ulib::read(0, &mut b);
                break 'outer;
            }
            let step = 50.min(delay_ms - waited);
            if ulib::sleep_ms(step) < 0 {
                break 'outer;
            }
            waited += step;
        }
    }
    if tty {
        ulib::console_raw(0, false);
    }
    0
}
