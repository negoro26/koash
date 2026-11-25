use std::io::{self, BufRead};
use std::path::Path;
use std::process::ExitCode;

use shell_builtins::{Sink, run_program};
use shell_kernel::{CowWorkspace, Session, reap_background_jobs};
use shell_rpc::RpcServer;
use shell_surface::SurfaceMode;
use shell_tui::{LineEditor, ReadLineResult};

mod rc;

fn build_runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn print_help() {
    println!(
        "koa {} — agent-native structured shell (koash)

USAGE:
  koa                         Interactive TUI shell (or read script from stdin)
  koa --norc                  Interactive shell without reading the startup file
  koa -c <script>             Execute command string and exit (-lc/-ic also supported)
  koa --rpc | --acp           Run headless NDJSON-RPC or ACP server on stdio
  koa --socket <path>         Run headless NDJSON-RPC server on Unix socket
  koa --tsp                   Force Tern Surface Protocol (TSP) JSON frames
  koa -V, --version           Print version
  koa -h, --help              Print this help

BASH TRANSITION QUICK GUIDE:
  * POSIX pipes, &&, ||, ;, redirects, $VAR, $(...), and loops work directly.
  * Coreutils emit structured tables: ls, stat, find, ps, cat, grep, ast, env.
  * cat/grep/ast output Hashline anchors (file, line, hash, text) for `patch`.
  * Standard bash flags work: grep -rn -i -F -w -l -c --include, find -type/-name,
    head/tail -n N [file], ls -la, mkdir -p, rm -rf, cd -, cat --raw.
  * Table verbs: where <col op val>, select <cols...>, reject <cols...>,
    sort-by [--desc] <col>, reverse, first [N], last [N], skip [N],
    count/length, uniq [-c] [col], group-by [--asc] <col>, schema/describe,
    stats/measure [cols...], tee [-a] <files...>,
    from json|csv|tsv|lines, to json|compact|csv|tsv|md.
  * Undo & code intel: tx-log, undo, redo, rollback <id>, prewalk, ast, diag, refs, rename.
  * Startup file: interactive koa runs ~/.koarc (or $KOA_RC) line by line; -c and --rpc never do.
  * Shell state: alias/unalias, history [N], export, unset, set [-e|+e] [-o|+o pipefail].
  * Sub-kernels & discovery: @sql \"...\", @py \"...\", @js \"...\", discover \"<intent>\".",
        env!("CARGO_PKG_VERSION")
    );
}

fn find_command_flag(args: &[String]) -> Option<usize> {
    args.iter().position(|a| {
        a == "-c"
            || (a.starts_with('-')
                && !a.starts_with("--")
                && a.len() > 1
                && a.as_bytes()[1..]
                    .iter()
                    .all(|&b| matches!(b, b'c' | b'l' | b'i'))
                && a.as_bytes()[1..].contains(&b'c'))
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("koa {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    if args.iter().any(|a| a == "--acp") {
        let rt = match build_runtime() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("koa: runtime error: {e}");
                return ExitCode::from(1);
            }
        };
        return rt.block_on(async {
            let server = shell_rpc::AcpServer::new();
            match server.serve_stdio().await {
                Ok(()) => ExitCode::SUCCESS,
                Err(diag) => {
                    eprintln!("koa: {diag}");
                    ExitCode::from(1)
                }
            }
        });
    }

    if args.iter().any(|a| a == "--rpc") {
        let rt = match build_runtime() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("koa: runtime error: {e}");
                return ExitCode::from(1);
            }
        };
        return rt.block_on(async {
            let server = RpcServer::new();
            match server.serve_stdio().await {
                Ok(()) => ExitCode::SUCCESS,
                Err(diag) => {
                    eprintln!("koa: {diag}");
                    ExitCode::from(1)
                }
            }
        });
    }

    if let Some(pos) = args.iter().position(|a| a == "--socket") {
        let Some(sock_path) = args.get(pos + 1) else {
            eprintln!("koa: --socket requires a path");
            return ExitCode::from(2);
        };
        let path = Path::new(sock_path).to_path_buf();
        let rt = match build_runtime() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("koa: runtime error: {e}");
                return ExitCode::from(1);
            }
        };
        return rt.block_on(async move {
            let server = RpcServer::new();
            match server.serve_socket(&path).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(diag) => {
                    eprintln!("koa: {diag}");
                    ExitCode::from(1)
                }
            }
        });
    }

    let force_tsp = args.iter().any(|a| a == "--tsp");

    if let Ok(cwd) = std::env::current_dir() {
        CowWorkspace::sweep_orphans(&cwd);
    }

    if let Some(pos) = find_command_flag(&args) {
        if pos + 1 >= args.len() {
            eprintln!("koa: -c requires a command string");
            return ExitCode::from(2);
        }
        let script = args[pos + 1..].join(" ");
        let flag = |name: &str| {
            let at = args[..pos].iter().position(|a| a == name)?;
            args.get(at + 1)?.parse::<i32>().ok()
        };
        return run_oneshot(&script, force_tsp, flag("--last-status"), flag("--last-bg"));
    }

    let read_rc = !args.iter().any(|a| a == "--norc");
    run_interactive_repl(force_tsp, read_rc)
}

