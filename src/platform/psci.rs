//! PSCI 0.2 calls (used to start secondary cores on QEMU `virt`).
//!
//! The conduit depends on where firmware lives: when the kernel was entered
//! at EL1, QEMU implements PSCI behind `HVC`; when it was entered at EL2
//! (`-M virt,virtualization=on`), behind `SMC`.

use core::sync::atomic::{AtomicBool, Ordering};

const CPU_ON_64: u64 = 0xc400_0003;

static USE_SMC: AtomicBool = AtomicBool::new(false);

/// Choose the conduit from the exception level the kernel was entered at.
pub fn set_conduit_from_boot_el(boot_el: u8) {
    USE_SMC.store(boot_el >= 2, Ordering::Relaxed);
}

fn call(fid: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    // SAFETY: PSCI calls follow the SMC calling convention; x0-x3 are
    // arguments and x0 the result. Other caller-saved registers may be
    // clobbered by firmware, so they are declared as clobbered.
    unsafe {
        if USE_SMC.load(Ordering::Relaxed) {
            core::arch::asm!("smc #0",
                inlateout("x0") fid as i64 => ret, inlateout("x1") a1 => _,
                inlateout("x2") a2 => _, inlateout("x3") a3 => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _,
                out("x8") _, out("x9") _, out("x10") _, out("x11") _,
                out("x12") _, out("x13") _, out("x14") _, out("x15") _,
                out("x16") _, out("x17") _, options(nostack));
        } else {
            core::arch::asm!("hvc #0",
                inlateout("x0") fid as i64 => ret, inlateout("x1") a1 => _,
                inlateout("x2") a2 => _, inlateout("x3") a3 => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _,
                out("x8") _, out("x9") _, out("x10") _, out("x11") _,
                out("x12") _, out("x13") _, out("x14") _, out("x15") _,
                out("x16") _, out("x17") _, options(nostack));
        }
    }
    ret
}

/// Power on core `mpidr` at physical address `entry`. Returns the PSCI
/// status code (0 = success).
pub fn cpu_on(mpidr: u64, entry: usize, context: u64) -> i64 {
    call(CPU_ON_64, mpidr, entry as u64, context)
}
