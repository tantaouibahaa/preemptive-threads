//! `cat [file...]`: copy files (or standard input) to standard output.
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::File;

ulib::entry!(main);

fn copy(fd: u64) -> Result<(), Errno> {
    let mut buf = [0u8; 1024];
    loop {
        let n = Errno::check(ulib::read(fd, &mut buf))? as usize;
        if n == 0 {
            return Ok(());
        }
        let mut out = &buf[..n];
        while !out.is_empty() {
            let w = Errno::check(ulib::write(1, out))? as usize;
            out = &out[w..];
        }
    }
}

fn main(args: Args) -> i32 {
    if args.len() < 2 {
        return match copy(0) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("cat: {}", e);
                1
            }
        };
    }
    let mut status = 0;
    for path in args.iter().skip(1) {
        let r = File::read_only(path).and_then(|f| copy(f.fd() as u64));
        if let Err(e) = r {
            if e.0 == ulib::errno::EPIPE {
                return 141;
            }
            eprintln!("cat: {}: {}", path, e);
            status = 1;
        }
    }
    status
}
