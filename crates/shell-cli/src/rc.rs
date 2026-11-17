use std::io::ErrorKind;
use std::path::PathBuf;

use shell_builtins::{Sink, run_program};
use shell_kernel::Session;

fn rc_path() -> Option<(PathBuf, bool)> {
    match std::env::var_os("KOA_RC") {
        Some(path) if path.is_empty() => None,
        Some(path) => Some((path.into(), true)),
        None => std::env::var_os("HOME").map(|home| (PathBuf::from(home).join(".koarc"), false)),
    }
}

fn continues(line: &str) -> bool {
    !line.trim_start().starts_with('#')
        && line.bytes().rev().take_while(|&b| b == b'\\').count() % 2 == 1
}

pub fn run_startup_file(session: &mut Session, sink: &Sink) -> Option<i32> {
    let (path, explicit) = rc_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if !explicit && e.kind() == ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("koa: {}: {e}", path.display());
            return None;
        }
    };
    let mut lines = text.lines().enumerate();
    while let Some((index, first)) = lines.next() {
        let mut command = first.to_string();
        while continues(&command)
            && let Some((_, next)) = lines.next()
        {
            command.pop();
            command.push_str(next);
        }
        let command = command.trim();
        if command.is_empty() {
            continue;
        }
        match session.parse(command) {
            Ok(program) => session.last_status = run_program(session, &program, sink),
            Err(err) => eprintln!(
                "koa: {}:{}: syntax error ({}): {}",
                path.display(),
                index + 1,
                err.code,
                err.message
            ),
        }
        if let Some(code) = session.should_exit {
            return Some(code);
        }
    }
    session.last_status = 0;
    None
}