fn run_oneshot(
    script: &str,
    force_tsp: bool,
    last_status: Option<i32>,
    last_bg: Option<i32>,
) -> ExitCode {
    let mut session = match Session::new(false) {
        Ok(s) => s,
        Err(diag) => {
            eprintln!("koa: {diag}");
            return ExitCode::from(1);
        }
    };
    shell_kernel::forward_to_jobs(false);
    session.last_status = last_status.unwrap_or(0);
    if let Some(pid) = last_bg {
        session.env.set_last_background(pid);
    }

    let program = match session.parse(script) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("koa: syntax error ({}): {}", err.code, err.message);
            return ExitCode::from(2);
        }
    };

    let sink = Sink::Tty {
        mode: SurfaceMode::detect(force_tsp, false),
    };
    let code = run_program(&mut session, &program, &sink);
    ExitCode::from((code & 0xff) as u8)
}

fn report_finished_jobs(session: &mut Session) {
    for notice in reap_background_jobs(session) {
        eprintln!("{notice}");
    }
}

fn run_interactive_repl(force_tsp: bool, read_rc: bool) -> ExitCode {
    let mut session = match Session::new(true) {
        Ok(s) => s,
        Err(diag) => {
            eprintln!("koa: {diag}");
            return ExitCode::from(1);
        }
    };
    shell_kernel::forward_to_jobs(session.interactive);

    if session.interactive {
        let mut editor = LineEditor::new();
        let sink = Sink::Tty {
            mode: SurfaceMode::detect(force_tsp, true),
        };
        if read_rc && let Some(code) = rc::run_startup_file(&mut session, &sink) {
            return ExitCode::from((code & 0xff) as u8);
        }
        loop {
            report_finished_jobs(&mut session);
            let prompt = format!(
                "{} koa> ",
                session
                    .cwd
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("/")
            );
            match editor.read_line(&prompt, &session.cwd) {
                Ok(ReadLineResult::Eof) => break,
                Ok(ReadLineResult::Line(raw)) => {
                    let trimmed = raw.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let code = match session.parse(trimmed) {
                        Ok(program) => run_program(&mut session, &program, &sink),
                        Err(err) => {
                            eprintln!("koa: syntax error ({}): {}", err.code, err.message);
                            2
                        }
                    };
                    session.last_status = code;
                    editor
                        .history
                        .append(trimmed, &session.cwd.to_string_lossy(), code);
                    if let Some(exit_code) = session.should_exit {
                        return ExitCode::from((exit_code & 0xff) as u8);
                    }
                }
                Err(diag) => {
                    eprintln!("koa: {diag}");
                    return ExitCode::from(1);
                }
            }
        }
        return ExitCode::from((session.last_status & 0xff) as u8);
    }

    let stdin = io::stdin();
    let sink = Sink::Tty {
        mode: SurfaceMode::detect(force_tsp, false),
    };
    let mut line = String::new();

    loop {
        report_finished_jobs(&mut session);
        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                session.last_status = match session.parse(trimmed) {
                    Ok(program) => run_program(&mut session, &program, &sink),
                    Err(err) => {
                        eprintln!("koa: syntax error ({}): {}", err.code, err.message);
                        2
                    }
                };
                if let Some(code) = session.should_exit {
                    return ExitCode::from((code & 0xff) as u8);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                continue;
            }
            Err(e) => {
                eprintln!("koa: read error: {e}");
                return ExitCode::from(1);
            }
        }
    }

    ExitCode::from((session.last_status & 0xff) as u8)
}
