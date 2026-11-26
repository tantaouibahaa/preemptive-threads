//! `poke r|w|x <addr>`: read, write or jump to an address; `poke u`:
//! execute an undefined instruction. The kernel should kill us before we
//! print "survived".
#![no_std]
#![no_main]

use ut::{println, Args};

#[no_mangle]
fn ut_main(args: Args) -> i32 {
    let addr = args.get(2).and_then(ut::parse_u64).unwrap_or(0);
    match args.get(1) {
        // SAFETY (all arms): deliberately invalid accesses; the point of
        // this program is to be killed by the kernel.
        Some(b"r") => {
            let v = unsafe { core::ptr::read_volatile(addr as *const u64) };
            println!("poke: read {:#x} = {:#x} (survived)", addr, v);
        }
        Some(b"w") => {
            unsafe { core::ptr::write_volatile(addr as *mut u64, 0xbad0_bad0_bad0_bad0) };
            println!("poke: wrote {:#x} (survived)", addr);
        }
        Some(b"x") => {
            let f: extern "C" fn() = unsafe { core::mem::transmute(addr as usize) };
            f();
            println!("poke: called {:#x} (survived)", addr);
        }
        Some(b"u") => {
            unsafe { core::arch::asm!("udf #0") };
            println!("poke: udf (survived)");
        }
        _ => return 2,
    }
    0
}
