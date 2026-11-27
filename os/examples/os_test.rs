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
use pt_os::console::{self, STATUS_INTERRUPTED};
use pt_os::process::{self, STATUS_KILLED};
use pt_os::vfs::{self, FdTable, File};
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
        ("vfs_files_and_dirs", vfs_files_and_dirs),
        ("fd_inheritance", fd_inheritance),
        ("pipe_eof_and_epipe", pipe_eof_and_epipe),
        ("read_interrupted_by_kill", read_interrupted_by_kill),
        ("procinfo", procinfo),
        ("console_line_discipline", console_line_discipline),
        ("ctrl_c_kills_foreground", ctrl_c_kills_foreground),
        ("shell_session", shell_session),
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

fn vfs_files_and_dirs() -> TestResult {
    let (_, status, out) = run("fstest", &["vfs"])?;
    ensure!(status == 0, "fstest vfs failed check {} (output {:?})", status, out);
    // Kernel-side view of the same tree.
    let motd = vfs::read_file("/etc/motd").map_err(|e| format!("/etc/motd: {:?}", e))?;
    ensure!(motd.windows(5).any(|w| w == b"pt-os"), "motd {:?}", String::from_utf8_lossy(&motd));
    ensure!(vfs::lookup("/tmp/t1").is_err(), "fstest left /tmp/t1 behind");
    Ok(String::from(out.trim_end()))
}

fn fd_inheritance() -> TestResult {
    let (_, status, out) = run("fstest", &["inherit"])?;
    ensure!(status == 0, "fstest inherit failed check {} (output {:?})", status, out);
    Ok(String::from(out.trim_end()))
}

fn pipe_eof_and_epipe() -> TestResult {
    let (_, status, out) = run("pipetest", &[])?;
    ensure!(status == 0, "pipetest failed check {} (output {:?})", status, out);
    Ok(String::from(out.lines().last().unwrap_or("").trim_end()))
}

/// Wait until process `pid` is blocked (in the kernel).
fn wait_blocked(pid: Pid) -> Result<(), String> {
    let p = process::find(pid).ok_or("process vanished")?;
    let t = Instant::now();
    while p.state() != b'S' {
        ensure!(t.elapsed() < Duration::from_secs(5), "pid {} never blocked (state {})", pid, p.state() as char);
        thread::sleep(Duration::from_millis(2));
    }
    // Give it a moment to really be asleep in the wait queue.
    thread::sleep(Duration::from_millis(10));
    ensure!(p.state() == b'S', "pid {} woke up by itself", pid);
    Ok(())
}

/// Kill `pid` (blocked in the kernel) and check it goes away quickly.
fn kill_blocked(pid: Pid, what: &str) -> Result<u64, String> {
    wait_blocked(pid)?;
    let t = Instant::now();
    pt_os::kill(pid).map_err(|e| format!("{:?}", e))?;
    let s = wait(pid)?;
    let us = t.elapsed().as_micros() as u64;
    ensure!(s == STATUS_KILLED, "{}: status {}", what, s);
    // Without interruptible waits these would hang (or take 100 s for
    // `sleep 100`); allow for a slow emulator host.
    ensure!(us < 1_000_000, "{}: kill took {} us", what, us);
    Ok(us)
}

