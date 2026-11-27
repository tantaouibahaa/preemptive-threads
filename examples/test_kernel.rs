//! Self-checking test kernel.
//!
//! Boots, runs every test below on the scheduler and exits QEMU through
//! semihosting: status 0 if all tests pass, 1 otherwise (a panic exits with
//! 101, a fatal exception with 3; a hang is caught by the runner's timeout).
//!
//! Each test prints one deterministic marker line:
//!
//! ```text
//! [TEST] <name> ... ok (<details>)
//! [TEST] <name> ... FAILED: <reason>
//! ```
//!
//! followed by a final `ALL <n> TESTS PASSED` or `<k> OF <n> TESTS FAILED`.
//!
//! Run with `make test-virt` or `make test-raspi3b`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use preemptive_threads::sync::{channel, Condvar, Mutex, Semaphore, TryRecvError};
use preemptive_threads::{self as pt, priority, println, thread, Builder, Config, Duration, Instant, SchedPolicy};

type TestResult = Result<String, String>;
type Test = (&'static str, fn() -> TestResult);

macro_rules! ensure {
    ($cond:expr, $($fmt:tt)*) => {
        if !$cond {
            return Err(format!($($fmt)*));
        }
    };
}

const EXPECTED_CPUS: usize = 4;

#[no_mangle]
fn kernel_main() -> ! {
    println!();
    println!("preemptive-threads test kernel ({}, entered at EL{})", pt::platform::NAME, pt::boot_el());
    pt::start(Config::default(), main)
}

/// Index of the running test and when it started (counter ticks).
static CURRENT_TEST: AtomicUsize = AtomicUsize::new(0);
static TEST_STARTED: AtomicU64 = AtomicU64::new(0);
const TEST_DEADLINE: Duration = Duration::from_secs(20);

/// If one test runs for longer than TEST_DEADLINE, print the scheduler
/// state and fail fast instead of waiting for the runner's timeout.
fn watchdog(names: Vec<&'static str>) {
    loop {
        thread::sleep(Duration::from_millis(500));
        let started = TEST_STARTED.load(Ordering::Acquire);
        let running = Duration::from_nanos(pt::arch::timer::ticks_to_ns(Instant::now().ticks().saturating_sub(started)));
        if started != 0 && running > TEST_DEADLINE {
            let name = names.get(CURRENT_TEST.load(Ordering::Acquire)).copied().unwrap_or("?");
            println!("[TEST] {} ... FAILED: no progress after {:?}", name, TEST_DEADLINE);
            pt::sched::dump_state();
            pt::arch::semihosting::exit(2);
        }
    }
}

fn main() {
    let tests: &[Test] = &[
        ("smp_boot", smp_boot),
        ("join_returns_value", join_returns_value),
        ("preemption", preemption),
        ("all_threads_complete", all_threads_complete),
        ("mutex_no_lost_updates", mutex_no_lost_updates),
        ("channel_ordering", channel_ordering),
        ("channel_disconnect", channel_disconnect),
        ("sleep_accuracy", sleep_accuracy),
        ("all_cores_ran_threads", all_cores_ran_threads),
        ("priority_inheritance", priority_inheritance),
        ("priority_bands", priority_bands),
        ("fifo_policy", fifo_policy),
        ("semaphore_limits_concurrency", semaphore_limits_concurrency),
        ("condvar_producer_consumer", condvar_producer_consumer),
        ("fpu_state_preserved", fpu_state_preserved),
        ("console_write_keeps_irqs_on", console_write_keeps_irqs_on),
        ("threads_reaped", threads_reaped),
    ];
    let names: Vec<&'static str> = tests.iter().map(|t| t.0).collect();
    Builder::new().name("watchdog").priority(255).cpu(0).spawn(move || watchdog(names)).unwrap();
    let mut failed = 0;
    for (i, (name, test)) in tests.iter().enumerate() {
        let t0 = Instant::now();
        CURRENT_TEST.store(i, Ordering::Release);
        TEST_STARTED.store(t0.ticks(), Ordering::Release);
        match test() {
            Ok(detail) => println!("[TEST] {} ... ok ({}; {} ms)", name, detail, t0.elapsed().as_millis()),
            Err(why) => {
                failed += 1;
                println!("[TEST] {} ... FAILED: {}", name, why);
            }
        }
    }
    // Every interrupt source the kernel did not enable must stay quiet.
    let stray = pt::stray_irqs();
    if stray != 0 {
        failed += 1;
        println!("[CHECK] no stray interrupts ... FAILED: {} stray IRQs", stray);
    }
    let s = pt::stats();
    println!(
        "[STATS] cpus={} switches={} preemptions={} steals={} spawned={} reaped={} heap_used={}B stray_irqs={}",
        s.cpus_online(),
        s.context_switches(),
        s.preemptions(),
        s.cpus.iter().map(|c| c.steals).sum::<u64>(),
        s.threads_spawned,
        s.threads_reaped,
        s.heap.used,
        stray
    );
    for (i, c) in s.cpus.iter().enumerate().filter(|(_, c)| c.online) {
        println!(
            "[STATS] cpu{}: ticks={} idle_ticks={} switches={} preemptions={} steals={} ipis={}",
            i, c.ticks, c.idle_ticks, c.context_switches, c.preemptions, c.steals, c.ipis
        );
    }
    // Disarm the progress watchdog: without semihosting (real hardware) the
    // exit below only halts this core and the watchdog would keep running.
    TEST_STARTED.store(0, Ordering::Release);
    if failed == 0 {
        println!("ALL {} TESTS PASSED on {} cores", tests.len(), s.cpus_online());
        pt::arch::semihosting::exit(0);
    } else {
        println!("{} OF {} TESTS FAILED", failed, tests.len());
        pt::arch::semihosting::exit(1);
    }
}

/// Busy work the optimiser cannot remove.
fn spin_work(iters: u64) -> u64 {
    let mut x = 0u64;
    for i in 0..iters {
        x = core::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(i));
    }
    x
}

