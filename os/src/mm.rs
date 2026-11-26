//! A process's memory map: VMAs over an [`AddressSpace`], the heap
//! (`sbrk`), the initial stack, and checked copies between user and kernel
//! memory.
//!
//! The Cortex-A53 has no PAN, so nothing stops EL1 from dereferencing a user
//! pointer, and a raw dereference would run with whatever TTBR0 happens to
//! be loaded. The kernel therefore never dereferences user pointers: every
//! access goes through [`Mm::copy_from_user`] / [`Mm::copy_to_user`], which
//! check the whole range against the VMAs (and their write permission),
//! then translate page by page through the process's own tables and copy
//! through the kernel's identity map. That works from any thread, whatever
//! address space is loaded.
//!
//! ```text
//!  0x40_0000_0000  USER_BASE      unmapped (first MiB)
//!  0x40_0010_0000  IMAGE_MIN      program image: text (R-X), rodata (R--), data/bss (RW-)
//!                  brk_start      heap, grows up with sbrk (RW-)
//!                  ...            unmapped
//!  HEAP_LIMIT                     guard page
//!  STACK_BOTTOM                   stack, 64 KiB (RW-)
//!  0x41_0000_0000  STACK_TOP = USER_END
//! ```

use crate::errno::Errno;
use alloc::vec::Vec;
use preemptive_threads::arch::mmu::{AddressSpace, PagePerms, PAGE_SIZE, USER_BASE, USER_END};

/// Size of the user stack (mapped at spawn).
pub const STACK_SIZE: u64 = 64 * 1024;
/// Top of the user stack (exclusive).
pub const STACK_TOP: u64 = USER_END;
/// Lowest stack address.
pub const STACK_BOTTOM: u64 = STACK_TOP - STACK_SIZE;
/// The heap may grow up to here, leaving a guard page below the stack.
pub const HEAP_LIMIT: u64 = STACK_BOTTOM - PAGE_SIZE;
/// Lowest address a program may be loaded at.
pub const IMAGE_MIN: u64 = USER_BASE + 0x10_0000;
/// Most argument bytes `spawn` accepts (strings plus pointers).
pub const ARG_MAX: usize = 8 * 1024;

