//! Identity-mapped MMU setup.
//!
//! The MMU is enabled from assembly on every core before any Rust code runs,
//! using statically initialised translation tables (they live in `.data`, so
//! no code has to build them with the MMU and caches off).
//!
//! Translation regime: TTBR0_EL1 only, 4 KiB granule, 39-bit VA (T0SZ = 25),
//! so the walk starts at level 1 with 1 GiB entries. The first GiB is
//! described by a level-2 table with 2 MiB blocks, because on the BCM2837 RAM
//! and peripherals share it.
//!
//! Memory types (MAIR_EL1):
//! - index 0: Device-nGnRnE (MMIO)
//! - index 1: Normal, inner/outer write-back read/write-allocate (RAM)
//!
//! Every kernel mapping is EL1-only (AP\[2:1\] = 00) and never executable
//! at EL0 (UXN), and global (nG = 0), so it is shared by all address spaces.
//!
//! # User address spaces
//!
//! A user process gets its own level-1 table ([`AddressSpace`]): a copy of
//! the kernel's level-1 entries (the level-2 table of the first GiB is shared
//! by pointer, not copied) plus entry 256, the user window
//! [`USER_BASE`]..[`USER_END`] (VA 256 GiB to 257 GiB), mapped with 4 KiB
//! pages through a private level-2 and level-3 tables. User pages are nG,
//! tagged with the address space's 8-bit ASID, and always PXN. Switching
//! address spaces is a single TTBR0_EL1 write (table + ASID); no TLB flush is
//! needed. Unmapping invalidates with inner-shareable `tlbi`, so all cores
//! stay coherent, and an ASID is flushed from every core before it is
//! reused.
//!
//! RAM is mapped Normal + Inner Shareable, which is what makes the
//! load/store-exclusive instructions behind Rust atomics work on real
//! Cortex-A53 hardware: with the MMU off every access is Device memory and
//! the BCM2837 has no global exclusive monitor, so atomics would never
//! succeed.

use crate::mem::frame;
use crate::platform::memory_map::{L1_ENTRIES, L2_ENTRIES};
use crate::sync::IrqSpinLock;
use core::arch::asm;

/// One 4 KiB translation table.
#[repr(C, align(4096))]
pub struct PageTable(pub [u64; 512]);

/// Level-1 table. Entry 0 is patched at boot to point at [`BOOT_L2`].
#[no_mangle]
pub static mut BOOT_L1: PageTable = PageTable(L1_ENTRIES);

/// Level-2 table describing the first GiB in 2 MiB blocks.
#[no_mangle]
pub static mut BOOT_L2: PageTable = PageTable(L2_ENTRIES);

const VALID_BLOCK: u64 = 0b01;
const VALID_TABLE: u64 = 0b11;
const VALID_PAGE: u64 = 0b11;
const ATTR_DEVICE: u64 = 0 << 2;
const ATTR_NORMAL: u64 = 1 << 2;
/// AP[2:1] = 00: read/write at EL1, no access at EL0.
const AP_EL1_RW: u64 = 0b00 << 6;
/// AP[2:1] = 01: read/write at EL1 and EL0.
const AP_EL0_RW: u64 = 0b01 << 6;
/// AP[2:1] = 11: read-only at EL1 and EL0.
const AP_EL0_RO: u64 = 0b11 << 6;
const SH_INNER: u64 = 0b11 << 8;
const AF: u64 = 1 << 10;
const NG: u64 = 1 << 11;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
/// Table descriptor: no privileged execution anywhere below.
const PXN_TABLE: u64 = 1 << 59;
const ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;

/// Block descriptor for cacheable RAM: EL1 only, never executable at EL0.
pub const fn normal_block(addr: u64) -> u64 {
    addr | VALID_BLOCK | ATTR_NORMAL | SH_INNER | AF | AP_EL1_RW | UXN
}

/// Block descriptor for MMIO (EL1 only, never executable).
pub const fn device_block(addr: u64) -> u64 {
    addr | VALID_BLOCK | ATTR_DEVICE | AF | AP_EL1_RW | PXN | UXN
}

/// MAIR_EL1: attr0 = Device-nGnRnE, attr1 = Normal WB RW-allocate.
pub const MAIR: u64 = 0x0000_ff00;

