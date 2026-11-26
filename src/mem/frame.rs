//! Physical frame allocator.
//!
//! Hands out 4 KiB frames of board RAM that nothing else uses: everything
//! from the end of the kernel image (which includes the boot stacks and the
//! heap, see the linker scripts) to the end of the RAM the board gives the
//! ARM cores ([`crate::platform::memory_map::RAM_END`]).
//!
//! Frames are accessed by the kernel through its identity map, so a
//! physical address is also a valid kernel pointer. Free frames form an
//! intrusive singly linked list (the link is stored in the free frame
//! itself); frames never handed out yet are taken from a bump pointer, so
//! boot does not have to touch the whole pool. Allocated frames are zeroed.

use crate::platform::memory_map::{RAM_END, RAM_START};
use crate::sync::IrqSpinLock;

/// Size of a frame.
pub const FRAME_SIZE: usize = 4096;

struct Pool {
    start: usize,
    end: usize,
    bump: usize,
    free_list: usize,
    free: usize,
}

static POOL: IrqSpinLock<Pool> = IrqSpinLock::new(Pool { start: 0, end: 0, bump: 0, free_list: 0, free: 0 });

/// Frame allocator counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStats {
    /// Frames managed by the allocator.
    pub total: usize,
    /// Frames currently free.
    pub free: usize,
}

/// Hand the RAM after the kernel image to the allocator. Called once at boot.
pub(crate) fn init() {
    let (_, kernel_end) = crate::arch::boot::kernel_region();
    // RAM_START is 0 on the Pi, so `max` is a no-op there.
    #[allow(clippy::unnecessary_min_or_max)]
    let start = kernel_end.max(RAM_START).next_multiple_of(FRAME_SIZE);
    let end = RAM_END & !(FRAME_SIZE - 1);
    let mut p = POOL.lock();
    p.start = start;
    p.end = end.max(start);
    p.bump = start;
    p.free_list = 0;
    p.free = (p.end - start) / FRAME_SIZE;
}

/// Allocate one zeroed frame. Returns its physical address.
pub fn alloc_frame() -> Option<usize> {
    let pa = {
        let mut p = POOL.lock();
        let pa = if p.free_list != 0 {
            let pa = p.free_list;
            // SAFETY: a free frame holds the address of the next free frame.
            p.free_list = unsafe { (pa as *const usize).read() };
            pa
        } else if p.bump < p.end {
            let pa = p.bump;
            p.bump += FRAME_SIZE;
            pa
        } else {
            return None;
        };
        p.free -= 1;
        pa
    };
    // SAFETY: the frame is ours now and identity mapped as normal memory.
    unsafe { core::ptr::write_bytes(pa as *mut u8, 0, FRAME_SIZE) };
    Some(pa)
}

/// Return a frame obtained from [`alloc_frame`].
pub fn free_frame(pa: usize) {
    let mut p = POOL.lock();
    assert!(pa.is_multiple_of(FRAME_SIZE) && pa >= p.start && pa < p.bump, "free_frame: bad frame {:#x}", pa);
    // SAFETY: the frame was allocated by us and is no longer used by its
    // owner; the first word becomes the free-list link.
    unsafe { (pa as *mut usize).write(p.free_list) };
    p.free_list = pa;
    p.free += 1;
}

/// Current counters.
pub fn frame_stats() -> FrameStats {
    let p = POOL.lock();
    FrameStats { total: (p.end - p.start) / FRAME_SIZE, free: p.free }
}
