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
//!   `>> file`; a trailing `&` runs the job in the background;
//! - builtins: `cd`, `pwd`, `jobs`, `wait [pid]`, `kill pid...`,
//!   `exit [code]`, `help`;
//! - every job gets its own process group, made the console's foreground
//!   group while the shell waits for it, so Ctrl-C kills the job and not
//!   the shell;
//! - abnormal exits are reported (`[pid 7] segfault`), and finished
//!   background jobs before the next prompt.
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
            '|' | '<' | '>' | '&' => {
                if in_word {
                    toks.push(Tok::Word(core::mem::take(&mut cur)));
                    in_word = false;
                }
                toks.push(match c {
                    '|' => Tok::Pipe,
                    '<' => Tok::In,
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

fn parse(toks: Vec<Tok>) -> Result<Option<Job>, &'static str> {
    let mut stages = vec![Stage::default()];
    let mut background = false;
    let mut it = toks.into_iter().peekable();
    while let Some(t) = it.next() {
        if background {
            return Err("& must come last");
        }
        let st = stages.last_mut().unwrap();
        match t {
            Tok::Word(w) => st.argv.push(w),
            Tok::Pipe => {
                if st.argv.is_empty() {
                    return Err("empty pipeline stage");
                }
                stages.push(Stage::default());
            }
            Tok::In | Tok::Out | Tok::Append => {
                let Some(Tok::Word(path)) = it.next() else { return Err("missing file name after redirect") };
                match t {
                    Tok::In => st.stdin = Some(path),
                    Tok::Out => st.stdout = Some((path, false)),
                    _ => st.stdout = Some((path, true)),
                }
            }
            Tok::Amp => background = true,
        }
    }
    if stages.len() == 1 && stages[0].argv.is_empty() {
        return if background { Err("nothing to run in the background") } else { Ok(None) };
    }
    if stages.iter().any(|s| s.argv.is_empty()) {
        return Err("empty pipeline stage");
    }
    Ok(Some(Job { stages, background }))
}

struct Shell {
    /// Background jobs: (pid, command line).
    jobs: Vec<(i64, String)>,
}

fn report(pid: i64, status: i32, background: bool) {
    if status < 0 {
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
            self.jobs.retain(|(p, _)| *p != pid);
        }
    }

    fn wait_job(&mut self, pid: i64) {
        match ulib::wait_pid(pid) {
            Ok(status) => report(pid, status, true),
            Err(e) => eprintln!("sh: wait {}: {}", pid, e),
        }
        self.jobs.retain(|(p, _)| *p != pid);
    }

    /// Run a builtin. Returns `None` if `argv[0]` is not one.
    fn builtin(&mut self, argv: &[String]) -> Option<()> {
        let args: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
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
                for (pid, cmd) in &self.jobs {
                    let st = procs.iter().find(|p| p.pid as i64 == *pid);
                    let (state, cpu) = st.map_or(('?', 0), |p| (p.state, p.cpu));
                    let what = match state {
                        'R' => "running",
                        'Q' => "ready",
                        'S' => "sleeping",
                        _ => "exited",
                    };
                    println!("[pid {}] {} on cpu{}  {}", pid, what, cpu, cmd);
                }
            }
            "wait" => match args.get(1) {
                Some(p) => match p.parse::<i64>() {
                    Ok(pid) => self.wait_job(pid),
                    Err(_) => eprintln!("sh: wait: bad pid {}", p),
                },
                None => {
                    while let Some(&(pid, _)) = self.jobs.first() {
                        self.wait_job(pid);
                    }
                    self.reap();
                }
            },
            "kill" => {
                if args.len() < 2 {
                    eprintln!("usage: kill pid...");
                }
                for p in &args[1..] {
                    match p.parse::<i64>() {
                        Ok(pid) => {
                            let r = ulib::kill(pid);
                            if r < 0 {
                                eprintln!("sh: kill {}: {}", pid, Errno(-r));
                            }
                        }
                        Err(_) => eprintln!("sh: kill: bad pid {}", p),
                    }
                }
            }
            "help" => {
                println!("builtins: cd pwd jobs wait [pid] kill pid... exit [code] help");
                println!("syntax:   cmd args | cmd > file >> file < file &   'quotes' \"quotes\"");
                print!("programs:");
                if let Ok(es) = ulib::read_dir("/bin") {
                    for e in es {
                        print!(" {}", e.name);
                    }
                }
                println!();
                println!("keys:     Ctrl-C kills the foreground job, Ctrl-D ends input");
            }
            _ => return None,
        }
        Some(())
    }

    fn run(&mut self, line: &str, job: Job) {
        if job.stages.len() == 1 && !job.background && self.builtin(&job.stages[0].argv).is_some() {
            return;
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
                return;
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
                        return;
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
                            return;
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
                Err(e) if e.0 == ulib::errno::ENOENT => eprintln!("sh: {}: command not found", argv[0]),
                Err(e) => eprintln!("sh: {}: {}", argv[0], e),
            }
        }
        // Our copies of the pipe ends and files must go, or readers never
        // see end of file.
        close_all(&pipes, &[]);
        drop(files);
        if pids.is_empty() {
            return;
        }
        if job.background {
            for &pid in &pids {
                match cpu_of(pid) {
                    Some(c) => println!("[pid {}] started on cpu{}", pid, c),
                    None => println!("[pid {}] started", pid),
                }
                self.jobs.push((pid, String::from(line.trim().trim_end_matches('&').trim_end())));
            }
            return;
        }
        ulib::setfg(pgid);
        for &pid in &pids {
            match ulib::wait_pid(pid) {
                Ok(status) => report(pid, status, false),
                Err(e) => eprintln!("sh: wait {}: {}", pid, e),
            }
        }
        ulib::setfg(0);
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
            Ok(Some(job)) => sh.run(&line, job),
            Ok(None) => {}
            Err(e) => eprintln!("sh: {}", e),
        }
    }
}
