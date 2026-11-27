//! `mandel [width height [iterations]]`: an ASCII Mandelbrot set computed
//! by 4 worker processes (one per core), each sending its rows back
//! through its own pipe. The default, 78x32 at 150000 iterations, fills an
//! 80-column terminal and takes about 0.3 s on QEMU (nearly all of it in
//! the points inside the set).
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::{File, SpawnAttr};

ulib::entry!(main);

const WORKERS: usize = 4;
/// Outside the set, by escape time (on a log scale); `@` is the set.
const SHADES: &[u8] = b" .,:;-=+*#%";
const IN_SET: u8 = b'@';

fn row(y: usize, w: usize, h: usize, iters: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(w);
    // Sample the middle of each character cell.
    let ci = -1.2 + 2.4 * (y as f64 + 0.5) / h as f64;
    for x in 0..w {
        let cr = -2.2 + 3.0 * (x as f64 + 0.5) / w as f64;
        let (mut zr, mut zi) = (0.0f64, 0.0f64);
        let mut i = 0;
        while i < iters && zr * zr + zi * zi < 4.0 {
            let t = zr * zr - zi * zi + cr;
            zi = 2.0 * zr * zi + ci;
            zr = t;
            i += 1;
        }
        out.push(if i >= iters { IN_SET } else { SHADES[shade(i)] });
    }
    out
}

/// About 1.4 * log2(i), without floating-point logarithms (no libm).
fn shade(i: u32) -> usize {
    let log2 = 31 - i.max(1).leading_zeros() as usize;
    // Half steps: is i past 2^log2 * 1.5?
    let half = (i as usize) >= (3usize << log2) / 2;
    ((log2 * 14 + if half { 7 } else { 0 }) / 10).min(SHADES.len() - 1)
}

/// `mandel -w index count width height iterations`: compute rows
/// index, index + count, ... and write `row:chars` lines to stdout,
/// then a `cpus <mask>` line.
fn worker(args: &Args) -> i32 {
    let num = |i: usize| args.get(i).and_then(ulib::parse_u64).unwrap_or(0) as usize;
    let (idx, count, w, h, iters) = (num(2), num(3).max(1), num(4), num(5), num(6) as u32);
    let mut mask = 0u32;
    let mut out = String::new();
    let mut y = idx;
    while y < h {
        mask |= 1 << ulib::cpu();
        out.push_str(&format!("{}:", y));
        out.push_str(core::str::from_utf8(&row(y, w, h, iters)).unwrap_or(""));
        out.push('\n');
        y += count;
    }
    out.push_str(&format!("cpus {}\n", mask));
    print!("{}", out);
    0
}

fn main(args: Args) -> i32 {
    if args.str(1) == Some("-w") {
        return worker(&args);
    }
    let w = args.get(1).and_then(ulib::parse_u64).unwrap_or(78).clamp(8, 200) as usize;
    let h = args.get(2).and_then(ulib::parse_u64).unwrap_or(32).clamp(4, 100) as usize;
    let iters = args.get(3).and_then(ulib::parse_u64).unwrap_or(150_000).clamp(10, 1_000_000) as u32;
    let t0 = ulib::uptime_ns();
    let mut readers = Vec::new();
    let mut pids = Vec::new();
    for i in 0..WORKERS {
        let mut fds = [0i32; 2];
        if ulib::pipe(&mut fds) < 0 {
            eprintln!("mandel: pipe failed");
            return 1;
        }
        let attr = SpawnAttr::new().fd(0, 0).fd(1, fds[1]).fd(2, 2);
        let (si, sc, sw, sh, sit) = (format!("{}", i), format!("{}", WORKERS), format!("{}", w), format!("{}", h), format!("{}", iters));
        let argv = ["mandel", "-w", si.as_str(), sc.as_str(), sw.as_str(), sh.as_str(), sit.as_str()];
        let r = ulib::spawn_path("/bin/mandel", &argv, Some(&attr));
        ulib::close(fds[1] as u64);
        match r {
            Ok(pid) => pids.push(pid),
            Err(e) => {
                eprintln!("mandel: spawn worker: {}", e);
                return 1;
            }
        }
        readers.push(File::from_fd(fds[0]));
    }
    let mut rows: Vec<String> = vec![String::new(); h];
    let mut cpus = 0u32;
    for r in &readers {
        let text = r.read_to_string().unwrap_or_default();
        for line in text.lines() {
            if let Some(m) = line.strip_prefix("cpus ") {
                cpus |= m.parse::<u32>().unwrap_or(0);
            } else if let Some((y, chars)) = line.split_once(':') {
                if let Ok(y) = y.parse::<usize>() {
                    if y < h {
                        rows[y] = String::from(chars);
                    }
                }
            }
        }
    }
    let mut failed = 0;
    for &pid in &pids {
        if ulib::wait_pid(pid) != Ok(0) {
            failed += 1;
        }
    }
    for r in &rows {
        println!("{}", r);
    }
    let ms = (ulib::uptime_ns() - t0) / 1_000_000;
    let used: Vec<String> = (0..4).filter(|c| cpus & (1 << c) != 0).map(|c| format!("{}", c)).collect();
    println!("mandel: {}x{}, {} iterations, {} workers on cpus {}, {} ms", w, h, iters, WORKERS, used.join(","), ms);
    if failed == 0 {
        0
    } else {
        1
    }
}