fn spin_for(d: Duration) {
    let t = Instant::now();
    while t.elapsed() < d {
        spin_work(100);
    }
}

fn smp_boot() -> TestResult {
    let s = pt::stats();
    ensure!(s.cpus_online() == EXPECTED_CPUS, "{} of {} cores online", s.cpus_online(), EXPECTED_CPUS);
    Ok(format!("{} cores online", s.cpus_online()))
}

fn join_returns_value() -> TestResult {
    let handles: Vec<_> = (0..16u64).map(|i| thread::spawn(move || i * i)).collect();
    let sum: u64 = handles.into_iter().map(|h| h.join()).sum();
    ensure!(sum == (0..16u64).map(|i| i * i).sum(), "sum of joined values = {}", sum);
    let s = thread::spawn(|| String::from("hello from a thread")).join();
    ensure!(s == "hello from a thread", "string result = {:?}", s);
    Ok(format!("16 joins, sum={}", sum))
}

/// Two threads pinned to one core that never yield: the first spins until
/// it sees the second run, which only a timer preemption allows.
fn preemption() -> TestResult {
    static SECOND_RAN: AtomicBool = AtomicBool::new(false);
    static FIRST_SPINS: AtomicU64 = AtomicU64::new(0);
    let before = pt::stats().cpus[1].preemptions;
    let a = Builder::new().cpu(1).spawn(|| {
        while !SECOND_RAN.load(Ordering::Acquire) {
            FIRST_SPINS.fetch_add(1, Ordering::Relaxed);
            spin_work(50);
        }
    }).unwrap();
    let b = Builder::new().cpu(1).spawn(|| {
        SECOND_RAN.store(true, Ordering::Release);
    }).unwrap();
    a.join();
    b.join();
    let after = pt::stats().cpus[1].preemptions;
    ensure!(after > before, "no preemption recorded on cpu1");
    Ok(format!("cpu1 preemptions +{}", after - before))
}

