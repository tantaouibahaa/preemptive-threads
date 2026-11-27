//! The initial ramdisk: a cpio archive (the "newc" format) embedded in the
//! kernel image and unpacked into the ramfs at boot.
//!
//! `build.rs` generates the archive from the user programs (`/bin/*`) and
//! `user/rootfs/` (`/etc/motd`, `/tmp`, ...). The test kernel also unpacks
//! [`TEST_ARCHIVE`], which holds the kernel-test programs under `/test`.
//!
//! newc entry: a 110-byte ASCII header (`070701` and 13 eight-digit hex
//! fields: ino, mode, uid, gid, nlink, mtime, filesize, devmajor, devminor,
//! rdevmajor, rdevminor, namesize, check), the NUL-terminated name padded
//! to 4 bytes, then the data padded to 4 bytes. `TRAILER!!!` ends it.

use crate::errno::Errno;
use crate::vfs;
use alloc::format;
use alloc::string::String;

include!(concat!(env!("OUT_DIR"), "/initrd.rs"));

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

/// One archive member.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    /// Path inside the archive (no leading `/` or `./`).
    pub name: &'a str,
    /// File mode (type and permission bits).
    pub mode: u32,
    /// Contents.
    pub data: &'a [u8],
}

fn hex(field: &[u8]) -> Result<u32, Errno> {
    let s = core::str::from_utf8(field).map_err(|_| Errno::EINVAL)?;
    u32::from_str_radix(s, 16).map_err(|_| Errno::EINVAL)
}

const fn align4(x: usize) -> usize {
    (x + 3) & !3
}

/// Iterate over the members of a newc archive.
pub fn entries(archive: &[u8]) -> impl Iterator<Item = Result<Entry<'_>, Errno>> {
    let mut off = 0usize;
    let mut done = false;
    core::iter::from_fn(move || {
        if done {
            return None;
        }
        let r = (|| {
            let h = archive.get(off..off + 110).ok_or(Errno::EINVAL)?;
            if &h[..6] != b"070701" {
                return Err(Errno::EINVAL);
            }
            let field = |i: usize| hex(&h[6 + 8 * i..6 + 8 * (i + 1)]);
            let mode = field(1)?;
            let size = field(6)? as usize;
            let namesize = field(11)? as usize;
            let name_start = off + 110;
            let name = archive.get(name_start..name_start + namesize.saturating_sub(1)).ok_or(Errno::EINVAL)?;
            let name = core::str::from_utf8(name).map_err(|_| Errno::EINVAL)?;
            let data_start = align4(name_start + namesize);
            let data = archive.get(data_start..data_start + size).ok_or(Errno::EINVAL)?;
            off = align4(data_start + size);
            let name = name.trim_start_matches("./").trim_start_matches('/');
            Ok(Entry { name, mode, data })
        })();
        match r {
            Ok(e) if e.name == "TRAILER!!!" => None,
            Ok(e) => Some(Ok(e)),
            Err(e) => {
                done = true;
                Some(Err(e))
            }
        }
    })
}

/// Unpack `archive` into the ramfs. Returns the number of files.
pub fn unpack(archive: &[u8]) -> Result<usize, Errno> {
    let mut files = 0;
    for e in entries(archive) {
        let e = e?;
        if e.name.is_empty() || e.name == "." {
            continue;
        }
        let path: String = format!("/{}", e.name);
        match e.mode & S_IFMT {
            S_IFDIR => {
                vfs::mkdir_all(&path)?;
            }
            S_IFREG => {
                if let Some(i) = path.rfind('/').filter(|&i| i > 0) {
                    vfs::mkdir_all(&path[..i])?;
                }
                vfs::write_file(&path, e.data)?;
                files += 1;
            }
            _ => {} // no symlinks or devices in our archives
        }
    }
    Ok(files)
}

/// Unpack the built-in archive. Returns the number of files.
pub fn mount() -> Result<usize, Errno> {
    unpack(ARCHIVE)
}