fn read_interrupted_by_kill() -> TestResult {
    // A kernel thread blocked in a pipe read gets EINTR when interrupted.
    let (r, w) = vfs::pipe();
    let reader = Builder::new()
        .name("pipe-reader")
        .spawn(move || {
            let mut b = [0u8; 8];
            r.read(&mut b)
        })
        .map_err(|e| format!("{:?}", e))?;
    let t = Instant::now();
    while reader.thread().state() != pt::ThreadState::Blocked {
        ensure!(t.elapsed() < Duration::from_secs(2), "reader never blocked");
        thread::sleep(Duration::from_millis(1));
    }
    reader.thread().interrupt();
    let r = reader.join();
    ensure!(r == Err(Errno::EINTR), "interrupted pipe read returned {:?}", r);
    // Processes blocked in console read, pipe read, pipe write and sleep
    // die at once when killed.
    let console_cat = spawn_with("cat", &[], SpawnOptions { files: Some(FdTable::console()), ..Default::default() })?;
    let t_console = kill_blocked(console_cat, "cat on the console")?;
    let (r2, w2) = vfs::pipe();
    let mut fds = FdTable::new();
    fds.set(0, r2).map_err(|e| format!("{:?}", e))?;
    fds.set(1, File::console()).map_err(|e| format!("{:?}", e))?;
    let pipe_cat = spawn_with("cat", &[], SpawnOptions { files: Some(fds), ..Default::default() })?;
    let t_pipe = kill_blocked(pipe_cat, "cat on a pipe")?;
    let (r3, w3) = vfs::pipe();
    let mut fds = FdTable::new();
    fds.set(1, w3).map_err(|e| format!("{:?}", e))?;
    let blocker = spawn_with("pipetest", &["block"], SpawnOptions { files: Some(fds), ..Default::default() })?;
    let t_write = kill_blocked(blocker, "writer on a full pipe")?;
    ensure!(r3.available() == Ok(vfs::PIPE_CAPACITY), "pipe not full: {:?}", r3.available());
    let sleeper = spawn_with("sleep", &["100"], SpawnOptions::default())?;
    let t_sleep = kill_blocked(sleeper, "sleep 100")?;
    drop((w, w2, r3));
    Ok(format!(
        "kernel pipe read -> EINTR; killed while blocked: console read {} us, pipe read {} us, pipe write {} us, sleep {} us",
        t_console, t_pipe, t_write, t_sleep
    ))
}

fn procinfo() -> TestResult {
    const CPU: usize = 3;
    let spin = spawn_with("spin", &["3000"], SpawnOptions { cpu: Some(CPU), ..Default::default() })?;
    let secret = spawn_with("secret", &["quiet"], SpawnOptions::default())?;
    wait_blocked(secret)?;
    let sp = process::find(spin).ok_or("spin vanished")?;
    let t = Instant::now();
    while sp.cpu_time() < Duration::from_millis(20) {
        ensure!(t.elapsed() < Duration::from_secs(5), "spin got no CPU time: {:?}", sp.cpu_time());
        thread::sleep(Duration::from_millis(5));
    }
    let (ps_pid, status, out) = run("ps", &[])?;
    ensure!(status == 0, "ps exited {}", status);
    let row = |pid: Pid| -> Option<Vec<String>> {
        out.lines()
            .map(|l| l.split_whitespace().map(String::from).collect::<Vec<_>>())
            .find(|f| f.first().map(|s| s.as_str()) == Some(format!("{}", pid).as_str()))
    };
    let sr = row(spin).ok_or_else(|| format!("spin missing from {:?}", out))?;
    ensure!((sr[3] == "R" || sr[3] == "Q") && sr[4] == "3" && sr.last().map(|s| s.as_str()) == Some("spin"), "spin row {:?}", sr);
    let cr = row(secret).ok_or_else(|| format!("secret missing from {:?}", out))?;
    ensure!(cr[3] == "S", "secret row {:?}", cr);
    let pr = row(ps_pid).ok_or_else(|| format!("ps missing from {:?}", out))?;
    ensure!(pr[3] == "R", "ps row {:?}", pr);
    let cpu_ms = sp.cpu_time().as_millis();
    pt_os::kill(spin).map_err(|e| format!("{:?}", e))?;
    pt_os::kill(secret).map_err(|e| format!("{:?}", e))?;
    wait(spin)?;
    wait(secret)?;
    Ok(format!("ps: spin R on cpu3 ({} ms CPU), secret S, ps R", cpu_ms))
}