const fn page_up(a: u64) -> u64 {
    (a + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// What a VMA holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmaKind {
    /// A PT_LOAD segment of the program.
    Image,
    /// The sbrk heap.
    Heap,
    /// The user stack.
    Stack,
}

/// A mapped range of user addresses `[start, end)` (page aligned).
#[derive(Debug, Clone, Copy)]
pub struct Vma {
    /// First address.
    pub start: u64,
    /// One past the last address.
    pub end: u64,
    /// Access rights.
    pub perms: PagePerms,
    /// Contents.
    pub kind: VmaKind,
}

/// The memory of one process.
pub struct Mm {
    space: AddressSpace,
    vmas: Vec<Vma>,
    brk_start: u64,
    brk: u64,
}

impl Mm {
    /// Empty memory map with a fresh address space (and ASID).
    pub fn new() -> Result<Self, Errno> {
        Ok(Self { space: AddressSpace::new()?, vmas: Vec::new(), brk_start: 0, brk: 0 })
    }

    /// TTBR0_EL1 value of the address space.
    pub fn ttbr0(&self) -> u64 {
        self.space.ttbr0()
    }

    /// The address space's ASID.
    pub fn asid(&self) -> u8 {
        self.space.asid()
    }

    /// The VMAs, in creation order.
    pub fn vmas(&self) -> &[Vma] {
        &self.vmas
    }

    /// `(start, current break)` of the heap.
    pub fn heap(&self) -> (u64, u64) {
        (self.brk_start, self.brk)
    }

    /// Number of user pages mapped.
    pub fn mapped_pages(&self) -> usize {
        self.space.mapped_pages()
    }

    /// Physical address backing user address `va`, if mapped.
    pub fn translate(&self, va: u64) -> Option<usize> {
        self.space.translate(va).map(|(pa, _)| pa)
    }

    fn vma_at(&self, va: u64) -> Option<&Vma> {
        self.vmas.iter().find(|v| v.start <= va && va < v.end)
    }

    /// Whether `[start, end)` overlaps an existing VMA.
    pub(crate) fn overlaps(&self, start: u64, end: u64) -> bool {
        self.vmas.iter().any(|v| v.start < end && start < v.end)
    }

    /// Map `[start, end)` (page aligned) with fresh zeroed pages.
    pub(crate) fn map_region(&mut self, start: u64, end: u64, perms: PagePerms, kind: VmaKind) -> Result<(), Errno> {
        if !start.is_multiple_of(PAGE_SIZE) || !end.is_multiple_of(PAGE_SIZE) || start >= end || self.overlaps(start, end) {
            return Err(Errno::EINVAL);
        }
        let mut va = start;
        while va < end {
            if let Err(e) = self.space.map(va, perms) {
                while va > start {
                    va -= PAGE_SIZE;
                    self.space.unmap(va);
                }
                return Err(e.into());
            }
            va += PAGE_SIZE;
        }
        self.vmas.push(Vma { start, end, perms, kind });
        Ok(())
    }

    /// Check that `[va, va + len)` lies in VMAs (writable ones if `write`).
    pub fn check(&self, va: u64, len: usize, write: bool) -> Result<(), Errno> {
        if len == 0 {
            return Ok(());
        }
        let end = va.checked_add(len as u64).ok_or(Errno::EFAULT)?;
        let mut cur = va;
        while cur < end {
            let v = self.vma_at(cur).ok_or(Errno::EFAULT)?;
            if write && !v.perms.write {
                return Err(Errno::EFAULT);
            }
            cur = v.end;
        }
        Ok(())
    }

    /// Copy page by page through the identity map, with no VMA check.
    fn copy_raw(&self, va: u64, len: usize, mut f: impl FnMut(*mut u8, usize, usize)) -> Result<(), Errno> {
        let mut done = 0;
        while done < len {
            let cur = va + done as u64;
            let n = (len - done).min((PAGE_SIZE - cur % PAGE_SIZE) as usize);
            let pa = self.translate(cur).ok_or(Errno::EFAULT)?;
            f(pa as *mut u8, done, n);
            done += n;
        }
        Ok(())
    }

    /// Read user memory into `buf` (checked against the VMAs).
    pub fn copy_from_user(&self, va: u64, buf: &mut [u8]) -> Result<(), Errno> {
        self.check(va, buf.len(), false)?;
        let dst = buf.as_mut_ptr();
        // SAFETY: `pa..pa+n` is a mapped user frame (identity mapped in the
        // kernel); `dst+off..+n` is within `buf`.
        self.copy_raw(va, buf.len(), |pa, off, n| unsafe { core::ptr::copy_nonoverlapping(pa, dst.add(off), n) })
    }

    /// Write `data` to user memory (checked against writable VMAs).
    pub fn copy_to_user(&self, va: u64, data: &[u8]) -> Result<(), Errno> {
        self.check(va, data.len(), true)?;
        self.write_unchecked(va, data)
    }

    /// Write `data` to mapped user memory ignoring the page permissions
    /// (the loader writes code into read-only pages).
    pub(crate) fn write_unchecked(&self, va: u64, data: &[u8]) -> Result<(), Errno> {
        let src = data.as_ptr();
        // SAFETY: as in `copy_from_user`, with the roles swapped.
        self.copy_raw(va, data.len(), |pa, off, n| unsafe { core::ptr::copy_nonoverlapping(src.add(off), pa, n) })
    }

    /// Read a user `u64`.
    pub fn read_u64(&self, va: u64) -> Result<u64, Errno> {
        let mut b = [0u8; 8];
        self.copy_from_user(va, &mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    /// Read a NUL-terminated user string of at most `max` bytes (without
    /// the NUL).
    pub fn read_cstr(&self, va: u64, max: usize) -> Result<Vec<u8>, Errno> {
        let mut out = Vec::new();
        let mut cur = va;
        loop {
            let n = (PAGE_SIZE - cur % PAGE_SIZE) as usize;
            let mut chunk = [0u8; 256];
            let n = n.min(chunk.len());
            self.copy_from_user(cur, &mut chunk[..n])?;
            if let Some(z) = chunk[..n].iter().position(|&b| b == 0) {
                out.extend_from_slice(&chunk[..z]);
                return if out.len() > max { Err(Errno::ENAMETOOLONG) } else { Ok(out) };
            }
            out.extend_from_slice(&chunk[..n]);
            if out.len() > max {
                return Err(Errno::ENAMETOOLONG);
            }
            cur = cur.checked_add(n as u64).ok_or(Errno::EFAULT)?;
        }
    }

    /// Start the (empty) heap at `start`.
    pub(crate) fn init_heap(&mut self, start: u64) {
        self.brk_start = start;
        self.brk = start;
        self.vmas.push(Vma { start, end: start, perms: PagePerms::RW, kind: VmaKind::Heap });
    }

    /// Move the break by `incr` bytes. Returns the old break. Pages are
    /// mapped (zeroed) or unmapped and freed as the break crosses page
    /// boundaries.
    pub fn sbrk(&mut self, incr: i64) -> Result<u64, Errno> {
        let old = self.brk;
        let new = if incr >= 0 { old.checked_add(incr as u64) } else { old.checked_sub(incr.unsigned_abs()) }
            .ok_or(Errno::ENOMEM)?;
        if new < self.brk_start || new > HEAP_LIMIT {
            return Err(Errno::ENOMEM);
        }
        let (old_top, new_top) = (page_up(old), page_up(new));
        let mut va = old_top;
        while va < new_top {
            if let Err(e) = self.space.map(va, PagePerms::RW) {
                while va > old_top {
                    va -= PAGE_SIZE;
                    self.space.unmap(va);
                }
                return Err(e.into());
            }
            va += PAGE_SIZE;
        }
        let mut va = new_top;
        while va < old_top {
            self.space.unmap(va);
            va += PAGE_SIZE;
        }
        if let Some(h) = self.vmas.iter_mut().find(|v| v.kind == VmaKind::Heap) {
            h.end = new_top;
        }
        self.brk = new;
        Ok(old)
    }

    /// Map the stack and lay out the arguments on it, SysV style:
    ///
    /// ```text
    ///   STACK_TOP  argument strings (NUL-terminated)
    ///              padding to 16 bytes
    ///              0, 0           auxv (AT_NULL)
    ///              0              envp terminator
    ///              0              argv terminator
    ///              argv[argc-1] .. argv[0]
    ///   sp ->      argc
    /// ```
    ///
    /// Returns `(sp, argv)`; the program starts with x0 = argc, x1 = argv.
    pub(crate) fn setup_stack(&mut self, argv: &[&[u8]]) -> Result<(u64, u64), Errno> {
        let bytes: usize = argv.iter().map(|a| a.len() + 1 + 8).sum();
        if bytes + 64 > ARG_MAX {
            return Err(Errno::E2BIG);
        }
        self.map_region(STACK_BOTTOM, STACK_TOP, PagePerms::RW, VmaKind::Stack)?;
        let mut sp = STACK_TOP;
        let mut ptrs = Vec::with_capacity(argv.len());
        for a in argv {
            sp -= a.len() as u64 + 1;
            self.write_unchecked(sp, a)?;
            self.write_unchecked(sp + a.len() as u64, &[0])?;
            ptrs.push(sp);
        }
        sp &= !15;
        let words = 1 + argv.len() + 1 + 1 + 2;
        sp = (sp - 8 * words as u64) & !15;
        let mut block = Vec::with_capacity(words * 8);
        block.extend_from_slice(&(argv.len() as u64).to_le_bytes());
        for p in &ptrs {
            block.extend_from_slice(&p.to_le_bytes());
        }
        block.resize(words * 8, 0);
        self.write_unchecked(sp, &block)?;
        Ok((sp, sp + 8))
    }
}