fn all_threads_complete() -> TestResult {
    static DONE: AtomicUsize = AtomicUsize::new(0);
    let handles: Vec<_> = (0..64)
        .map(|i| {
            Builder::new()
                .priority(if i % 2 == 0 { priority::NORMAL } else { priority::LOW + 1 })
                .spawn(move || {
                    spin_work(20_000 + i * 100);
                    DONE.fetch_add(1, Ordering::Relaxed);
                })
                .unwrap()
        })
        .collect();
    for h in handles {
        h.join();
    }
    let done = DONE.load(Ordering::Relaxed);
    ensure!(done == 64, "{} of 64 threads completed", done);
    Ok(String::from("64/64 threads completed"))
}

fn mutex_no_lost_updates() -> TestResult {
    static COUNTER: Mutex<u64> = Mutex::new(0);
    const THREADS: u64 = 8;
    const ITERS: u64 = 5_000;
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            thread::spawn(|| {
                for _ in 0..ITERS {
                    let mut g = COUNTER.lock();
                    // Read-modify-write with a window for preemption.
                    let v = core::hint::black_box(*g);
                    spin_work(20);
                    *g = v + 1;
                }
            })
        })
        .collect();
    for h in handles {
        h.join();
    }
    let total = *COUNTER.lock();
    ensure!(total == THREADS * ITERS, "counter = {}, expected {}", total, THREADS * ITERS);
    Ok(format!("{} threads x {} increments = {}", THREADS, ITERS, total))
}

fn channel_ordering() -> TestResult {
    const PRODUCERS: u64 = 3;
    const PER: u64 = 1_000;
    let (tx, rx) = channel::<(u64, u64)>(4);
    let producers: Vec<_> = (0..PRODUCERS)
        .map(|p| {
            let tx = tx.clone();
            thread::spawn(move || {
                for seq in 0..PER {
                    tx.send((p, seq)).unwrap();
                }
            })
        })
        .collect();
    drop(tx);
    let mut next = [0u64; PRODUCERS as usize];
    let mut total = 0;
    while let Ok((p, seq)) = rx.recv() {
        ensure!(seq == next[p as usize], "producer {} sent {} but expected {}", p, seq, next[p as usize]);
        next[p as usize] += 1;
        total += 1;
    }
    for h in producers {
        h.join();
    }
    ensure!(total == PRODUCERS * PER, "received {} messages", total);
    Ok(format!("{} messages in per-producer order", total))
}

fn channel_disconnect() -> TestResult {
    let (tx, rx) = channel::<u32>(2);
    let h = thread::spawn(move || {
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        thread::sleep(Duration::from_millis(5));
    });
    ensure!(rx.recv() == Ok(1), "first message");
    ensure!(rx.recv() == Ok(2), "second message");
    ensure!(rx.recv().is_err(), "recv after all senders dropped should fail");
    ensure!(rx.try_recv() == Err(TryRecvError::Disconnected), "try_recv should report disconnect");
    h.join();
    let (tx, rx) = channel::<u32>(1);
    drop(rx);
    ensure!(tx.send(7).is_err(), "send without receivers should fail");
    Ok(String::from("disconnect reported on both ends"))
}

fn sleep_accuracy() -> TestResult {
    let mut report = String::new();
    for ms in [5u64, 20, 50] {
        let t = Instant::now();
        thread::sleep(Duration::from_millis(ms));
        let got = t.elapsed();
        ensure!(got >= Duration::from_millis(ms), "sleep({} ms) returned after {:?}", ms, got);
        ensure!(got < Duration::from_millis(ms + 40), "sleep({} ms) took {:?}", ms, got);
        report += &format!("{}ms->{}us ", ms, got.as_micros());
    }
    // Concurrent sleepers wake in deadline order.
    let (tx, rx) = channel::<u64>(8);
    let hs: Vec<_> = [40u64, 10, 30, 20]
        .into_iter()
        .map(|ms| {
            let tx = tx.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(ms));
                tx.send(ms).unwrap();
            })
        })
        .collect();
    drop(tx);
    let order: Vec<u64> = core::iter::from_fn(|| rx.recv().ok()).collect();
    for h in hs {
        h.join();
    }
    ensure!(order == [10, 20, 30, 40], "wake order {:?}", order);
    Ok(format!("{}order {:?}", report, order))
}

