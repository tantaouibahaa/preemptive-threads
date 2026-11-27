//! `ls [-l] [path...]`: list directories (sorted). On the console names
//! are laid out in columns, otherwise one per line; `-l` adds kind and
//! size.
#![no_std]
#![no_main]

use ulib::prelude::*;
use ulib::Kind;

ulib::entry!(main);

fn show(path: &str, long: bool, header: bool, tty: bool) -> bool {
    let st = match ulib::stat(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ls: {}: {}", path, e);
            return false;
        }
    };
    let entries = if st.kind == Kind::Dir {
        match ulib::read_dir(path) {
            Ok(es) => es.into_iter().map(|e| (e.kind, e.name)).collect(),
            Err(e) => {
                eprintln!("ls: {}: {}", path, e);
                return false;
            }
        }
    } else {
        vec![(st.kind, String::from(path))]
    };
    if header {
        println!("{}:", path);
    }
    let full = |name: &str| if st.kind == Kind::Dir { format!("{}/{}", path.trim_end_matches('/'), name) } else { String::from(name) };
    if long {
        for (kind, name) in &entries {
            let (k, size) = match ulib::stat(&full(name)) {
                Ok(s) => (s.kind, s.size),
                Err(_) => (*kind, 0),
            };
            let (c, suffix) = match k {
                Kind::Dir => ('d', "/"),
                Kind::Device => ('c', ""),
                Kind::File => ('-', ""),
            };
            println!("{} {:>8}  {}{}", c, size, name, suffix);
        }
    } else if tty {
        let names: Vec<String> =
            entries.iter().map(|(k, n)| if *k == Kind::Dir { format!("{}/", n) } else { n.clone() }).collect();
        let width = names.iter().map(|n| n.len()).max().unwrap_or(0) + 2;
        let cols = (72 / width).max(1);
        let mut line = String::new();
        for (i, n) in names.iter().enumerate() {
            line.push_str(n);
            if (i + 1) % cols == 0 || i + 1 == names.len() {
                println!("{}", line.trim_end());
                line.clear();
            } else {
                for _ in n.len()..width {
                    line.push(' ');
                }
            }
        }
    } else {
        for (_, n) in &entries {
            println!("{}", n);
        }
    }
    true
}

fn main(args: Args) -> i32 {
    let mut long = false;
    let mut paths: Vec<&str> = Vec::new();
    for a in args.iter().skip(1) {
        match a {
            "-l" => long = true,
            p => paths.push(p),
        }
    }
    if paths.is_empty() {
        paths.push(".");
    }
    let tty = ulib::is_console(1);
    let mut ok = true;
    for (i, p) in paths.iter().enumerate() {
        if i > 0 {
            println!();
        }
        ok &= show(p, long, paths.len() > 1, tty);
    }
    if ok {
        0
    } else {
        1
    }
}
