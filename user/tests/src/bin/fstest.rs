//! File system checks, run by the OS test kernel. Exits 0 on success or
//! the number of the failed check.
//!
//! - `fstest vfs`: create, write, append, truncate, read back, directories,
//!   `readdir`, relative paths, `unlink`, error cases, `/dev/null`;
//! - `fstest inherit`: fd inheritance across `spawn` (shared offsets),
//!   `dup2`, and a child given only mapped fds;
//! - `fstest child <fd>` / `fstest isolated <fd>`: the children.
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::{errno, File, Kind, SpawnAttr, O_APPEND, O_CREATE, O_READ, O_TRUNC, O_WRITE};

ulib::entry!(main);

macro_rules! check {
    ($n:expr, $cond:expr) => {
        if !$cond {
            eprintln!("fstest: check {} failed: {}", $n, stringify!($cond));
            return $n;
        }
    };
}

fn read_file(path: &str) -> Option<String> {
    File::read_only(path).ok()?.read_to_string().ok()
}

fn vfs() -> i32 {
    check!(1, File::create("/tmp/t1").and_then(|f| f.write_all(b"hello world\n")).is_ok());
    check!(2, read_file("/tmp/t1").as_deref() == Some("hello world\n"));
    check!(3, File::open("/tmp/t1", O_WRITE | O_APPEND).and_then(|f| f.write_all(b"more\n")).is_ok());
    check!(4, read_file("/tmp/t1").as_deref() == Some("hello world\nmore\n"));
    check!(5, ulib::stat("/tmp/t1").map(|s| s.size).ok() == Some(17));
    // Overwrite in place: a write-only open without truncate keeps the tail.
    check!(6, File::open("/tmp/t1", O_WRITE).and_then(|f| f.write_all(b"HELLO")).is_ok());
    check!(7, read_file("/tmp/t1").as_deref() == Some("HELLO world\nmore\n"));
    check!(8, File::open("/tmp/t1", O_WRITE | O_TRUNC).is_ok());
    check!(9, ulib::stat("/tmp/t1").map(|s| s.size).ok() == Some(0));
    check!(10, File::read_only("/tmp/nope").err() == Some(Errno(errno::ENOENT)));
    check!(11, ulib::mkdir("/tmp/d") == 0);
    check!(12, ulib::mkdir("/tmp/d") == -errno::EEXIST);
    check!(13, File::open("/tmp/d/f", O_WRITE | O_CREATE).and_then(|f| f.write_all(b"in d")).is_ok());
    let names = |p: &str| -> Vec<String> { ulib::read_dir(p).map(|v| v.into_iter().map(|e| e.name).collect()).unwrap_or_default() };
    check!(14, names("/tmp/d") == ["f"]);
    let tmp = ulib::read_dir("/tmp").unwrap_or_default();
    check!(15, tmp.iter().any(|e| e.name == "d" && e.kind == Kind::Dir) && tmp.iter().any(|e| e.name == "t1" && e.kind == Kind::File));
    // Relative paths and the working directory.
    check!(16, ulib::chdir("/tmp/d") == 0);
    check!(17, read_file("f").as_deref() == Some("in d"));
    check!(18, read_file("../d/./f").as_deref() == Some("in d"));
    let mut cwd = [0u8; 64];
    let n = ulib::getcwd(&mut cwd);
    check!(19, n > 0 && &cwd[..n as usize] == b"/tmp/d");
    check!(20, ulib::chdir("/") == 0);
    check!(21, ulib::unlink("/tmp/d") == -errno::ENOTEMPTY);
    check!(22, ulib::unlink("/tmp/d/f") == 0 && ulib::unlink("/tmp/d") == 0);
    check!(23, ulib::stat("/tmp/d").err() == Some(Errno(errno::ENOENT)));
    check!(24, File::open("/tmp", O_WRITE).err() == Some(Errno(errno::EISDIR)));
    check!(25, File::read_only("/bin/hello/x").err() == Some(Errno(errno::ENOTDIR)));
    // Access modes.
    let ro = File::read_only("/etc/motd").ok();
    check!(26, ro.as_ref().is_some_and(|f| ulib::write(f.fd() as u64, b"x") == -errno::EBADF));
    let wo = File::open("/tmp/t1", O_WRITE).ok();
    check!(27, wo.as_ref().is_some_and(|f| ulib::read(f.fd() as u64, &mut [0u8; 4]) == -errno::EBADF));
    check!(28, ulib::close(30) == -errno::EBADF);
    // The tree from the initrd, and devices.
    check!(29, names("/") == ["bin", "dev", "etc", "test", "tmp"]);
    check!(30, File::open("/dev/null", O_READ | O_WRITE).is_ok_and(|f| f.write_all(b"gone").is_ok() && f.read_to_end().ok() == Some(Vec::new())));
    check!(31, ulib::unlink("/tmp/t1") == 0);
    println!("fstest vfs ok: create/write/append/overwrite/truncate, mkdir/readdir/unlink, relative paths, EISDIR/ENOTDIR/EBADF/ENOTEMPTY");
    0
}

fn inherit() -> i32 {
    let Ok(f) = File::create("/tmp/inherit") else { return 1 };
    let fd = f.fd();
    check!(2, f.write_all(b"parent\n").is_ok());
    // No attributes: the child inherits every fd, sharing the open file
    // (and its offset).
    let arg = format!("{}", fd);
    let child = ulib::spawn_path("/test/fstest", &["fstest", "child", &arg], None);
    check!(3, child.is_ok());
    check!(4, ulib::wait_pid(child.unwrap()) == Ok(0));
    check!(5, f.write_all(b"again\n").is_ok());
    check!(6, read_file("/tmp/inherit").as_deref() == Some("parent\nchild\nagain\n"));
    // dup2: another fd for the same open file.
    check!(7, ulib::dup2(fd as u64, 9) == 9);
    check!(8, ulib::write(9, b"dup\n") == 4);
    check!(9, read_file("/tmp/inherit").as_deref() == Some("parent\nchild\nagain\ndup\n"));
    check!(10, ulib::close(9) == 0 && ulib::write(9, b"x") == -errno::EBADF);
    // With a mapping, the child only gets what it is given.
    let attr = SpawnAttr::new().fd(1, 1).fd(2, 2);
    let iso = ulib::spawn_path("/test/fstest", &["fstest", "isolated", &arg], Some(&attr));
    check!(11, iso.is_ok());
    check!(12, ulib::wait_pid(iso.unwrap()) == Ok(0));
    drop(f);
    check!(13, ulib::unlink("/tmp/inherit") == 0);
    println!("fstest inherit ok: child shared fd {} and its offset, dup2, mapped-only child", fd);
    0
}

fn main(args: Args) -> i32 {
    let fd = args.get(2).and_then(ulib::parse_u64).unwrap_or(3);
    match args.str(1) {
        Some("vfs") => vfs(),
        Some("inherit") => inherit(),
        Some("child") => {
            // fds 0-2 came along too.
            check!(40, ulib::write(1, b"") == 0 && ulib::write(2, b"") == 0);
            check!(41, ulib::write(fd, b"child\n") == 6);
            0
        }
        Some("isolated") => {
            check!(50, ulib::write(fd, b"x") == -errno::EBADF);
            check!(51, ulib::read(0, &mut [0u8; 1]) == -errno::EBADF);
            check!(52, ulib::write(1, b"") == 0);
            0
        }
        _ => 99,
    }
}
