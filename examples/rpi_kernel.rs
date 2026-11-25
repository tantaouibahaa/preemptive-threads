//! Demo kernel for the Raspberry Pi Zero 2 W (also runs on QEMU `raspi3b`,
//! and on QEMU `virt` with `--features qemu-virt`).
//!
//! - four unpinned worker threads count primes and report which core ran
//!   them, sharing a `Mutex`-protected total;
//! - a producer/consumer pair talks over a bounded channel;
//! - a monitor thread prints per-core statistics every second;
//! - on the Pi, a thread blinks the green ACT LED (GPIO 29) at 2 Hz, so
//!   you can see the scheduler running even without a serial cable.
//!
//! Build `kernel8.img` with `make kernel8.img` and see the README for how
//! to put it on an SD card.

#![no_std]
#![no_main]

use preemptive_threads::sync::{channel, Mutex};
use preemptive_threads::{self as pt, priority, println, thread, Builder, Config, Duration, Instant};

#[no_mangle]
fn kernel_main() -> ! {
    println!();
    println!("========================================");
    println!(" preemptive-threads {} demo", env!("CARGO_PKG_VERSION"));
    println!(" board: {}  (entered at EL{})", pt::platform::NAME, pt::boot_el());
    println!("========================================");
    pt::start(Config::default(), main)
}

static PRIMES: Mutex<u64> = Mutex::new(0);

fn is_prime(n: u64) -> bool {
    n >= 2 && (2..).take_while(|d| d * d <= n).all(|d| !n.is_multiple_of(d))
}

fn main() {
    println!("[main] {} cores online", pt::stats().cpus_online());

    #[cfg(not(feature = "qemu-virt"))]
    Builder::new().name("led").priority(priority::HIGH).spawn(led::blink).unwrap();

    Builder::new().name("monitor").priority(priority::HIGH).spawn(monitor).unwrap();

    let (tx, rx) = channel::<u64>(8);
    Builder::new().name("producer").spawn(move || {
        let mut n = 0u64;
        loop {
            tx.send(n).unwrap();
            n += 1;
            thread::sleep(Duration::from_millis(250));
        }
    }).unwrap();
    Builder::new().name("consumer").spawn(move || {
        while let Ok(n) = rx.recv() {
            if n % 8 == 0 {
                println!("[consumer] received message #{} on cpu{}", n, thread::current_cpu());
            }
        }
    }).unwrap();

    for id in 0..4u64 {
        Builder::new().name("worker").spawn(move || {
            let mut n = 1_000_000 * (id + 1);
            loop {
                let t = Instant::now();
                let mut found = 0;
                while t.elapsed() < Duration::from_secs(2) {
                    if is_prime(n) {
                        found += 1;
                    }
                    n += 1;
                }
                let total = {
                    let mut p = PRIMES.lock();
                    *p += found;
                    *p
                };
                println!(
                    "[worker {}] cpu{}: +{} primes (total {}), cores used so far {:#06b}",
                    id,
                    thread::current_cpu(),
                    found,
                    total,
                    thread::current().cpus_ran()
                );
            }
        }).unwrap();
    }
}

fn monitor() {
    loop {
        thread::sleep(Duration::from_secs(1));
        let s = pt::stats();
        println!(
            "[monitor] uptime {:>4}s  switches {:>7}  preemptions {:>5}  threads {}  heap {} KiB",
            pt::time::uptime().as_secs(),
            s.context_switches(),
            s.preemptions(),
            s.threads_live,
            s.heap.used / 1024
        );
    }
}

#[cfg(not(feature = "qemu-virt"))]
mod led {
    use core::ptr::write_volatile;
    use preemptive_threads::{thread, Duration};

    const GPFSEL2: usize = 0x3f20_0008;
    const GPSET0: usize = 0x3f20_001c;
    const GPCLR0: usize = 0x3f20_0028;
    const ACT_LED: u32 = 29; // Pi Zero 2 W activity LED

    pub fn blink() {
        // SAFETY: GPIO MMIO; only pin 29's function bits are changed.
        unsafe {
            let sel = core::ptr::read_volatile(GPFSEL2 as *const u32);
            write_volatile(GPFSEL2 as *mut u32, (sel & !(7 << 27)) | (1 << 27));
        }
        loop {
            // SAFETY: set/clear registers only affect pin 29.
            unsafe { write_volatile(GPSET0 as *mut u32, 1 << ACT_LED) };
            thread::sleep(Duration::from_millis(250));
            // SAFETY: as above.
            unsafe { write_volatile(GPCLR0 as *mut u32, 1 << ACT_LED) };
            thread::sleep(Duration::from_millis(250));
        }
    }
}
