//! Self-checking test kernel for pt-os: user processes (M1/M2), the file
//! system, pipes, the console line discipline and the shell (M3/M4).
//!
//! Same protocol as the thread test kernel: one `[TEST] <name> ... ok` or
//! `... FAILED: <reason>` line per test, then `ALL <n> OS TESTS PASSED on 4
//! cores` and a semihosting exit status (0 = pass).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use preemptive_threads::arch::user::FaultKind;
use preemptive_threads::{self as pt, println, thread, Builder, Config, Duration, Instant};
use pt_os::console;
use pt_os::process::{self, STATUS_KILLED};
use pt_os::vfs;
use pt_os::{initrd, Errno, Image, Output, Pid, SpawnOptions};

type TestResult = Result<String, String>;
type Test = (&'static str, fn() -> TestResult);

macro_rules! ensure {
    ($cond:expr, $($fmt:tt)*) => {
        if !$cond {
            return Err(format!($($fmt)*));
        }
    };
}

#[no_mangle]
fn kernel_main() -> ! {
    println!();
    println!("pt-os test kernel ({}, entered at EL{})", pt::platform::NAME, pt::boot_el());
    pt::start(Config::default(), main)
}

static CURRENT_TEST: AtomicUsize = AtomicUsize::new(0);
static TEST_STARTED: AtomicU64 = AtomicU64::new(0);
const TEST_DEADLINE: Duration = Duration::from_secs(30);

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

static BASELINE: pt::sync::IrqSpinLock<Option<pt_os::OsStats>> = pt::sync::IrqSpinLock::new(None);

fn main() {
    let files = initrd::mount().expect("initrd") + initrd::unpack(initrd::TEST_ARCHIVE).expect("test initrd");
    println!("[pt-os] initrd: {} files", files);
    console::init();
    let tests: &[Test] = &[
        ("el0_hello_exit_code", el0_hello_exit_code),
        ("syscalls_basic", syscalls_basic),
        ("user_spawn_wait_kill", user_spawn_wait_kill),
        ("kernel_write_killed", kernel_write_killed),
        ("exec_and_undef_killed", exec_and_undef_killed),
        ("isolation", isolation),
        ("sbrk_growth", sbrk_growth),
        ("busy_loop_preempted", busy_loop_preempted),
        ("user_registers_preserved", user_registers_preserved),
        ("procs_32_across_cores", procs_32_across_cores),
        ("elf_rejected", elf_rejected),
        ("asid_exhaustion", asid_exhaustion),
        ("resources_back_to_baseline", resources_back_to_baseline),
    ];
    *BASELINE.lock() = Some(pt_os::stats());
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
    let s = pt::stats();
    let o = pt_os::stats();
    println!(
        "[STATS] cpus={} switches={} preemptions={} frames free {}/{} asids free {}",
        s.cpus_online(),
        s.context_switches(),
        s.preemptions(),
        o.frames_free,
        o.frames_total,
        o.asids_free
    );
    // Disarm the progress watchdog: without semihosting (real hardware) the
    // exit below only halts this core and the watchdog would keep running.
    TEST_STARTED.store(0, Ordering::Release);
    if failed == 0 {
        println!("ALL {} OS TESTS PASSED on {} cores", tests.len(), s.cpus_online());
        pt::arch::semihosting::exit(0);
    } else {
        println!("{} OF {} OS TESTS FAILED", failed, tests.len());
        pt::arch::semihosting::exit(1);
    }
}

// --- helpers ---------------------------------------------------------------

/// Where a test program lives: `/test/<name>` for the kernel-test
/// programs, `/bin/<name>` for the userland.
fn path_of(name: &str) -> String {
    let t = format!("/test/{}", name);
    if name.contains('/') || vfs::lookup(&t).is_err() {
        String::from(name)
    } else {
        t
    }
}

fn spawn_with(name: &str, args: &[&str], opts: SpawnOptions) -> Result<Pid, String> {
    let mut argv: Vec<&[u8]> = Vec::new();
    argv.push(name.as_bytes());
    argv.extend(args.iter().map(|a| a.as_bytes()));
    pt_os::spawn(Image::Named(&path_of(name)), &argv, opts).map_err(|e| format!("spawn {} {:?}: {:?}", name, args, e))
}

fn spawn(name: &str, args: &[&str]) -> Result<Pid, String> {
    spawn_with(name, args, SpawnOptions::default())
}

fn wait(pid: Pid) -> Result<i32, String> {
    pt_os::wait(Some(pid)).map(|(_, s)| s).map_err(|e| format!("wait {}: {:?}", pid, e))
}

/// Spawn with captured output, wait. Returns (pid, status, output).
fn run(name: &str, args: &[&str]) -> Result<(Pid, i32, String), String> {
    let out = Output::capture();
    let pid = spawn_with(name, args, SpawnOptions { output: out.clone(), ..Default::default() })?;
    let status = wait(pid)?;
    Ok((pid, status, String::from_utf8_lossy(&out.captured()).into_owned()))
}

/// Spawn `poke` and check it is killed with the given fault.
fn expect_fault(args: &[&str], kind: FaultKind, addr: Option<u64>) -> Result<String, String> {
    let pid = spawn("poke", args)?;
    let p = process::find(pid).ok_or("process vanished")?;
    let status = wait(pid)?;
    let f = p.fault().ok_or_else(|| format!("poke {:?}: exit status {} without a fault", args, status))?;
    ensure!(status == -kind.signal(), "poke {:?}: status {} (expected {})", args, status, -kind.signal());
    ensure!(f.kind == kind, "poke {:?}: fault {:?} (expected {:?})", args, f.kind, kind);
    if let Some(a) = addr {
        ensure!(f.addr == a, "poke {:?}: fault address {:#x} (expected {:#x})", args, f.addr, a);
    }
    Ok(format!("[pid {}] {} at {:#x}", pid, f.kind, f.addr))
}

// --- tests -----------------------------------------------------------------

fn el0_hello_exit_code() -> TestResult {
    let (pid, status, out) = run("hello", &[])?;
    ensure!(status == 0, "hello exited with {}", status);
    ensure!(out.starts_with("hello from EL0"), "output {:?}", out);
    let (_, status42, _) = run("hello", &["42"])?;
    ensure!(status42 == 42, "hello 42 exited with {}", status42);
    Ok(format!("pid {} said {:?}, exit 0; exit(42) -> wait() = {}", pid, out.trim_end(), status42))
}

fn syscalls_basic() -> TestResult {
    let (pid, status, out) = run("syscalls", &[])?;
    ensure!(status == 0, "syscalls failed check {} (output {:?})", status, out);
    ensure!(out.contains(&format!("pid={} ", pid)), "getpid mismatch: pid {} output {:?}", pid, out);
    Ok(String::from(out.trim_end()))
}

fn user_spawn_wait_kill() -> TestResult {
    let (_, status, out) = run("parent", &[])?;
    ensure!(status == 0, "parent failed check {} (output {:?})", status, out);
    Ok(String::from(out.lines().last().unwrap_or("").trim_end()))
}

static KERNEL_CANARY: AtomicU64 = AtomicU64::new(0x1234_5678_9abc_def0);

fn kernel_write_killed() -> TestResult {
    let addr = &KERNEL_CANARY as *const AtomicU64 as u64;
    let r = expect_fault(&["w", &format!("{:#x}", addr)], FaultKind::Write, Some(addr))?;
    ensure!(KERNEL_CANARY.load(Ordering::SeqCst) == 0x1234_5678_9abc_def0, "kernel memory was modified");
    let r2 = expect_fault(&["r", &format!("{:#x}", addr)], FaultKind::Read, Some(addr))?;
    // The kernel keeps working: run another process afterwards.
    let (_, status, _) = run("hello", &[])?;
    ensure!(status == 0, "hello after the fault exited with {}", status);
    Ok(format!("{}; {}; canary intact", r, r2))
}

fn exec_and_undef_killed() -> TestResult {
    let kfn = kernel_main as *const () as u64;
    let r1 = expect_fault(&["x", &format!("{:#x}", kfn)], FaultKind::Exec, Some(kfn))?;
    let r2 = expect_fault(&["u"], FaultKind::IllegalInstruction, None)?;
    Ok(format!("{}; {}", r1, r2))
}

fn wait_for_output(out: &Output, needle: &str) -> Result<String, String> {
    let t = Instant::now();
    loop {
        let s = String::from_utf8_lossy(&out.captured()).into_owned();
        if s.contains(needle) {
            return Ok(s);
        }
        ensure!(t.elapsed() < Duration::from_secs(5), "no {:?} in output {:?}", needle, s);
        thread::sleep(Duration::from_millis(2));
    }
}

fn isolation() -> TestResult {
    const SECRET: u64 = 0x5ec2_e75e_c2e7_5ec2;
    let out_a = Output::capture();
    let a = spawn_with("secret", &[], SpawnOptions { output: out_a.clone(), ..Default::default() })?;
    wait_for_output(&out_a, "stored at")?;
    let pa = process::find(a).ok_or("secret vanished")?;
    let (heap, _) = pa.with_mm(|m| m.heap()).ok_or("secret has no memory")?;
    let mut buf = [0u8; 8];
    pa.with_mm(|m| m.copy_from_user(heap, &mut buf)).ok_or("no mm")?.map_err(|e| format!("{:?}", e))?;
    ensure!(u64::from_le_bytes(buf) == SECRET, "secret not where expected");
    // B is a different program: it never maps A's heap page.
    let r = expect_fault(&["r", &format!("{:#x}", heap)], FaultKind::Read, Some(heap))?;
    // A second `secret` maps the same virtual address, backed by another frame.
    let out_b = Output::capture();
    let b = spawn_with("secret", &[], SpawnOptions { output: out_b.clone(), ..Default::default() })?;
    wait_for_output(&out_b, "stored at")?;
    let pb = process::find(b).ok_or("secret B vanished")?;
    let fa = pa.with_mm(|m| m.translate(heap)).flatten().ok_or("A heap unmapped")?;
    let fb = pb.with_mm(|m| m.translate(heap)).flatten().ok_or("B heap not at the same address")?;
    ensure!(fa != fb, "A and B share frame {:#x}", fa);
    let (asid_a, asid_b) = (pa.with_mm(|m| m.asid()).unwrap_or(0), pb.with_mm(|m| m.asid()).unwrap_or(0));
    ensure!(asid_a != asid_b, "A and B share ASID {}", asid_a);
    pt_os::kill(a).map_err(|e| format!("{:?}", e))?;
    pt_os::kill(b).map_err(|e| format!("{:?}", e))?;
    ensure!(wait(a)? == STATUS_KILLED && wait(b)? == STATUS_KILLED, "secret not killed");
    Ok(format!("{}; same VA {:#x} -> frames {:#x}/{:#x}, ASIDs {}/{}", r, heap, fa, fb, asid_a, asid_b))
}

fn sbrk_growth() -> TestResult {
    let (_, status, out) = run("sbrk", &[])?;
    ensure!(status == 0, "sbrk failed step {} (output {:?})", status, out);
    let (pid, status, _) = run("sbrk", &["fault"])?;
    ensure!(status == -11, "touching freed heap: status {}", status);
    let _ = pid;
    Ok(format!("{}; freed heap page faults", out.trim_end()))
}

fn busy_loop_preempted() -> TestResult {
    const CPU: usize = 1;
    let before = pt::stats().cpus[CPU].preemptions;
    let busy = spawn_with("busy", &[], SpawnOptions { cpu: Some(CPU), ..Default::default() })?;
    thread::sleep(Duration::from_millis(20));
    let t = Instant::now();
    let spin = spawn_with("spin", &["30"], SpawnOptions { cpu: Some(CPU), ..Default::default() })?;
    let mask = wait(spin)?;
    let took = t.elapsed();
    let bp = process::find(busy).ok_or("busy vanished")?;
    ensure!(bp.status().is_none(), "busy exited early with {:?}", bp.status());
    ensure!(mask == 1 << CPU, "spin ran on cores {:#06b}", mask);
    let after = pt::stats().cpus[CPU].preemptions;
    ensure!(after > before, "no preemption recorded on cpu{}", CPU);
    let tk = Instant::now();
    pt_os::kill(busy).map_err(|e| format!("{:?}", e))?;
    let st = wait(busy)?;
    ensure!(st == STATUS_KILLED, "busy status {}", st);
    Ok(format!(
        "spin finished next to a syscall-free busy loop on cpu{} in {} ms, cpu{} preemptions +{}, kill took {} us",
        CPU,
        took.as_millis(),
        CPU,
        after - before,
        tk.elapsed().as_micros()
    ))
}

/// Three processes share one core; each keeps its own TPIDR_EL0 and FP state.
fn user_registers_preserved() -> TestResult {
    const CPU: usize = 2;
    let before = pt::stats().cpus[CPU].preemptions;
    let tags = ["0x1111", "0x2222", "0x3333"];
    let pids: Vec<Pid> = tags
        .iter()
        .map(|t| spawn_with("regs", &[t], SpawnOptions { cpu: Some(CPU), ..Default::default() }))
        .collect::<Result<_, _>>()?;
    for (pid, tag) in pids.iter().zip(tags) {
        let s = wait(*pid)?;
        ensure!(s == 0, "regs {} failed with {} (1 = TPIDR_EL0, 2 = FP)", tag, s);
    }
    let preempted = pt::stats().cpus[CPU].preemptions - before;
    ensure!(preempted > 0, "no preemption on cpu{}", CPU);
    Ok(format!("3 processes on cpu{}, {} preemptions, TPIDR_EL0 and FP intact", CPU, preempted))
}

fn procs_32_across_cores() -> TestResult {
    let pids: Vec<Pid> = (0..32).map(|_| spawn("spin", &["40"])).collect::<Result<_, _>>()?;
    let procs: Vec<_> = pids.iter().filter_map(|&p| process::find(p)).collect();
    let mut mask = 0;
    for &pid in &pids {
        let s = wait(pid)?;
        ensure!((1..=15).contains(&s), "pid {} exited with {}", pid, s);
        mask |= s;
    }
    let ran = procs.iter().fold(0u8, |m, p| m | p.cpus_ran());
    ensure!(mask == 0b1111 && ran == 0b1111, "cores used: reported {:#06b}, scheduler {:#06b}", mask, ran);
    Ok(format!("32 processes, cores used mask={:#06b}", mask))
}

fn elf_rejected() -> TestResult {
    let hello = vfs::read_file("/bin/hello").map_err(|e| format!("no /bin/hello: {:?}", e))?;
    let hello = &hello[..];
    let expect = |what: &str, img: &[u8], want: Errno| -> Result<(), String> {
        let r = pt_os::spawn(Image::Elf(img), &[b"x"], SpawnOptions::default());
        ensure!(r == Err(want), "{}: {:?} (expected {:?})", what, r, want);
        Ok(())
    };
    expect("garbage", &[0x7f, b'E', b'L', b'F', 1, 2, 3], Errno::ENOEXEC)?;
    expect("truncated", &hello[..200], Errno::ENOEXEC)?;
    // Make the text segment writable as well as executable.
    let mut rwx = hello.to_vec();
    let phoff = u64::from_le_bytes(rwx[32..40].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(rwx[56..58].try_into().unwrap()) as usize;
    let mut patched = false;
    for i in 0..phnum {
        let ph = phoff + i * 56;
        let flags = u32::from_le_bytes(rwx[ph + 4..ph + 8].try_into().unwrap());
        if u32::from_le_bytes(rwx[ph..ph + 4].try_into().unwrap()) == 1 && flags & 1 != 0 {
            rwx[ph + 4..ph + 8].copy_from_slice(&(flags | 2).to_le_bytes());
            patched = true;
        }
    }
    ensure!(patched, "no executable segment in hello");
    expect("W+X segment", &rwx, Errno::ENOEXEC)?;
    // Load address inside the kernel's half.
    let mut low = hello.to_vec();
    let ph = phoff;
    low[ph + 16..ph + 24].copy_from_slice(&0x8_0000u64.to_le_bytes());
    expect("kernel address segment", &low, Errno::ENOEXEC)?;
    let r = pt_os::spawn(Image::Named("no-such-program"), &[b"x"], SpawnOptions::default());
    ensure!(r == Err(Errno::ENOENT), "unknown program: {:?}", r);
    Ok(String::from("garbage, truncated, W+X, kernel-address and unknown images refused"))
}

fn asid_exhaustion() -> TestResult {
    let free = pt_os::stats().asids_free;
    let mut pids = Vec::new();
    let err = loop {
        match pt_os::spawn(Image::Named("/test/secret"), &[b"secret", b"quiet"], SpawnOptions::default()) {
            Ok(pid) => pids.push(pid),
            Err(e) => break e,
        }
        ensure!(pids.len() <= 300, "no limit after 300 processes");
    };
    ensure!(err == Errno::EAGAIN, "spawn #{} failed with {:?} (expected EAGAIN)", pids.len() + 1, err);
    ensure!(pids.len() == free, "{} processes before EAGAIN, {} ASIDs were free", pids.len(), free);
    let t = Instant::now();
    for &p in &pids {
        pt_os::kill(p).map_err(|e| format!("{:?}", e))?;
    }
    for &p in &pids {
        let s = wait(p)?;
        ensure!(s == STATUS_KILLED, "pid {} status {}", p, s);
    }
    let reaped = t.elapsed();
    let (_, status, _) = run("hello", &[])?;
    ensure!(status == 0, "hello after exhaustion exited with {}", status);
    Ok(format!("{} live processes, then EAGAIN; killed and reaped all in {} ms; ASIDs reusable", pids.len(), reaped.as_millis()))
}

fn resources_back_to_baseline() -> TestResult {
    let base = BASELINE.lock().ok_or("no baseline")?;
    let now = pt_os::stats();
    ensure!(now == base, "baseline {:?}, now {:?}", base, now);
    Ok(format!("frames free {}/{}, ASIDs free {}, processes {}", now.frames_free, now.frames_total, now.asids_free, now.processes))
}
