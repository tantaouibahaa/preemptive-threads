//! `rm path...`: remove files or empty directories.
#![no_std]
#![no_main]

use ulib::prelude::*;

ulib::entry!(main);

fn main(args: Args) -> i32 {
    let mut status = 0;
    for p in args.iter().skip(1) {
        let r = ulib::unlink(p);
        if r < 0 {
            eprintln!("rm: {}: {}", p, Errno(-r));
            status = 1;
        }
    }
    status
}