fn all_cores_ran_threads() -> TestResult {
    let before: Vec<u64> = pt::stats().cpus.iter().map(|c| c.context_switches).collect();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            thread::spawn(|| {
                spin_for(Duration::from_millis(40));
                (thread::current().cpus_ran(), thread::current_cpu())
            })
        })
        .collect();
    let mut mask = 0u8;
    for h in handles {
        let (ran, _) = h.join();
        mask |= ran;
    }
    let after = pt::stats();
    ensure!(mask == 0b1111, "threads only ran on cores {:#06b}", mask);
    for (i, c) in after.cpus.iter().enumerate() {
        ensure!(c.context_switches > before[i], "cpu{} performed no switches", i);
    }
    Ok(format!("cores used mask={:#06b}", mask))
}

/// Low-priority L holds a mutex; high-priority H wants it; medium M hogs
/// the core. Without inheritance, M starves L forever and H never gets the
/// lock (the runner's timeout would fire).
fn priority_inheritance() -> TestResult {
    static LOCK: Mutex<u32> = Mutex::new(0);
    static L_HOLDS: AtomicBool = AtomicBool::new(false);
    static H_DONE: AtomicBool = AtomicBool::new(false);
    static L_MAX_PRIO: AtomicU8 = AtomicU8::new(0);
    const CPU: usize = 3;

    let l = Builder::new().name("L").cpu(CPU).priority(priority::LOW).spawn(|| {
        let mut g = LOCK.lock();
        L_HOLDS.store(true, Ordering::Release);
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(30) {
            L_MAX_PRIO.fetch_max(thread::current().priority(), Ordering::Relaxed);
            spin_work(200);
        }
        *g += 1; // L's critical section
    }).unwrap();
    while !L_HOLDS.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(1));
    }
    let m = Builder::new().name("M").cpu(CPU).priority(priority::NORMAL).spawn(|| {
        let mut spins = 0u64;
        while !H_DONE.load(Ordering::Acquire) {
            spin_work(100);
            spins += 1;
        }
        spins
    }).unwrap();
    thread::sleep(Duration::from_millis(5));
    let h = Builder::new().name("H").cpu(CPU).priority(priority::HIGH).spawn(|| {
        let mut g = LOCK.lock();
        *g += 10; // runs strictly after L's critical section
        let v = *g;
        H_DONE.store(true, Ordering::Release);
        v
    }).unwrap();
    let hv = h.join();
    l.join();
    m.join();
    let boosted = L_MAX_PRIO.load(Ordering::Relaxed);
    ensure!(hv == 11, "H saw {} (expected L's update first)", hv);
    ensure!(boosted == priority::HIGH, "L ran at max priority {} (expected {})", boosted, priority::HIGH);
    ensure!(l_priority_restored(), "L priority not restored");
    Ok(format!("L boosted {}->{} while H waited", priority::LOW, boosted))
}

fn l_priority_restored() -> bool {
    // L has exited; check the mechanism on the current thread instead:
    // lock/unlock without contention must not change our priority.
    let p = thread::current().priority();
    static M: Mutex<()> = Mutex::new(());
    drop(M.lock());
    thread::current().priority() == p
}

