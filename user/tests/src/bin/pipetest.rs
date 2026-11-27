//! Pipe checks, run by the OS test kernel. Exits 0 on success or the
//! number of the failed check.
//!
//! - `pipetest`: EOF once every writer is gone, EPIPE once every reader is
//!   gone, in one process and across processes;
//! - `pipetest writer`: write to stdout until EPIPE (exit 0) or another
//!   error (exit 2);
//! - `pipetest block`: write to stdout forever (blocks on a full pipe).
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::{errno, File, SpawnAttr};

ulib::entry!(main);

macro_rules! check {
    ($n:expr, $cond:expr) => {
        if !$cond {
            eprintln!("pipetest: check {} failed: {}", $n, stringify!($cond));
            return $n;
        }
    };
}

fn writer() -> i32 {
    let chunk = [b'x'; 1024];
    let mut total = 0usize;
    loop {
        let r = ulib::write(1, &chunk);
        if r == -errno::EPIPE {
            eprintln!("pipetest writer: EPIPE after {} bytes", total);
            return 0;
        }
        if r <= 0 {
            return 2;
        }
        total += r as usize;
    }
}

fn main(args: Args) -> i32 {
    match args.str(1) {
        Some("writer") => return writer(),
        Some("block") => loop {
            if ulib::write(1, &[b'y'; 1024]) < 0 {
                return 3;
            }
        },
        _ => {}
    }
    // In one process: data, then EOF after the write end closes.
    let mut p = [0i32; 2];
    check!(1, ulib::pipe(&mut p) == 0);
    check!(2, ulib::write(p[1] as u64, b"abc") == 3);
    check!(3, ulib::input_available(p[0] as u64) == 3);
    check!(4, ulib::close(p[1] as u64) == 0);
    let mut buf = [0u8; 16];
    check!(5, ulib::read(p[0] as u64, &mut buf) == 3 && &buf[..3] == b"abc");
    check!(6, ulib::read(p[0] as u64, &mut buf) == 0);
    check!(7, ulib::close(p[0] as u64) == 0);
    // EPIPE once the read end is gone; wrong-direction access is EBADF.
    check!(8, ulib::pipe(&mut p) == 0);
    check!(9, ulib::read(p[1] as u64, &mut buf) == -errno::EBADF && ulib::write(p[0] as u64, b"x") == -errno::EBADF);
    check!(10, ulib::close(p[0] as u64) == 0);
    check!(11, ulib::write(p[1] as u64, b"x") == -errno::EPIPE);
    check!(12, ulib::close(p[1] as u64) == 0);
    // Across processes: echo writes, we read until EOF (echo exiting
    // closes the last write end).
    check!(13, ulib::pipe(&mut p) == 0);
    let attr = SpawnAttr::new().fd(1, p[1]).fd(2, 2);
    let echo = ulib::spawn_path("/bin/echo", &["echo", "through", "a", "pipe"], Some(&attr));
    check!(14, echo.is_ok());
    ulib::close(p[1] as u64);
    let text = File::from_fd(p[0]).read_to_string();
    check!(15, text.as_deref() == Ok("through a pipe\n"));
    check!(16, ulib::wait_pid(echo.unwrap()) == Ok(0));
    // A writer that fills the pipe (and blocks), then gets EPIPE when we
    // close our read end.
    check!(17, ulib::pipe(&mut p) == 0);
    let attr = SpawnAttr::new().fd(1, p[1]).fd(2, 2);
    let w = ulib::spawn_path("/test/pipetest", &["pipetest", "writer"], Some(&attr));
    check!(18, w.is_ok());
    ulib::close(p[1] as u64);
    check!(19, ulib::read(p[0] as u64, &mut buf) == 16);
    // The writer refills the pipe and blocks again.
    let mut tries = 0;
    while ulib::input_available(p[0] as u64) < 4096 && tries < 250 {
        ulib::sleep_ms(20);
        tries += 1;
    }
    check!(20, ulib::input_available(p[0] as u64) == 4096);
    check!(21, ulib::close(p[0] as u64) == 0);
    check!(22, ulib::wait_pid(w.unwrap()) == Ok(0));
    println!("pipetest ok: EOF after the last writer, EPIPE after the last reader, blocked writer woken with EPIPE");
    0
}
