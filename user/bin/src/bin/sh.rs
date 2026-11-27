//! `sh`: the pt-os shell.
//!
//! ```text
//! pt-os> ls /bin | grep p > /tmp/p-programs
//! pt-os> primes 1000000 &
//! [pid 9] started on cpu2
//! pt-os> wait
//! ```
//!
//! - words split on blanks; `'single'` and `"double"` quotes, `\` escapes;
//! - `a | b | c` pipelines (any number of stages), `< file`, `> file`,
//!   `>> file`; `&` runs a job in the background;
//! - lists: `a ; b` runs both, `a && b` runs `b` only if `a` exited 0
//!   (the exit status of a pipeline is its last stage's);
//! - builtins: `cd`, `pwd`, `jobs`, `wait [pid]`, `kill pid...`,
//!   `exit [code]`, `help`;
//! - every job gets its own process group, made the console's foreground
//!   group while the shell waits for it, so Ctrl-C kills the job and not
//!   the shell. `wait` brings the jobs it waits for to the foreground one
//!   at a time: Ctrl-C interrupts the job being waited for and returns to
//!   the prompt;
//! - abnormal exits are reported (`[pid 7] interrupted`), and finished
//!   background jobs before the next prompt. A process killed by a fault
//!   is reported by the kernel, with the address and pc, so the shell
//!   says nothing more.
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::{File, LineReader, SpawnAttr, O_APPEND, O_CREATE, O_READ, O_TRUNC, O_WRITE};

ulib::entry!(main);

const PROMPT: &str = "pt-os> ";

#[derive(Debug, PartialEq)]
enum Tok {
    Word(String),
    Pipe,
    In,
    Out,
    Append,
    Amp,
    AndAnd,
    Semi,
}

fn tokenize(line: &str) -> Result<Vec<Tok>, &'static str> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    toks.push(Tok::Word(core::mem::take(&mut cur)));
                    in_word = false;
                }
            }
            '|' | '<' | '>' | '&' | ';' => {
                if in_word {
                    toks.push(Tok::Word(core::mem::take(&mut cur)));
                    in_word = false;
                }
                toks.push(match c {
                    '|' => Tok::Pipe,
                    '<' => Tok::In,
                    ';' => Tok::Semi,
                    '&' if chars.peek() == Some(&'&') => {
                        chars.next();
                        Tok::AndAnd
                    }
                    '&' => Tok::Amp,
                    _ if chars.peek() == Some(&'>') => {
                        chars.next();
                        Tok::Append
                    }
                    _ => Tok::Out,
                });
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated '"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch) => cur.push(ch),
                            None => return Err("unterminated \""),
                        },
                        Some(ch) => cur.push(ch),
                        None => return Err("unterminated \""),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(ch) = chars.next() {
                    cur.push(ch);
                }
            }
            '#' if !in_word => break,
            _ => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        toks.push(Tok::Word(cur));
    }
    Ok(toks)
}

#[derive(Default, Debug)]
struct Stage {
    argv: Vec<String>,
    stdin: Option<String>,
    stdout: Option<(String, bool)>, // (path, append)
}

struct Job {
    stages: Vec<Stage>,
    background: bool,
}

impl Job {
    /// The job as a command line, for `jobs`.
    fn text(&self) -> String {
        let stages: Vec<String> = self.stages.iter().map(|s| s.argv.join(" ")).collect();
        stages.join(" | ")
    }
}

/// When a job in a list runs.
#[derive(Clone, Copy, PartialEq)]
enum Cond {
    /// First job, or after `;` or `&`.
    Always,
    /// After `&&`: only if the previous job exited 0.
    IfOk,
}

