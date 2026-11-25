//! ARM semihosting, used by the emulator test kernels to report results.
//!
//! With the `semihosting` feature, [`exit`] issues `SYS_EXIT_EXTENDED`
//! (`HLT #0xF000`), which makes QEMU (run with `-semihosting`) terminate with
//! the given exit status. Without the feature it just halts the core, so the
//! same kernel is safe to run on real hardware, where a semihosting call
//! would trap.

/// Terminate the emulator with `code` (or halt without the feature).
pub fn exit(code: u32) -> ! {
    #[cfg(feature = "semihosting")]
    {
        const SYS_EXIT_EXTENDED: u64 = 0x20;
        const ADP_STOPPED_APPLICATION_EXIT: u64 = 0x20026;
        let block: [u64; 2] = [ADP_STOPPED_APPLICATION_EXIT, code as u64];
        // SAFETY: a semihosting request; QEMU exits and never returns.
        unsafe {
            core::arch::asm!(
                "hlt #0xf000",
                inout("x0") SYS_EXIT_EXTENDED => _,
                in("x1") block.as_ptr(),
                options(nostack)
            );
        }
    }
    let _ = code;
    super::halt()
}