fn wait_captured(out: &Output, needle: &str) -> Result<String, String> {
    wait_for_output(out, needle)
}

fn console_line_discipline() -> TestResult {
    console::flush_input();
    console::capture_start();
    let out = Output::capture();
    let mut fds = FdTable::new();
    fds.set(0, File::console()).map_err(|e| format!("{:?}", e))?;
    fds.set(1, out.file()).map_err(|e| format!("{:?}", e))?;
    let cat = spawn_with("cat", &[], SpawnOptions { files: Some(fds), ..Default::default() })?;
    console::inject(b"hellx\x7fo w\x08\x08 world\r\n");
    wait_captured(&out, "hello world\n")?;
    console::inject(b"partial\x04");
    wait_captured(&out, "hello world\npartial")?;
    console::inject(b"junk\x15line\n");
    wait_captured(&out, "partialline\n")?;
    console::inject(b"\x04");
    let s = wait(cat)?;
    let echo = console::capture_stop();
    ensure!(s == 0, "cat exited {}", s);
    let got = String::from_utf8_lossy(&out.captured()).into_owned();
    ensure!(got == "hello world\npartialline\n", "cat read {:?}", got);
    let echo = String::from_utf8_lossy(&echo).into_owned();
    ensure!(echo.contains("hellx\x08 \x08o w\x08 \x08"), "echo {:?}", echo);
    Ok(String::from("backspace/DEL, CR LF, Ctrl-D flush and EOF, Ctrl-U; echoed"))
}

fn ctrl_c_kills_foreground() -> TestResult {
    console::flush_input();
    let spin = spawn_with("spin", &["60000"], SpawnOptions { pgid: Some(0), ..Default::default() })?;
    console::set_foreground(spin);
    thread::sleep(Duration::from_millis(30));
    let t = Instant::now();
    console::inject(b"\x03");
    let s = wait(spin)?;
    let us = t.elapsed().as_micros();
    console::set_foreground(0);
    ensure!(s == STATUS_INTERRUPTED, "spin status {}", s);
    // With no foreground job, Ctrl-C abandons the line and hands the
    // reader an empty one.
    let out = Output::capture();
    let mut fds = FdTable::new();
    fds.set(0, File::console()).map_err(|e| format!("{:?}", e))?;
    fds.set(1, out.file()).map_err(|e| format!("{:?}", e))?;
    let cat = spawn_with("cat", &[], SpawnOptions { files: Some(fds), ..Default::default() })?;
    console::inject(b"abandoned\x03");
    wait_captured(&out, "\n")?;
    console::inject(b"\x04");
    ensure!(wait(cat)? == 0, "cat did not exit");
    let got = String::from_utf8_lossy(&out.captured()).into_owned();
    ensure!(got == "\n", "cat read {:?}", got);
    Ok(format!("foreground spin interrupted (status -2) in {} us; no foreground: line abandoned", us))
}

struct Typist {
    mark: usize,
}

impl Typist {
    fn prompts(text: &str) -> usize {
        text.matches("pt-os> ").count()
    }

