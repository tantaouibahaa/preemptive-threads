//! `echo [-n] words...`: print the arguments.
#![no_std]
#![no_main]

use ulib::prelude::*;

ulib::entry!(main);

fn main(args: Args) -> i32 {
    let mut words: Vec<&str> = args.iter().skip(1).collect();
    let newline = words.first() != Some(&"-n");
    if !newline {
        words.remove(0);
    }
    let mut s = words.join(" ");
    if newline {
        s.push('\n');
    }
    print!("{}", s);
    0
}
