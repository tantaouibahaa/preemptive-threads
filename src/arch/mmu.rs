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
//! RAM is mapped Normal + Inner Shareable, which is what makes the
//! load/store-exclusive instructions behind Rust atomics work on real
//! Cortex-A53 hardware: with the MMU off every access is Device memory and
//! the BCM2837 has no global exclusive monitor, so atomics would never
//! succeed.

use crate::platform::memory_map::{L1_ENTRIES, L2_ENTRIES};

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
const ATTR_DEVICE: u64 = 0 << 2;
const ATTR_NORMAL: u64 = 1 << 2;
const SH_INNER: u64 = 0b11 << 8;
const AF: u64 = 1 << 10;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;

/// Block descriptor for cacheable RAM.
pub const fn normal_block(addr: u64) -> u64 {
    addr | VALID_BLOCK | ATTR_NORMAL | SH_INNER | AF
}

/// Block descriptor for MMIO (never executable).
pub const fn device_block(addr: u64) -> u64 {
    addr | VALID_BLOCK | ATTR_DEVICE | AF | PXN | UXN
}

/// MAIR_EL1: attr0 = Device-nGnRnE, attr1 = Normal WB RW-allocate.
pub const MAIR: u64 = 0x0000_ff00;

/// TCR_EL1: T0SZ=25, IRGN0/ORGN0 = WB-WA, SH0 = inner, TG0 = 4K, EPD1 = 1,
/// IPS = 32-bit.
pub const TCR: u64 = 25 | (1 << 8) | (1 << 10) | (3 << 12) | (1 << 23);

/// SCTLR_EL1 with the MMU, data cache and instruction cache enabled
/// (0x30D00800 are the ARMv8.0 RES1 bits).
pub const SCTLR_MMU_ON: u64 = 0x30D0_0800 | (1 << 12) | (1 << 2) | 1;

/// SCTLR_EL1 value used before the MMU is enabled.
pub const SCTLR_MMU_OFF: u64 = 0x30D0_0800;