/// Parse a command line into a list of jobs separated by `;`, `&&` and
/// `&`.
fn parse(toks: Vec<Tok>) -> Result<Vec<(Cond, Job)>, &'static str> {
    let mut list = Vec::new();
    let mut cond = Cond::Always;
    let mut stages = vec![Stage::default()];
    let mut it = toks.into_iter().peekable();
    loop {
        let t = it.next();
        let st = stages.last_mut().unwrap();
        match t {
            Some(Tok::Word(w)) => st.argv.push(w),
            Some(Tok::Pipe) => {
                if st.argv.is_empty() {
                    return Err("empty pipeline stage");
                }
                stages.push(Stage::default());
            }
            Some(t @ (Tok::In | Tok::Out | Tok::Append)) => {
                let Some(Tok::Word(path)) = it.next() else { return Err("missing file name after redirect") };
                match t {
                    Tok::In => st.stdin = Some(path),
                    Tok::Out => st.stdout = Some((path, false)),
                    _ => st.stdout = Some((path, true)),
                }
            }
            Some(Tok::Amp | Tok::AndAnd | Tok::Semi) | None => {
                let empty = stages.len() == 1 && stages[0].argv.is_empty();
                if !empty && stages.iter().any(|s| s.argv.is_empty()) {
                    return Err("empty pipeline stage");
                }
                match t {
                    None if empty => {
                        if cond == Cond::IfOk {
                            return Err("missing command after &&");
                        }
                        return Ok(list);
                    }
                    None => {}
                    // `a ;` and `a & ;` are fine, a lone `;` or `&&` is not.
                    Some(Tok::Semi) if empty && !list.is_empty() && cond == Cond::Always => continue,
                    Some(Tok::Amp) if empty => return Err("nothing to run in the background"),
                    Some(_) if empty => return Err("missing command before ; or &&"),
                    _ => {}
                }
                let job = Job { stages: core::mem::replace(&mut stages, vec![Stage::default()]), background: t == Some(Tok::Amp) };
                list.push((cond, job));
                match t {
                    None => return Ok(list),
                    Some(Tok::AndAnd) => cond = Cond::IfOk,
                    _ => cond = Cond::Always,
                }
            }
        }
    }
}

/// A background job's process: (pid, process group, command line).
struct BgProc {
    pid: i64,
    pgid: u32,
    cmd: String,
}

struct Shell {
    /// Background jobs, one entry per process.
    jobs: Vec<BgProc>,
}

/// Exit status of a job whose command was not found.
const NOT_FOUND: i32 = 127;

fn report(pid: i64, status: i32, background: bool) {
    if ulib::is_fault_status(status) {
        // The kernel has already said what happened, where, and at which pc.
    } else if status < 0 {
        println!("[pid {}] {}", pid, ulib::status_text(status));
    } else if background {
        println!("[pid {}] done ({})", pid, ulib::status_text(status));
    }
}

fn cpu_of(pid: i64) -> Option<u8> {
    ulib::procinfo().into_iter().find(|p| p.pid as i64 == pid).map(|p| p.cpu)
}

impl Shell {
    /// Reap finished background jobs and say so.
    fn reap(&mut self) {
        while let Ok(Some((pid, status))) = ulib::try_wait_any() {
            report(pid, status, true);
            self.jobs.retain(|j| j.pid != pid);
        }
    }

    /// `wait [pid]`: wait for one background process, or all of them.
    /// Each job is the console's foreground group while we wait for it,
    /// so Ctrl-C interrupts it; the remaining jobs keep running and we go
    /// back to the prompt.
    fn wait_jobs(&mut self, only: Option<i64>) -> i32 {
        let mut targets: Vec<(i64, u32)> =
            self.jobs.iter().filter(|j| only.is_none_or(|p| j.pid == p)).map(|j| (j.pid, j.pgid)).collect();
        if let (Some(pid), true) = (only, targets.is_empty()) {
            targets.push((pid, 0));
        }
        let mut status = 0;
        let mut interrupted: Option<u32> = None;
        for (pid, pgid) in targets {
            if interrupted.is_some_and(|g| g != pgid) {
                break;
            }
            if pgid != 0 {
                ulib::setfg(pgid);
            }
            match ulib::wait_pid(pid) {
                Ok(s) => {
                    report(pid, s, true);
                    status = s;
                    if s == ulib::STATUS_INTERRUPTED {
                        interrupted = Some(pgid);
                    }
                }
                Err(e) => {
                    eprintln!("sh: wait {}: {}", pid, e);
                    status = 1;
                }
            }
            self.jobs.retain(|j| j.pid != pid);
        }
        ulib::setfg(0);
        if only.is_none() && interrupted.is_none() {
            self.reap();
        }
        status
    }

