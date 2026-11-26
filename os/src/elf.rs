//! Static ELF64 loader.
//!
//! Accepts little-endian AArch64 `ET_EXEC` images. Every `PT_LOAD` segment
//! is mapped into the process's user window with fresh pages and the
//! segment's permissions; a segment that is both writable and executable
//! is rejected (W^X), as are segments that overlap, leave the image area or
//! point outside the file. Code pages are made visible to instruction fetch
//! on every core before the program runs. The heap starts at the first page
//! after the highest segment.

use crate::errno::Errno;
use crate::mm::{Mm, VmaKind, HEAP_LIMIT, IMAGE_MIN};
use preemptive_threads::arch::mmu::{self, PagePerms, PAGE_SIZE};

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const EM_AARCH64: u16 = 183;
const ET_EXEC: u16 = 2;
const MAX_PHDRS: usize = 16;

fn rd<const N: usize>(d: &[u8], off: usize) -> Result<[u8; N], Errno> {
    d.get(off..off.checked_add(N).ok_or(Errno::ENOEXEC)?)
        .and_then(|s| s.try_into().ok())
        .ok_or(Errno::ENOEXEC)
}
fn u16_at(d: &[u8], off: usize) -> Result<u16, Errno> {
    rd::<2>(d, off).map(u16::from_le_bytes)
}
fn u32_at(d: &[u8], off: usize) -> Result<u32, Errno> {
    rd::<4>(d, off).map(u32::from_le_bytes)
}
fn u64_at(d: &[u8], off: usize) -> Result<u64, Errno> {
    rd::<8>(d, off).map(u64::from_le_bytes)
}

/// Load `elf` into `mm`. Returns the entry point.
pub fn load(mm: &mut Mm, elf: &[u8]) -> Result<u64, Errno> {
    if elf.len() < 64 || elf[0..4] != *b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        return Err(Errno::ENOEXEC);
    }
    if u16_at(elf, 16)? != ET_EXEC || u16_at(elf, 18)? != EM_AARCH64 {
        return Err(Errno::ENOEXEC);
    }
    let entry = u64_at(elf, 24)?;
    let phoff = u64_at(elf, 32)? as usize;
    let phentsize = u16_at(elf, 54)? as usize;
    let phnum = u16_at(elf, 56)? as usize;
    if phentsize != 56 || phnum > MAX_PHDRS {
        return Err(Errno::ENOEXEC);
    }
    let mut image_end = 0u64;
    let mut entry_ok = false;
    for i in 0..phnum {
        let ph = phoff.checked_add(i * phentsize).ok_or(Errno::ENOEXEC)?;
        if u32_at(elf, ph)? != PT_LOAD {
            continue;
        }
        let flags = u32_at(elf, ph + 4)?;
        let offset = u64_at(elf, ph + 8)? as usize;
        let vaddr = u64_at(elf, ph + 16)?;
        let filesz = u64_at(elf, ph + 32)? as usize;
        let memsz = u64_at(elf, ph + 40)?;
        if memsz == 0 {
            continue;
        }
        if flags & PF_W != 0 && flags & PF_X != 0 {
            return Err(Errno::ENOEXEC); // W^X
        }
        let end = vaddr.checked_add(memsz).ok_or(Errno::ENOEXEC)?;
        let (start, end_page) = (vaddr & !(PAGE_SIZE - 1), end.checked_add(PAGE_SIZE - 1).ok_or(Errno::ENOEXEC)? & !(PAGE_SIZE - 1));
        if start < IMAGE_MIN || end_page > HEAP_LIMIT || filesz as u64 > memsz {
            return Err(Errno::ENOEXEC);
        }
        let file = elf.get(offset..offset.checked_add(filesz).ok_or(Errno::ENOEXEC)?).ok_or(Errno::ENOEXEC)?;
        if mm.overlaps(start, end_page) {
            return Err(Errno::ENOEXEC);
        }
        let perms = PagePerms { write: flags & PF_W != 0, exec: flags & PF_X != 0 };
        mm.map_region(start, end_page, perms, VmaKind::Image)?;
        mm.write_unchecked(vaddr, file)?;
        if perms.exec {
            let mut va = start;
            while va < end_page {
                let pa = mm.translate(va).ok_or(Errno::EFAULT)?;
                mmu::sync_icache(pa, PAGE_SIZE as usize);
                va += PAGE_SIZE;
            }
            entry_ok |= (vaddr..end).contains(&entry);
        }
        image_end = image_end.max(end_page);
    }
    if !entry_ok {
        return Err(Errno::ENOEXEC);
    }
    mm.init_heap(image_end);
    Ok(entry)
}