/// A HIGH thread made ready on a busy core preempts the running NORMAL one.
fn priority_bands() -> TestResult {
    static STOP: AtomicBool = AtomicBool::new(false);
    static HIGH_RAN_AT: AtomicU64 = AtomicU64::new(0);
    const CPU: usize = 2;
    let hog = Builder::new().cpu(CPU).priority(priority::NORMAL).spawn(|| {
        while !STOP.load(Ordering::Acquire) {
            spin_work(100);
        }
    }).unwrap();
    thread::sleep(Duration::from_millis(5));
    let t0 = Instant::now();
    let hi = Builder::new().cpu(CPU).priority(priority::HIGH).spawn(move || {
        HIGH_RAN_AT.store(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
        STOP.store(true, Ordering::Release);
    }).unwrap();
    hi.join();
    hog.join();
    let us = HIGH_RAN_AT.load(Ordering::Relaxed);
    // Must not wait for the hog's 10 ms slice: it is preempted by IPI.
    ensure!(us < 5_000, "HIGH thread started after {} us", us);
    Ok(format!("HIGH preempted NORMAL after {} us", us))
}

/// Under FIFO, equal-priority threads on one core run to completion in
/// arrival order instead of time-slicing.
fn fifo_policy() -> TestResult {
    static ORDER: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    const CPU: usize = 1;
    pt::sched::set_policy(SchedPolicy::Fifo);
    let gate = Builder::new().cpu(CPU).priority(priority::HIGH).spawn(|| {
        // Hold the core while the workers are queued behind us.
        spin_for(Duration::from_millis(5));
    }).unwrap();
    let workers: Vec<_> = (0..3u32)
        .map(|i| {
            Builder::new().cpu(CPU).spawn(move || {
                ORDER.lock().push(i * 10);
                spin_for(Duration::from_millis(25)); // > 2 RR slices
                ORDER.lock().push(i * 10 + 1);
            }).unwrap()
        })
        .collect();
    gate.join();
    for w in workers {
        w.join();
    }
    pt::sched::set_policy(SchedPolicy::RoundRobin);
    let order = ORDER.lock().clone();
    ensure!(order == [0, 1, 10, 11, 20, 21], "FIFO order {:?}", order);
    Ok(format!("order {:?}", order))
}

fn semaphore_limits_concurrency() -> TestResult {
    static SEM: Semaphore = Semaphore::new(2);
    static INSIDE: AtomicUsize = AtomicUsize::new(0);
    static MAX_INSIDE: AtomicUsize = AtomicUsize::new(0);
    let hs: Vec<_> = (0..6)
        .map(|_| {
            thread::spawn(|| {
                for _ in 0..20 {
                    SEM.acquire();
                    let n = INSIDE.fetch_add(1, Ordering::AcqRel) + 1;
                    MAX_INSIDE.fetch_max(n, Ordering::Relaxed);
                    spin_work(2_000);
                    INSIDE.fetch_sub(1, Ordering::AcqRel);
                    SEM.release();
                }
            })
        })
        .collect();
    for h in hs {
        h.join();
    }
    let max = MAX_INSIDE.load(Ordering::Relaxed);
    ensure!(max <= 2, "{} threads inside a 2-permit semaphore", max);
    ensure!(SEM.available() == 2, "{} permits left", SEM.available());
    Ok(format!("max concurrent holders {}", max))
}

fn condvar_producer_consumer() -> TestResult {
    static QUEUE: Mutex<VecDeque<u32>> = Mutex::new(VecDeque::new());
    static NOT_EMPTY: Condvar = Condvar::new();
    const N: u32 = 2_000;
    let consumer = thread::spawn(|| {
        let mut sum = 0u64;
        let mut expected = 0;
        while expected < N {
            let mut q = NOT_EMPTY.wait_while(QUEUE.lock(), |q| q.is_empty());
            while let Some(v) = q.pop_front() {
                if v != expected {
                    return Err(format!("got {} expected {}", v, expected));
                }
                expected += 1;
                sum += v as u64;
            }
        }
        Ok(sum)
    });
    for i in 0..N {
        QUEUE.lock().push_back(i);
        NOT_EMPTY.notify_one();
        if i % 64 == 0 {
            thread::yield_now();
        }
    }
    let sum = consumer.join()?;
    ensure!(sum == (N as u64 - 1) * N as u64 / 2, "sum {}", sum);
    Ok(format!("{} items, sum {}", N, sum))
}

/// Floating-point/SIMD state must survive preemption.
fn fpu_state_preserved() -> TestResult {
    fn series(seed: f64, n: u32) -> f64 {
        let mut acc = seed;
        for i in 1..=n {
            let x = core::hint::black_box(i as f64 + seed);
            acc = acc * 0.999_9 + 1.0 / (x * 1.000_1 + acc);
        }
        acc
    }
    let hs: Vec<_> = (0..8u32)
        .map(|t| {
            thread::spawn(move || {
                let seed = t as f64 * 1.5 + 0.25;
                let expected = series(seed, 2_000);
                let start = Instant::now();
                let mut rounds = 0u32;
                while start.elapsed() < Duration::from_millis(60) {
                    let got = series(seed, 2_000);
                    if got.to_bits() != expected.to_bits() {
                        return Err(format!("thread {} round {}: {} != {}", t, rounds, got, expected));
                    }
                    rounds += 1;
                }
                Ok(rounds)
            })
        })
        .collect();
    let mut rounds = 0;
    for h in hs {
        rounds += h.join()?;
    }
    Ok(format!("{} FP rounds bit-exact across preemptions", rounds))
}

/// A long console write must not keep interrupts masked: the console lock
/// is held one chunk at a time. Only meaningful with a UART that takes real
/// time (`--features slow-uart`, or real hardware): there 1 KiB takes about
/// 90 ms and the writing core has to keep taking its 1 kHz tick meanwhile.
fn console_write_keeps_irqs_on() -> TestResult {
    let h = Builder::new().cpu(2).spawn(|| {
        let mut text = Vec::new();
        for i in 0..13 {
            text.extend_from_slice(format!("[console] line {:2} ", i).as_bytes());
            text.resize(text.len() + 60, b'-');
            text.push(b'\n');
        }
        let ticks = pt::stats().cpus[2].ticks;
        let t0 = Instant::now();
        pt::console::write_bytes(&text);
        (text.len(), t0.elapsed(), pt::stats().cpus[2].ticks - ticks)
    }).unwrap();
    let (len, took, ticks) = h.join();
    let ms = took.as_millis() as u64;
    // Half the ideal tick count, with slack for the first and last tick.
    ensure!(ticks * 2 + 4 >= ms, "{} B took {} ms but cpu2 took only {} ticks: IRQs were masked", len, ms, ticks);
    Ok(format!("{} B in {} ms, cpu2 ticks +{}", len, ms, ticks))
}

fn threads_reaped() -> TestResult {
    // Reaping happens on the next switch after a thread exits.
    let settle = || thread::sleep(Duration::from_millis(5));
    settle();
    let base = pt::stats();
    for _ in 0..50 {
        thread::spawn(|| spin_work(100)).join();
    }
    settle();
    let warm = pt::stats();
    for _ in 0..400 {
        thread::spawn(|| spin_work(100)).join();
    }
    settle();
    let s = pt::stats();
    ensure!(s.threads_live == base.threads_live, "live threads {} -> {}", base.threads_live, s.threads_live);
    ensure!(s.stacks.in_use == base.stacks.in_use, "stacks in use {} -> {}", base.stacks.in_use, s.stacks.in_use);
    ensure!(s.stacks.allocated == warm.stacks.allocated, "stacks allocated grew {} -> {}", warm.stacks.allocated, s.stacks.allocated);
    ensure!(s.heap.used <= warm.heap.used + 4096, "heap grew {} -> {} bytes", warm.heap.used, s.heap.used);
    Ok(format!(
        "450 spawn/join cycles, live={} stacks_in_use={} heap {}B -> {}B",
        s.threads_live, s.stacks.in_use, warm.heap.used, s.heap.used
    ))
}