    /// Run a builtin. Returns `None` if `argv[0]` is not one, else its
    /// exit status.
    fn builtin(&mut self, argv: &[String]) -> Option<i32> {
        let args: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let mut status = 0;
        match args[0] {
            "exit" => {
                let code = args.get(1).and_then(|a| a.parse::<i32>().ok()).unwrap_or(0);
                ulib::exit(code);
            }
            "cd" => {
                let dir = args.get(1).copied().unwrap_or("/");
                let r = ulib::chdir(dir);
                if r < 0 {
                    eprintln!("sh: cd: {}: {}", dir, Errno(-r));
                    status = 1;
                }
            }
            "pwd" => {
                let mut b = [0u8; 260];
                let n = ulib::getcwd(&mut b);
                if n >= 0 {
                    println!("{}", core::str::from_utf8(&b[..n as usize]).unwrap_or("?"));
                }
            }
            "jobs" => {
                self.reap();
                let procs = ulib::procinfo();
                for j in &self.jobs {
                    let st = procs.iter().find(|p| p.pid as i64 == j.pid);
                    let (state, cpu) = st.map_or(('?', 0), |p| (p.state, p.cpu));
                    let what = match state {
                        'R' => "running",
                        'Q' => "ready",
                        'S' => "sleeping",
                        _ => "exited",
                    };
                    println!("[pid {}] {} on cpu{}  {}", j.pid, what, cpu, j.cmd);
                }
            }
            "wait" => match args.get(1) {
                Some(p) => match p.parse::<i64>() {
                    Ok(pid) => status = self.wait_jobs(Some(pid)),
                    Err(_) => {
                        eprintln!("sh: wait: bad pid {}", p);
                        status = 1;
                    }
                },
                None => status = self.wait_jobs(None),
            },
            "kill" => {
                if args.len() < 2 {
                    eprintln!("usage: kill pid...");
                    status = 1;
                }
                for p in &args[1..] {
                    match p.parse::<i64>() {
                        Ok(pid) => {
                            let r = ulib::kill(pid);
                            if r < 0 {
                                eprintln!("sh: kill {}: {}", pid, Errno(-r));
                                status = 1;
                            }
                        }
                        Err(_) => {
                            eprintln!("sh: kill: bad pid {}", p);
                            status = 1;
                        }
                    }
                }
            }
            "help" => {
                println!("builtins: cd pwd jobs wait [pid] kill pid... exit [code] help");
                println!("syntax:   cmd args | cmd > file >> file < file &   a ; b   a && b   'quotes' \"quotes\"");
                print!("programs:");
                if let Ok(es) = ulib::read_dir("/bin") {
                    for e in es {
                        print!(" {}", e.name);
                    }
                }
                println!();
                println!("keys:     Ctrl-C interrupts the foreground job (or a wait), Ctrl-D ends input");
            }
            _ => return None,
        }
        Some(status)
    }

