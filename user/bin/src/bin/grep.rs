//! `grep [-v] [-c] [-n] pattern [file...]`: print lines containing
//! `pattern` (a plain substring). Exits 0 if something matched, 1 if not.
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::{File, LineReader};

ulib::entry!(main);

struct Opts<'a> {
    pattern: &'a str,
    invert: bool,
    count: bool,
    number: bool,
    prefix: bool,
}

fn grep(fd: i32, name: &str, o: &Opts<'_>) -> usize {
    let mut r = LineReader::new(fd);
    let mut hits = 0;
    let mut lineno = 0;
    while let Some(line) = r.read_line() {
        lineno += 1;
        if line.contains(o.pattern) != o.invert {
            hits += 1;
            if !o.count {
                let pre = if o.prefix { format!("{}:", name) } else { String::new() };
                if o.number {
                    println!("{}{}:{}", pre, lineno, line);
                } else {
                    println!("{}{}", pre, line);
                }
            }
        }
    }
    if o.count {
        if o.prefix {
            println!("{}:{}", name, hits);
        } else {
            println!("{}", hits);
        }
    }
    hits
}

fn main(args: Args) -> i32 {
    let mut o = Opts { pattern: "", invert: false, count: false, number: false, prefix: false };
    let mut rest: Vec<&str> = Vec::new();
    for a in args.iter().skip(1) {
        match a {
            "-v" if rest.is_empty() => o.invert = true,
            "-c" if rest.is_empty() => o.count = true,
            "-n" if rest.is_empty() => o.number = true,
            _ => rest.push(a),
        }
    }
    let Some((&pattern, files)) = rest.split_first() else {
        eprintln!("usage: grep [-v] [-c] [-n] pattern [file...]");
        return 2;
    };
    o.pattern = pattern;
    o.prefix = files.len() > 1;
    let mut hits = 0;
    if files.is_empty() {
        hits += grep(0, "", &o);
    }
    for path in files {
        match File::read_only(path) {
            Ok(f) => hits += grep(f.fd(), path, &o),
            Err(e) => eprintln!("grep: {}: {}", path, e),
        }
    }
    if hits > 0 {
        0
    } else {
        1
    }
}