/// TCR_EL1: T0SZ=25, IRGN0/ORGN0 = WB-WA, SH0 = inner, TG0 = 4K, EPD1 = 1,
/// IPS = 32-bit. A1 = 0 (the ASID comes from TTBR0), AS = 0 (8-bit ASIDs).
pub const TCR: u64 = 25 | (1 << 8) | (1 << 10) | (3 << 12) | (1 << 23);

/// SCTLR_EL1 with the MMU, data cache and instruction cache enabled
/// (0x30D00800 are the ARMv8.0 RES1 bits). EL0 may not use cache
/// maintenance, DC ZVA, CTR_EL0, WFI/WFE or DAIF (UCI, DZE, UCT, nTWI, nTWE
/// and UMA are all 0), and alignment checking is off.
pub const SCTLR_MMU_ON: u64 = 0x30D0_0800 | (1 << 12) | (1 << 2) | 1;

/// SCTLR_EL1 value used before the MMU is enabled.
pub const SCTLR_MMU_OFF: u64 = 0x30D0_0800;

/// TTBR0_EL1 value of the kernel's own tables (ASID 0).
pub fn kernel_ttbr0() -> u64 {
    core::ptr::addr_of!(BOOT_L1) as u64
}

/// Load TTBR0_EL1 (translation table base and ASID).
///
/// # Safety
///
/// The tables must map the running kernel code, data and stacks exactly
/// like the boot tables.
#[inline]
pub unsafe fn set_ttbr0(value: u64) {
    // SAFETY: forwarded to the caller. The DSB makes table updates made by
    // this core visible to the walker first.
    unsafe { asm!("dsb ishst", "msr ttbr0_el1, {}", "isb", in(reg) value, options(nostack, preserves_flags)) };
}

// ---------------------------------------------------------------------------
// TLB and cache maintenance
// ---------------------------------------------------------------------------

/// Invalidate every TLB entry of `asid` on all cores (inner shareable).
pub fn tlb_flush_asid(asid: u8) {
    // SAFETY: TLB maintenance has no effect beyond dropping cached
    // translations.
    unsafe {
        asm!("dsb ishst", "tlbi aside1is, {}", "dsb ish", "isb",
             in(reg) (asid as u64) << 48, options(nostack, preserves_flags))
    };
}

/// Invalidate the translation of one user page of `asid` on all cores.
pub fn tlb_flush_page(asid: u8, va: u64) {
    let arg = ((asid as u64) << 48) | ((va >> 12) & 0x0000_0fff_ffff_ffff);
    // SAFETY: as above.
    unsafe { asm!("dsb ishst", "tlbi vae1is, {}", "dsb ish", "isb", in(reg) arg, options(nostack, preserves_flags)) };
}

/// Make instructions written through the kernel's identity map at
/// `start..start + len` visible to instruction fetch on every core: clean
/// the data cache to the point of unification, then invalidate all
/// instruction caches (inner shareable). Needed after loading user code.
pub fn sync_icache(start: usize, len: usize) {
    let ctr: u64;
    // SAFETY: reading CTR_EL0 has no side effects.
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    let line = 4usize << ((ctr >> 16) & 0xf);
    let mut a = start & !(line - 1);
    while a < start + len {
        // SAFETY: cleaning a mapped line is harmless.
        unsafe { asm!("dc cvau, {}", in(reg) a, options(nostack, preserves_flags)) };
        a += line;
    }
    // SAFETY: barriers and I-cache invalidation only.
    unsafe { asm!("dsb ish", "ic ialluis", "dsb ish", "isb", options(nostack, preserves_flags)) };
}

// ---------------------------------------------------------------------------
// ASIDs
// ---------------------------------------------------------------------------

/// Number of ASIDs available to user address spaces (1-255; 0 is the
/// kernel's).
pub const USER_ASIDS: usize = 255;

struct Asids {
    used: [u64; 4],
    free: usize,
    next: usize,
}

static ASIDS: IrqSpinLock<Asids> = IrqSpinLock::new(Asids { used: [1, 0, 0, 0], free: USER_ASIDS, next: 1 });

fn asid_alloc() -> Option<u8> {
    let mut a = ASIDS.lock();
    if a.free == 0 {
        return None;
    }
    // Next fit, so a freed ASID is not reused right away.
    for i in 0..256 {
        let id = (a.next + i) % 256;
        if id != 0 && a.used[id / 64] & (1 << (id % 64)) == 0 {
            a.used[id / 64] |= 1 << (id % 64);
            a.free -= 1;
            a.next = id + 1;
            return Some(id as u8);
        }
    }
    None
}

