//! `wc [file...]`: count lines, words and bytes.
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::File;

ulib::entry!(main);

#[derive(Default, Clone, Copy)]
struct Counts {
    lines: usize,
    words: usize,
    bytes: usize,
}

fn count(fd: u64) -> Result<Counts, Errno> {
    let mut c = Counts::default();
    let mut in_word = false;
    let mut buf = [0u8; 1024];
    loop {
        let n = Errno::check(ulib::read(fd, &mut buf))? as usize;
        if n == 0 {
            return Ok(c);
        }
        c.bytes += n;
        for &b in &buf[..n] {
            if b == b'\n' {
                c.lines += 1;
            }
            let space = b == b' ' || b == b'\n' || b == b'\t' || b == b'\r';
            if !space && !in_word {
                c.words += 1;
            }
            in_word = !space;
        }
    }
}

fn show(c: Counts, name: &str) {
    if name.is_empty() {
        println!("{:>7} {:>7} {:>7}", c.lines, c.words, c.bytes);
    } else {
        println!("{:>7} {:>7} {:>7} {}", c.lines, c.words, c.bytes, name);
    }
}

fn main(args: Args) -> i32 {
    if args.len() < 2 {
        return match count(0) {
            Ok(c) => {
                show(c, "");
                0
            }
            Err(e) => {
                eprintln!("wc: {}", e);
                1
            }
        };
    }
    let mut total = Counts::default();
    let mut status = 0;
    for path in args.iter().skip(1) {
        match File::read_only(path).and_then(|f| count(f.fd() as u64)) {
            Ok(c) => {
                show(c, path);
                total.lines += c.lines;
                total.words += c.words;
                total.bytes += c.bytes;
            }
            Err(e) => {
                eprintln!("wc: {}: {}", path, e);
                status = 1;
            }
        }
    }
    if args.len() > 2 {
        show(total, "total");
    }
    status
}
