//! `sleep N[ms]`: sleep N seconds (or milliseconds with the `ms` suffix).
#![no_std]
#![no_main]

use ulib::prelude::*;

ulib::entry!(main);

fn main(args: Args) -> i32 {
    let arg = args.str(1).unwrap_or("1");
    let ms = match arg.strip_suffix("ms") {
        Some(v) => v.parse::<u64>().ok(),
        None => arg.parse::<u64>().ok().map(|s| s * 1000),
    };
    match ms {
        Some(ms) => {
            ulib::sleep_ms(ms);
            0
        }
        None => {
            eprintln!("usage: sleep N | Nms");
            2
        }
    }
}
