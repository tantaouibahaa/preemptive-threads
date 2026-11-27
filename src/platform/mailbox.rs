//! VideoCore property mailbox (BCM2835 mailbox 0, channel 8).
//!
//! The ARM asks the firmware for information by handing it a buffer of
//! "tags" through the mailbox registers at 0x3F00_B880:
//!
//! | register     | offset | use                                          |
//! |--------------|--------|----------------------------------------------|
//! | `MAIL0_READ` | 0x00   | responses from the VideoCore                 |
//! | `MAIL0_STA`  | 0x18   | bit 30: nothing to read                      |
//! | `MAIL1_WRITE`| 0x20   | requests to the VideoCore                    |
//! | `MAIL1_STA`  | 0x38   | bit 31: no room for a request                |
//!
//! A request is the buffer's bus address with the channel in the low 4 bits,
//! so the buffer is 16-byte aligned. The VideoCore reads and writes it
//! through its own bus, past the ARM's caches: the buffer gets a cache line
//! of its own, cleaned before the request and invalidated after the reply,
//! and is passed as its "uncached" bus alias (0xC000_0000 | address).
//! QEMU `raspi3b` implements the same interface.

use crate::arch::timer;
use core::ptr::{addr_of_mut, read_volatile, write_volatile};

const BASE: usize = 0x3f00_b880;
const MAIL0_READ: usize = BASE;
const MAIL0_STA: usize = BASE + 0x18;
const MAIL1_WRITE: usize = BASE + 0x20;
const MAIL1_STA: usize = BASE + 0x38;
const STA_FULL: u32 = 1 << 31;
const STA_EMPTY: u32 = 1 << 30;
const CHANNEL_PROPERTY: u32 = 8;
/// Bus address alias of ARM physical memory that bypasses the VideoCore's L2.
const BUS_UNCACHED: u32 = 0xc000_0000;
const REQUEST: u32 = 0;
const RESPONSE_OK: u32 = 0x8000_0000;

const TAG_FIRMWARE_REVISION: u32 = 0x0000_0001;
const TAG_BOARD_REVISION: u32 = 0x0001_0002;
const TAG_ARM_MEMORY: u32 = 0x0001_0005;

/// How long to wait for the firmware before giving up.
const TIMEOUT_MS: u64 = 100;

/// What the firmware reported.
#[derive(Debug, Clone, Copy)]
pub struct FirmwareInfo {
    /// Base of the ARM's memory (physical address).
    pub arm_base: u32,
    /// Size of the ARM's memory in bytes (the rest belongs to the GPU).
    pub arm_size: u32,
    /// Board revision code (e.g. 0x902120 for a Pi Zero 2 W).
    pub board_revision: u32,
    /// Firmware revision (a build timestamp).
    pub firmware_revision: u32,
}

/// Why a mailbox query failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxError {
    /// The request could not be posted in time (mailbox full).
    SendTimeout,
    /// No reply in time.
    ReplyTimeout,
    /// The firmware answered, but not with success for every tag.
    BadResponse(u32),
}

/// One cache line: the VideoCore must not see stale data in it, nor may a
/// write-back of something else sharing the line overwrite its reply.
#[repr(C, align(64))]
struct Buffer([u32; 16]);

static mut BUFFER: Buffer = Buffer([0; 16]);

fn rd(addr: usize) -> u32 {
    // SAFETY: mailbox MMIO, mapped as device memory.
    unsafe { read_volatile(addr as *const u32) }
}

fn wr(addr: usize, v: u32) {
    // SAFETY: mailbox MMIO, mapped as device memory.
    unsafe { write_volatile(addr as *mut u32, v) }
}

/// Clean and invalidate the buffer's cache line to the point of coherency.
fn flush(addr: usize) {
    // SAFETY: cache maintenance of our own buffer, then a full barrier that
    // also orders it against the mailbox accesses.
    unsafe { core::arch::asm!("dsb sy", "dc civac, {}", "dsb sy", in(reg) addr, options(nostack, preserves_flags)) };
}

/// Ask the firmware for the ARM memory range and the board and firmware
/// revisions. Boot core only, before the other cores run.
pub fn query() -> Result<FirmwareInfo, MailboxError> {
    let buf = addr_of_mut!(BUFFER) as *mut u32;
    let words: [u32; 16] = [
        16 * 4,
        REQUEST,
        TAG_ARM_MEMORY, 8, REQUEST, 0, 0,
        TAG_BOARD_REVISION, 4, REQUEST, 0,
        TAG_FIRMWARE_REVISION, 4, REQUEST, 0,
        0, // end tag
    ];
    // SAFETY: only the boot core uses the buffer, once, before the other
    // cores start; it is identity mapped.
    unsafe {
        for (i, w) in words.iter().enumerate() {
            write_volatile(buf.add(i), *w);
        }
    }
    flush(buf as usize);

    let deadline = timer::counter() + timer::ns_to_ticks(TIMEOUT_MS * 1_000_000);
    // Drop replies nobody collected (e.g. the firmware's own leftovers).
    while rd(MAIL0_STA) & STA_EMPTY == 0 && timer::counter() < deadline {
        let _ = rd(MAIL0_READ);
    }
    while rd(MAIL1_STA) & STA_FULL != 0 {
        if timer::counter() >= deadline {
            return Err(MailboxError::SendTimeout);
        }
        core::hint::spin_loop();
    }
    let msg = BUS_UNCACHED | buf as usize as u32 | CHANNEL_PROPERTY;
    wr(MAIL1_WRITE, msg);
    loop {
        if timer::counter() >= deadline {
            return Err(MailboxError::ReplyTimeout);
        }
        if rd(MAIL0_STA) & STA_EMPTY == 0 && rd(MAIL0_READ) == msg {
            break;
        }
        core::hint::spin_loop();
    }
    flush(buf as usize);
    // SAFETY: as above; the VideoCore is done with the buffer.
    let r = |i: usize| unsafe { read_volatile(buf.add(i)) };
    if r(1) != RESPONSE_OK {
        return Err(MailboxError::BadResponse(r(1)));
    }
    // Each tag's request/response code: bit 31 = answered.
    for i in [4, 9, 13] {
        if r(i) & RESPONSE_OK == 0 {
            return Err(MailboxError::BadResponse(r(i)));
        }
    }
    Ok(FirmwareInfo { arm_base: r(5), arm_size: r(6), board_revision: r(10), firmware_revision: r(14) })
}