    /// Type `line` + Enter and return the output up to the next prompt.
    fn command(&mut self, line: &str) -> Result<String, String> {
        let before = Self::prompts(&String::from_utf8_lossy(&console::captured()));
        let mut bytes = Vec::from(line.as_bytes());
        bytes.push(b'\r');
        console::inject(&bytes);
        let t = Instant::now();
        loop {
            let text = String::from_utf8_lossy(&console::captured()).into_owned();
            if Self::prompts(&text) > before {
                let end = text.rfind("pt-os> ").unwrap_or(text.len());
                let out = String::from(&text[self.mark.min(end)..end]);
                self.mark = end + "pt-os> ".len();
                return Ok(out);
            }
            ensure!(t.elapsed() < Duration::from_secs(20), "no prompt after {:?}; output {:?}", line, &text[self.mark.min(text.len())..]);
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn shell_session() -> TestResult {
    console::flush_input();
    console::capture_start();
    let sh = spawn_with("/bin/sh", &[], SpawnOptions { files: Some(FdTable::console()), ..Default::default() })?;
    let t = Instant::now();
    while !String::from_utf8_lossy(&console::captured()).contains("pt-os> ") {
        ensure!(t.elapsed() < Duration::from_secs(5), "no prompt from sh");
        thread::sleep(Duration::from_millis(5));
    }
    let mut ty = Typist { mark: "pt-os> ".len() };
    let expect = |out: &str, needle: &str, what: &str| -> Result<(), String> {
        ensure!(out.contains(needle), "{}: {:?} not in {:?}", what, needle, out);
        Ok(())
    };
    let out = ty.command("echo hi | wc")?;
    expect(&out, "      1       1       3\n", "echo hi | wc")?;
    let out = ty.command("ls /bin")?;
    for p in ["cat", "mandel", "primes", "sh", "top", "wc"] {
        expect(&out, p, "ls /bin")?;
    }
    let bg = ty.command("primes 100000 &")?;
    ensure!(bg.contains("] started on cpu"), "primes &: {:?}", bg);
    let out = ty.command("wait")?;
    let all = String::from(&bg) + &out;
    expect(&all, "primes below 100000: 9592", "wait")?;
    expect(&all, "done (exited 0)", "wait")?;
    ty.command("cat /etc/motd > /tmp/x")?;
    let out = ty.command("cat /tmp/x")?;
    expect(&out, "welcome to pt-os", "cat /tmp/x")?;
    let out = ty.command("cat /tmp/x | grep pt-os | wc")?;
    expect(&out, "      1 ", "3-stage pipeline")?;
    let out = ty.command("crash")?;
    expect(&out, "] segfault\n", "crash")?;
    let out = ty.command("echo 'still  alive'")?;
    expect(&out, "still  alive\n", "after crash")?;
    // Ctrl-C a running job: the shell survives.
    // (Ctrl-C again if the first one came before the shell started the job.)
    console::inject(b"spin 60000\r");
    thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    'ctrl_c: loop {
        console::inject(b"\x03");
        let sent = Instant::now();
        while sent.elapsed() < Duration::from_secs(2) {
            let text = String::from_utf8_lossy(&console::captured()).into_owned();
            let seg = &text[ty.mark.min(text.len())..];
            if seg.contains("] interrupted\n") && seg.rfind("pt-os> ") > seg.find("] interrupted\n") {
                ensure!(seg.contains("^C\n"), "Ctrl-C output {:?}", seg);
                break 'ctrl_c;
            }
            thread::sleep(Duration::from_millis(5));
        }
        ensure!(t.elapsed() < Duration::from_secs(10), "spin never interrupted");
    }
    // Let any extra prompt (from an early Ctrl-C) come out first.
    thread::sleep(Duration::from_millis(200));
    let text = String::from_utf8_lossy(&console::captured()).into_owned();
    ty.mark = text.rfind("pt-os> ").unwrap_or(0) + "pt-os> ".len();
    let out = ty.command("echo back")?;
    expect(&out, "back\n", "after Ctrl-C")?;
    console::inject(b"rm /tmp/x\rexit 3\r");
    let s = wait(sh)?;
    console::capture_stop();
    ensure!(s == 3, "sh exit status {}", s);
    Ok(String::from("echo|wc, ls, primes & + wait, > and cat, 3-stage pipe, crash, Ctrl-C, exit 3"))
}

fn resources_back_to_baseline() -> TestResult {
    let base = BASELINE.lock().ok_or("no baseline")?;
    let now = pt_os::stats();
    ensure!(now == base, "baseline {:?}, now {:?}", base, now);
    Ok(format!("frames free {}/{}, ASIDs free {}, processes {}", now.frames_free, now.frames_total, now.asids_free, now.processes))
}