fn asid_free(id: u8) {
    // No core may keep translations of the old owner once the ASID is
    // handed to someone else.
    tlb_flush_asid(id);
    let mut a = ASIDS.lock();
    let id = id as usize;
    debug_assert!(a.used[id / 64] & (1 << (id % 64)) != 0);
    a.used[id / 64] &= !(1 << (id % 64));
    a.free += 1;
}

/// ASIDs currently free (out of [`USER_ASIDS`]).
pub fn asids_free() -> usize {
    ASIDS.lock().free
}

// ---------------------------------------------------------------------------
// User address spaces
// ---------------------------------------------------------------------------

/// Level-1 index of the user window.
pub const USER_L1_INDEX: usize = 256;
/// Lowest user virtual address.
pub const USER_BASE: u64 = (USER_L1_INDEX as u64) << 30;
/// One past the highest user virtual address.
pub const USER_END: u64 = USER_BASE + (1 << 30);
/// Size of a user page.
pub const PAGE_SIZE: u64 = 4096;

/// Access rights of a user page. User pages are always readable at EL0
/// and never executable at EL1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagePerms {
    /// Writable from EL0.
    pub write: bool,
    /// Executable at EL0.
    pub exec: bool,
}

impl PagePerms {
    /// Read-only data.
    pub const R: Self = Self { write: false, exec: false };
    /// Read/write data.
    pub const RW: Self = Self { write: true, exec: false };
    /// Code.
    pub const RX: Self = Self { write: false, exec: true };
}

/// Errors from [`AddressSpace`] operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmError {
    /// No physical frame left.
    OutOfMemory,
    /// All [`USER_ASIDS`] ASIDs are in use.
    NoAsid,
    /// The address is outside the user window or not page aligned.
    BadAddress,
    /// The page is already mapped.
    AlreadyMapped,
    /// Writable and executable at once (W^X).
    WriteExec,
}

/// A user address space: a private level-1 table that shares the kernel's
/// mappings, plus the user window mapped with 4 KiB pages. Owns its ASID,
/// its tables and every frame mapped into it; dropping it returns all of
/// them.
pub struct AddressSpace {
    l1: usize,
    asid: u8,
    pages: usize,
}

fn table(pa: u64) -> *mut u64 {
    (pa & ADDR_MASK) as usize as *mut u64
}

impl AddressSpace {
    /// New address space with an empty user window.
    pub fn new() -> Result<Self, VmError> {
        let asid = asid_alloc().ok_or(VmError::NoAsid)?;
        let Some(l1) = frame::alloc_frame() else {
            asid_free(asid);
            return Err(VmError::OutOfMemory);
        };
        let Some(l2) = frame::alloc_frame() else {
            frame::free_frame(l1);
            asid_free(asid);
            return Err(VmError::OutOfMemory);
        };
        let src = core::ptr::addr_of!(BOOT_L1) as *const u64;
        let dst = l1 as *mut u64;
        // SAFETY: `l1` is a fresh, exclusively owned frame (identity
        // mapped); BOOT_L1 is only written once, at boot.
        unsafe {
            for i in 0..512 {
                let e = if i == USER_L1_INDEX { l2 as u64 | VALID_TABLE | PXN_TABLE } else { src.add(i).read_volatile() };
                dst.add(i).write_volatile(e);
            }
            asm!("dsb ishst", options(nostack, preserves_flags));
        }
        Ok(Self { l1, asid, pages: 0 })
    }

    /// TTBR0_EL1 value for this address space (table base and ASID).
    pub fn ttbr0(&self) -> u64 {
        self.l1 as u64 | ((self.asid as u64) << 48)
    }

    /// The address space's ASID.
    pub fn asid(&self) -> u8 {
        self.asid
    }

    /// Number of user pages mapped.
    pub fn mapped_pages(&self) -> usize {
        self.pages
    }

    fn l2(&self) -> *mut u64 {
        // SAFETY: entry 256 of our own L1 always holds our L2 table.
        table(unsafe { (self.l1 as *const u64).add(USER_L1_INDEX).read_volatile() })
    }