    /// Run one job. Returns its exit status (0 for a background job).
    fn run(&mut self, job: Job) -> i32 {
        if job.stages.len() == 1 && !job.background {
            if let Some(status) = self.builtin(&job.stages[0].argv) {
                return status;
            }
        }
        let n = job.stages.len();
        // Open everything first, so a bad redirect starts nothing.
        let mut pipes: Vec<[i32; 2]> = Vec::new();
        for _ in 1..n {
            let mut fds = [0i32; 2];
            let r = ulib::pipe(&mut fds);
            if r < 0 {
                eprintln!("sh: pipe: {}", Errno(-r));
                close_all(&pipes, &[]);
                return 1;
            }
            pipes.push(fds);
        }
        let mut files: Vec<File> = Vec::new();
        let mut ins = Vec::new();
        let mut outs = Vec::new();
        for (i, st) in job.stages.iter().enumerate() {
            let input = match &st.stdin {
                Some(path) => match File::open(path, O_READ) {
                    Ok(f) => {
                        let fd = f.fd();
                        files.push(f);
                        fd
                    }
                    Err(e) => {
                        eprintln!("sh: {}: {}", path, e);
                        close_all(&pipes, &[]);
                        return 1;
                    }
                },
                None if i == 0 => 0,
                None => pipes[i - 1][0],
            };
            let output = match &st.stdout {
                Some((path, append)) => {
                    let fl = O_WRITE | O_CREATE | if *append { O_APPEND } else { O_TRUNC };
                    match File::open(path, fl) {
                        Ok(f) => {
                            let fd = f.fd();
                            files.push(f);
                            fd
                        }
                        Err(e) => {
                            eprintln!("sh: {}: {}", path, e);
                            close_all(&pipes, &[]);
                            return 1;
                        }
                    }
                }
                None if i == n - 1 => 1,
                None => pipes[i][1],
            };
            ins.push(input);
            outs.push(output);
        }
        let mut pids = Vec::new();
        let mut pgid = 0u32;
        let mut last_missing = false;
        for (i, st) in job.stages.iter().enumerate() {
            let attr = SpawnAttr::new().fd(0, ins[i]).fd(1, outs[i]).fd(2, 2).group(pgid);
            let argv: Vec<&str> = st.argv.iter().map(|s| s.as_str()).collect();
            match ulib::spawn_path(argv[0], &argv, Some(&attr)) {
                Ok(pid) => {
                    if pgid == 0 {
                        pgid = pid as u32;
                    }
                    pids.push(pid);
                }
                Err(e) => {
                    if e.0 == ulib::errno::ENOENT {
                        eprintln!("sh: {}: command not found", argv[0]);
                    } else {
                        eprintln!("sh: {}: {}", argv[0], e);
                    }
                    last_missing = i == n - 1;
                }
            }
        }
        // Our copies of the pipe ends and files must go, or readers never
        // see end of file.
        close_all(&pipes, &[]);
        drop(files);
        if pids.is_empty() {
            return NOT_FOUND;
        }
        if job.background {
            let cmd = job.text();
            for &pid in &pids {
                match cpu_of(pid) {
                    Some(c) => println!("[pid {}] started on cpu{}", pid, c),
                    None => println!("[pid {}] started", pid),
                }
                self.jobs.push(BgProc { pid, pgid, cmd: cmd.clone() });
            }
            return 0;
        }
        ulib::setfg(pgid);
        let mut status = 0;
        for &pid in &pids {
            match ulib::wait_pid(pid) {
                Ok(s) => {
                    report(pid, s, false);
                    status = s;
                }
                Err(e) => {
                    eprintln!("sh: wait {}: {}", pid, e);
                    status = 1;
                }
            }
        }
        ulib::setfg(0);
        if last_missing {
            NOT_FOUND
        } else {
            status
        }
    }

    /// Run a list of jobs: `a ; b && c & d`.
    fn run_list(&mut self, list: Vec<(Cond, Job)>) {
        let mut status = 0;
        for (cond, job) in list {
            // A skipped job keeps the failing status, so `a && b && c`
            // skips both b and c when a fails.
            if cond == Cond::IfOk && status != 0 {
                continue;
            }
            status = self.run(job);
        }
    }
}

fn close_all(pipes: &[[i32; 2]], extra: &[i32]) {
    for p in pipes {
        ulib::close(p[0] as u64);
        ulib::close(p[1] as u64);
    }
    for &fd in extra {
        ulib::close(fd as u64);
    }
}

fn main(_args: Args) -> i32 {
    let mut sh = Shell { jobs: Vec::new() };
    let mut input = LineReader::new(0);
    loop {
        sh.reap();
        print!("{}", PROMPT);
        let Some(line) = input.read_line() else {
            println!("exit");
            return 0;
        };
        match tokenize(&line).and_then(parse) {
            Ok(list) => sh.run_list(list),
            Err(e) => eprintln!("sh: {}", e),
        }
    }
}