    /// Pointer to the level-3 entry for `va`, allocating the level-3 table
    /// if `alloc` is set.
    fn pte(&self, va: u64, alloc: bool) -> Result<Option<*mut u64>, VmError> {
        if !(USER_BASE..USER_END).contains(&va) {
            return Err(VmError::BadAddress);
        }
        let off = va - USER_BASE;
        let l2i = ((off >> 21) & 511) as usize;
        let l3i = ((off >> 12) & 511) as usize;
        // SAFETY: our tables are exclusively owned frames, identity mapped;
        // indices are < 512.
        unsafe {
            let l2e = self.l2().add(l2i);
            let mut e = l2e.read_volatile();
            if e & 1 == 0 {
                if !alloc {
                    return Ok(None);
                }
                let l3 = frame::alloc_frame().ok_or(VmError::OutOfMemory)?;
                e = l3 as u64 | VALID_TABLE;
                l2e.write_volatile(e);
            }
            Ok(Some(table(e).add(l3i)))
        }
    }

    /// Map a fresh zeroed frame at user address `va` (page aligned).
    /// Returns the frame's physical address, which the kernel can access
    /// directly through its identity map.
    pub fn map(&mut self, va: u64, perms: PagePerms) -> Result<usize, VmError> {
        if perms.write && perms.exec {
            return Err(VmError::WriteExec);
        }
        if !va.is_multiple_of(PAGE_SIZE) {
            return Err(VmError::BadAddress);
        }
        let pte = self.pte(va, true)?.ok_or(VmError::OutOfMemory)?;
        // SAFETY: `pte` points into one of our level-3 tables.
        if unsafe { pte.read_volatile() } & 1 != 0 {
            return Err(VmError::AlreadyMapped);
        }
        let pa = frame::alloc_frame().ok_or(VmError::OutOfMemory)?;
        let desc = pa as u64
            | VALID_PAGE
            | ATTR_NORMAL
            | SH_INNER
            | AF
            | NG
            | PXN
            | if perms.write { AP_EL0_RW } else { AP_EL0_RO }
            | if perms.exec { 0 } else { UXN };
        // SAFETY: as above. The entry was invalid, so no TLB holds it
        // (translation faults are never cached): a barrier is enough.
        unsafe {
            pte.write_volatile(desc);
            asm!("dsb ishst", options(nostack, preserves_flags));
        }
        self.pages += 1;
        Ok(pa)
    }

    /// Unmap the page at `va` and free its frame. Returns whether a page
    /// was mapped there.
    pub fn unmap(&mut self, va: u64) -> bool {
        let va = va & !(PAGE_SIZE - 1);
        let Ok(Some(pte)) = self.pte(va, false) else { return false };
        // SAFETY: `pte` points into one of our level-3 tables.
        let e = unsafe { pte.read_volatile() };
        if e & 1 == 0 {
            return false;
        }
        // SAFETY: as above.
        unsafe { pte.write_volatile(0) };
        // Every core must drop the translation before the frame is reused.
        tlb_flush_page(self.asid, va);
        frame::free_frame((e & ADDR_MASK) as usize);
        self.pages -= 1;
        true
    }

    /// Physical address and rights of the page mapped at `va`, if any.
    pub fn translate(&self, va: u64) -> Option<(usize, PagePerms)> {
        let pte = self.pte(va & !(PAGE_SIZE - 1), false).ok()??;
        // SAFETY: `pte` points into one of our level-3 tables.
        let e = unsafe { pte.read_volatile() };
        if e & 1 == 0 {
            return None;
        }
        let perms = PagePerms { write: e & (0b11 << 6) == AP_EL0_RW, exec: e & UXN == 0 };
        Some(((e & ADDR_MASK) as usize + (va % PAGE_SIZE) as usize, perms))
    }
}

impl Drop for AddressSpace {
    /// Return every frame, the tables and the ASID.
    ///
    /// The address space must not be loaded in TTBR0_EL1 on any core (see
    /// [`crate::sched::set_address_space`]).
    fn drop(&mut self) {
        // Drop all cached translations of this ASID (and walk-cache entries
        // pointing into our tables) on every core before freeing anything.
        tlb_flush_asid(self.asid);
        let l2 = self.l2();
        for i in 0..512 {
            // SAFETY: our own tables, identity mapped, indices < 512.
            let e = unsafe { l2.add(i).read_volatile() };
            if e & 1 == 0 {
                continue;
            }
            let l3 = table(e);
            for j in 0..512 {
                // SAFETY: as above.
                let p = unsafe { l3.add(j).read_volatile() };
                if p & 1 != 0 {
                    frame::free_frame((p & ADDR_MASK) as usize);
                }
            }
            frame::free_frame(l3 as usize);
        }
        frame::free_frame(l2 as usize);
        frame::free_frame(self.l1);
        asid_free(self.asid);
    }
}
