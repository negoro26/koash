//! Prototype: runs `Command::PosixFallback` lines in an in-process brush shell instead of `sh -c`.
//!
//! Koa keeps ownership of cwd, env, status, undo ledger and every child process. Brush only evaluates bash.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::Scope;
use std::time::Duration;

use brush_builtins::{BuiltinSet, ShellBuilderExt};
use brush_core::env::{EnvironmentLookup, EnvironmentScope};
use brush_core::extensions::{
    DefaultErrorFormatter, DefaultTaskSpawner, ProcessLauncher, ShellExtensionsImpl,
};
use brush_core::openfiles::OpenFile;
use brush_core::parser::ast;
use brush_core::processes::{ChildProcess, LaunchRequest, ProcessGroup};
use brush_core::traps::TrapSignal;
use brush_core::variables::ShellValueLiteral;
use brush_core::{ExecutionResult, ProfileLoadBehavior, RcLoadBehavior, Shell, SourceInfo};
use brush_parser::word::{Parameter, ParameterExpr, TildeExpr, WordPiece, WordPieceWithSource};
use nix::unistd::Pid;
use shell_kernel::{ChildGroup, KillSwitch, ResolvedSpawn, Session, SessionEnv, spawn_resolved};
use shell_syntax::Span;
use shell_value::StructuredDiagnostic;

use crate::sink::{Chunk, Sink};

static SPAWNED: AtomicUsize = AtomicUsize::new(0);

/// Number of external processes that went through [`KoaLauncher::spawn`] in this process.
pub fn spawned_total() -> usize {
    SPAWNED.load(Ordering::SeqCst)
}

#[derive(Default)]
struct RunCtx {
    kill: Option<KillSwitch>,
    group_children: bool,
    spawned: Vec<String>,
}

#[derive(Clone, Default)]
pub struct KoaLauncher(Arc<Mutex<RunCtx>>);

impl KoaLauncher {
    fn begin_run(&self, kill: KillSwitch, group_children: bool) {
        if let Ok(mut ctx) = self.0.lock() {
            *ctx = RunCtx {
                kill: Some(kill),
                group_children,
                spawned: Vec::new(),
            };
        }
    }

    fn take_spawned(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut ctx| std::mem::take(&mut ctx.spawned))
            .unwrap_or_default()
    }
}

fn policy_check(program: &str, argv0: &str) -> io::Result<()> {
    let Ok(deny) = std::env::var("KOA_DENY_EXEC") else {
        return Ok(());
    };
    let base = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    if deny.split(',').any(|d| d == base || d == argv0) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("`{argv0}` blocked by policy"),
        ));
    }
    Ok(())
}

fn trace_line(line: &str) {
    let Some(path) = std::env::var_os("KOA_TRACE_SPAWN") else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

fn trace_spawn(request: &LaunchRequest) {
    trace_line(&format!(
        "SPAWN\t{}\t{}",
        request.program.display(),
        request.args.join(" ")
    ));
}

fn stdio_for(request: &LaunchRequest, fd: i32) -> io::Result<Stdio> {
    match request.open_files.try_fd(fd) {
        None => Ok(Stdio::null()),
        Some(file) => {
            let borrowed = file.try_borrow_as_fd().map_err(io::Error::other)?;
            Ok(Stdio::from(borrowed.try_clone_to_owned()?))
        }
    }
}

impl ProcessLauncher for KoaLauncher {
    fn spawn(&self, request: LaunchRequest) -> Result<ChildProcess, io::Error> {
        let program = request.program.to_string_lossy().into_owned();
        policy_check(&program, &request.argv0)?;
        SPAWNED.fetch_add(1, Ordering::SeqCst);
        trace_spawn(&request);

        let (kill, grouped) = {
            let mut ctx = self.0.lock().map_err(|_| io::Error::other("launcher state"))?;
            ctx.spawned.push(request.argv0.clone());
            (ctx.kill.clone(), ctx.group_children)
        };

        // Builtin output sits in std's line buffer; a child writing to the same fd would overtake it.
        let _ = io::stdout().flush();

        let group = match request.process_group {
            _ if grouped => ChildGroup::Leader,
            ProcessGroup::Inherit => ChildGroup::Shared,
            ProcessGroup::Join(pgid) => ChildGroup::Join(Pid::from_raw(pgid)),
            _ => ChildGroup::Leader,
        };
        let mut child = spawn_resolved(ResolvedSpawn {
            program: &program,
            argv0: Some(&request.argv0),
            args: &request.args,
            cwd: &request.working_dir,
            env: &request.env,
            stdin: stdio_for(&request, 0)?,
            stdout: stdio_for(&request, 1)?,
            stderr: stdio_for(&request, 2)?,
            group,
            tty: None,
        })?;

        let tracked = match (&kill, group) {
            (Some(kill), ChildGroup::Leader) => {
                let pgid = Pid::from_raw(child.id() as i32);
                kill.track(pgid);
                Some((kill.clone(), pgid))
            }
            _ => None,
        };
        let waited = async move {
            let status = tokio::task::spawn_blocking(move || child.wait())
                .await
                .map_err(io::Error::other)??;
            if let Some((kill, pgid)) = tracked {
                kill.untrack(pgid);
            }
            Ok(exit_result(status))
        };
        Ok(ChildProcess::from_future(waited))
    }
}

fn exit_result(status: std::process::ExitStatus) -> ExecutionResult {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        return ExecutionResult::new((code & 0xff) as u8);
    }
    ExecutionResult::new(status.signal().map_or(127, |s| (s & 0xff) as u8 + 128))
}

type KoaExt = ShellExtensionsImpl<DefaultErrorFormatter, KoaLauncher, DefaultTaskSpawner>;

struct Host {
    shell: Option<Shell<KoaExt>>,
    launcher: KoaLauncher,
    pushed_env: Vec<String>,
}

fn runtime() -> Result<&'static tokio::runtime::Runtime, StructuredDiagnostic> {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    if let Some(rt) = RT.get() {
        return Ok(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| StructuredDiagnostic::from_io("E_RUNTIME", &e, None))?;
    Ok(RT.get_or_init(|| rt))
}

fn diag_from_brush(code: &str, err: impl std::fmt::Display) -> StructuredDiagnostic {
    StructuredDiagnostic::new(code, err.to_string(), None, None)
}

impl Host {
    async fn new(cwd: &Path, launcher: KoaLauncher) -> Result<Shell<KoaExt>, StructuredDiagnostic> {
        Shell::builder_with_extensions::<KoaExt>()
            .default_builtins(BuiltinSet::BashMode)
            .process_launcher(launcher)
            .do_not_inherit_env(true)
            .working_dir(cwd.to_path_buf())
            .shell_name("sh".to_string())
            .profile(ProfileLoadBehavior::Skip)
            .rc(RcLoadBehavior::Skip)
            .no_editing(true)
            .kill_external_commands_on_drop(true)
            .build()
            .await
            .map_err(|e| diag_from_brush("E_BRUSH_INIT", e))
    }

    fn sync_in(&mut self, session: &Session) -> Result<(), StructuredDiagnostic> {
        let Some(shell) = self.shell.as_mut() else {
            return Ok(());
        };
        shell
            .set_working_dir(&session.cwd)
            .map_err(|e| diag_from_brush("E_CWD", e))?;
        let wanted = session.env.materialize();
        for stale in self
            .pushed_env
            .iter()
            .filter(|k| !wanted.contains_key(*k))
        {
            let _ = shell.env_mut().unset(stale);
        }
        for (key, value) in &wanted {
            if exported_value(shell, key).as_deref() == Some(value.as_str()) {
                continue;
            }
            let _ = shell.env_mut().update_or_add(
                key.clone(),
                ShellValueLiteral::Scalar(value.clone()),
                |var| {
                    var.export();
                    Ok(())
                },
                EnvironmentLookup::Anywhere,
                EnvironmentScope::Global,
            );
        }
        self.pushed_env = wanted.into_keys().collect();
        shell.set_last_exit_status((session.last_status & 0xff) as u8);
        Ok(())
    }
}

fn exported_value(shell: &Shell<KoaExt>, name: &str) -> Option<String> {
    shell
        .env()
        .get(name)
        .filter(|(_, var)| var.is_exported())
        .and_then(|_| shell.env_str(name).map(|v| v.into_owned()))
}

fn exported_snapshot(shell: &Shell<KoaExt>) -> BTreeMap<String, String> {
    let names: Vec<String> = shell
        .env()
        .iter_exported()
        .map(|(name, _)| name.clone())
        .collect();
    names
        .into_iter()
        .filter_map(|name| shell.env_str(&name).map(|v| (name.clone(), v.into_owned())))
        .collect()
}

fn sync_out(session: &mut Session, shell: &Shell<KoaExt>, before: &BTreeMap<String, String>) {
    let after = exported_snapshot(shell);
    for (key, value) in &after {
        if before.get(key) != Some(value) {
            session.env.insert(key.clone(), value.clone());
        }
    }
    for key in before.keys().filter(|k| !after.contains_key(*k)) {
        session.env.remove(key);
    }
    session.cwd = shell.working_dir().to_path_buf();
}

fn parse_diag(err: &brush_core::parser::ParseError, len: usize) -> StructuredDiagnostic {
    use brush_core::parser::ParseError;
    let at = |offset: usize| Span::new(offset.min(len), (offset + 1).min(len));
    let span = match err {
        ParseError::ParsingNear(pos) => Some(at(pos.byte_offset)),
        ParseError::ParsingAtEndOfInput => Some(Span::new(len, len)),
        ParseError::Tokenizing { position, .. } => position.as_ref().map(|p| at(p.byte_offset)),
        _ => None,
    };
    StructuredDiagnostic::new(
        "E_SYNTAX",
        err.to_string(),
        span,
        Some("bash syntax error in an embedded line".to_string()),
    )
}

/// Runs one raw bash line through the session's brush shell. Returns the exit status.
pub fn run_embedded(session: &mut Session, raw: &str, sink: &Sink) -> i32 {
    trace_line(&format!("EMBED\t{raw}"));
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(diag) => {
            sink.diag(&diag);
            return 1;
        }
    };
    let mut host = match session
        .embedded
        .take()
        .and_then(|b| b.downcast::<Host>().ok())
    {
        Some(host) => *host,
        None => Host {
            shell: None,
            launcher: KoaLauncher::default(),
            pushed_env: Vec::new(),
        },
    };
    if host.shell.is_none() {
        match rt.block_on(Host::new(&session.cwd, host.launcher.clone())) {
            Ok(shell) => host.shell = Some(shell),
            Err(diag) => {
                sink.diag(&diag);
                return 1;
            }
        }
    }
    let status = run_with_host(rt, &mut host, session, raw, sink);
    session.embedded = Some(Box::new(host));
    status
}

fn run_with_host(
    rt: &tokio::runtime::Runtime,
    host: &mut Host,
    session: &mut Session,
    raw: &str,
    sink: &Sink,
) -> i32 {
    let program = {
        let Some(shell) = host.shell.as_ref() else {
            return 1;
        };
        match shell.parse_string(raw) {
            Ok(program) => program,
            Err(err) => {
                sink.diag(&parse_diag(&err, raw.len()));
                return 2;
            }
        }
    };

    if let Some(shell) = host.shell.as_ref() {
        let env = session.env.materialize();
        let targets = Walker::new(shell, &env, &session.cwd, raw).walk(&program);
        record_targets(session, raw, targets);
    }

    if let Err(diag) = host.sync_in(session) {
        sink.diag(&diag);
        return 1;
    }
    let Some(shell) = host.shell.take() else {
        return 1;
    };
    let before = exported_snapshot(&shell);
    host.launcher
        .begin_run(session.kill.clone(), sink.is_capture());

    let (shell, status) = std::thread::scope(|scope| {
        let mut fds: Vec<(i32, OpenFile)> = Vec::new();
        if let Sink::Capture(_) = sink {
            if let Ok(null) = brush_core::openfiles::null() {
                fds.push((0, null));
            }
            for (fd, make) in [
                (1, Chunk::Stdout as fn(Vec<u8>) -> Chunk),
                (2, Chunk::Stderr as fn(Vec<u8>) -> Chunk),
            ] {
                if let Ok((reader, writer)) = io::pipe() {
                    drain(scope, sink, OwnedFd::from(reader), make);
                    fds.push((fd, OpenFile::from(writer)));
                }
            }
        }
        run_task(rt, shell, raw.to_string(), fds, session.kill.clone())
    });

    for label in host.launcher.take_spawned() {
        session.mark_opaque(label);
    }
    match shell {
        Some(shell) => {
            sync_out(session, &shell, &before);
            host.shell = Some(shell);
        }
        None => host.pushed_env.clear(),
    }
    status
}

fn drain<'scope>(
    scope: &'scope Scope<'scope, '_>,
    sink: &'scope Sink,
    fd: OwnedFd,
    make: fn(Vec<u8>) -> Chunk,
) {
    scope.spawn(move || {
        let mut reader = std::fs::File::from(fd);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink.chunk(make(buf[..n].to_vec())),
            }
        }
    });
}

fn run_task(
    rt: &tokio::runtime::Runtime,
    shell: Shell<KoaExt>,
    raw: String,
    fds: Vec<(i32, OpenFile)>,
    kill: KillSwitch,
) -> (Option<Shell<KoaExt>>, i32) {
    let task = async move {
        let mut shell = shell;
        let mut params = shell.default_exec_params();
        for (fd, file) in fds {
            params.set_fd(fd, file);
        }
        shell.start_command_string_mode();
        let result = shell
            .run_string(raw, &SourceInfo::from("-c"), &params)
            .await;
        let _ = shell.end_command_string_mode();
        let mut status = match result {
            Ok(result) => u8::from(result.exit_code),
            Err(err) => {
                let _ = writeln!(params.stderr(&shell), "koa: {err}");
                1
            }
        };
        if let Some(command) = shell
            .traps()
            .get_handler(TrapSignal::Exit)
            .map(|h| h.command.clone())
        {
            shell.traps_mut().remove_handlers(TrapSignal::Exit);
            if let Ok(trap) = shell
                .run_string(command, &SourceInfo::from("trap"), &params)
                .await
                && trap.is_exit()
            {
                status = u8::from(trap.exit_code);
            }
        }
        (shell, i32::from(status))
    };
    rt.block_on(async move {
        let mut handle = tokio::spawn(task);
        loop {
            tokio::select! {
                joined = &mut handle => {
                    return match joined {
                        Ok((shell, status)) => (Some(shell), status),
                        Err(_) => (None, 1),
                    };
                }
                _ = tokio::time::sleep(Duration::from_millis(25)) => {
                    if kill.is_killed() {
                        handle.abort();
                        return (None, crate::pipeline::KILLED_STATUS);
                    }
                }
            }
        }
    })
}

enum Target {
    Write(PathBuf),
    Unknown(String),
}

fn record_targets(session: &mut Session, raw: &str, targets: Vec<Target>) {
    for target in targets {
        match target {
            Target::Write(path) => {
                session.record_output_target(&path);
                session.record_mutation(path.to_string_lossy().to_string());
            }
            Target::Unknown(why) => session.mark_opaque(format!("{raw}: {why}")),
        }
    }
}

#[derive(Clone, PartialEq, Default)]
struct State {
    vars: HashMap<String, Option<String>>,
    cwd: Option<PathBuf>,
    dynamic: bool,
}

fn join(a: &State, b: &State) -> State {
    let mut vars = HashMap::new();
    for key in a.vars.keys().chain(b.vars.keys()) {
        if vars.contains_key(key) {
            continue;
        }
        let (left, right) = (a.vars.get(key), b.vars.get(key));
        vars.insert(
            key.clone(),
            if left == right {
                left.cloned().flatten()
            } else {
                None
            },
        );
    }
    State {
        vars,
        cwd: if a.cwd == b.cwd { a.cwd.clone() } else { None },
        dynamic: a.dynamic || b.dynamic,
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn idents(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_alphanumeric() || c == '_' {
            cur.push(c);
        } else {
            if is_ident(&cur) {
                out.push(cur.clone());
            }
            cur.clear();
        }
    }
    out
}

const DYNAMIC_VARS: [&str; 14] = [
    "RANDOM", "SECONDS", "LINENO", "BASHPID", "PPID", "UID", "EUID", "HOSTNAME", "PWD", "OLDPWD",
    "REPLY", "OPTARG", "OPTIND", "EPOCHSECONDS",
];

/// Collects every path a bash line may write, over-approximating where the text cannot say.
struct Walker<'a> {
    shell: &'a Shell<KoaExt>,
    env: &'a BTreeMap<String, String>,
    opts: brush_parser::ParserOptions,
    funcs: HashMap<String, ast::FunctionBody>,
    active: HashSet<String>,
    targets: Vec<Target>,
    entry_cwd: PathBuf,
    has_cd: bool,
}

impl<'a> Walker<'a> {
    fn new(
        shell: &'a Shell<KoaExt>,
        env: &'a BTreeMap<String, String>,
        cwd: &Path,
        raw: &str,
    ) -> Self {
        Self {
            shell,
            env,
            opts: shell.parser_options(),
            funcs: HashMap::new(),
            active: HashSet::new(),
            targets: Vec::new(),
            entry_cwd: cwd.to_path_buf(),
            has_cd: idents(raw)
                .iter()
                .any(|w| matches!(w.as_str(), "cd" | "pushd" | "popd")),
        }
    }

    fn walk(mut self, program: &ast::Program) -> Vec<Target> {
        let mut st = State {
            cwd: Some(self.entry_cwd.clone()),
            ..State::default()
        };
        for list in &program.complete_commands {
            self.list(list, &mut st);
        }
        self.targets
    }

    fn unknown(&mut self, why: impl Into<String>) {
        self.targets.push(Target::Unknown(why.into()));
    }

    fn walk_text(&mut self, text: &str, st: &mut State) {
        match self.shell.parse_string(text) {
            Ok(program) => {
                for list in &program.complete_commands {
                    self.list(list, st);
                }
            }
            Err(_) => self.unknown("unparsable embedded program"),
        }
    }

    fn subshell(&mut self, list: &ast::CompoundList, st: &State) {
        let mut copy = st.clone();
        self.list(list, &mut copy);
    }

    fn list(&mut self, list: &ast::CompoundList, st: &mut State) {
        for ast::CompoundListItem(and_or, sep) in &list.0 {
            if matches!(sep, ast::SeparatorOperator::Async) {
                let mut copy = st.clone();
                self.and_or(and_or, &mut copy);
            } else {
                self.and_or(and_or, st);
            }
        }
    }

    fn and_or(&mut self, list: &ast::AndOrList, st: &mut State) {
        self.pipeline(&list.first, st);
        for next in &list.additional {
            let (ast::AndOr::And(p) | ast::AndOr::Or(p)) = next;
            let mut branch = st.clone();
            self.pipeline(p, &mut branch);
            *st = join(st, &branch);
        }
    }

    fn pipeline(&mut self, pipeline: &ast::Pipeline, st: &mut State) {
        if let [only] = pipeline.seq.as_slice() {
            self.command(only, st);
            return;
        }
        for cmd in &pipeline.seq {
            let mut copy = st.clone();
            self.command(cmd, &mut copy);
        }
    }

    fn command(&mut self, cmd: &ast::Command, st: &mut State) {
        match cmd {
            ast::Command::Simple(simple) => self.simple(simple, st),
            ast::Command::Compound(compound, redirects) => {
                if let Some(redirects) = redirects {
                    for r in &redirects.0 {
                        self.redirect(r, st);
                    }
                }
                self.compound(compound, st);
            }
            ast::Command::Function(def) => {
                if let Some(name) = self.expand(&def.fname.value, st) {
                    self.funcs.insert(name, def.body.clone());
                }
            }
        }
    }

    fn fixpoint(&mut self, st: &mut State, mut body: impl FnMut(&mut Self, &mut State)) {
        for _ in 0..8 {
            let before = st.clone();
            body(self, st);
            let merged = join(&before, st);
            let stable = merged == before;
            *st = merged;
            if stable {
                return;
            }
        }
        for value in st.vars.values_mut() {
            *value = None;
        }
        st.cwd = None;
    }

    fn compound(&mut self, cmd: &ast::CompoundCommand, st: &mut State) {
        use ast::CompoundCommand as C;
        match cmd {
            C::BraceGroup(g) => self.list(&g.list, st),
            C::Subshell(s) => self.subshell(&s.list, st),
            C::Arithmetic(a) => self.taint(&a.expr.value, st),
            C::ArithmeticForClause(f) => {
                for expr in [&f.initializer, &f.condition, &f.updater].into_iter().flatten() {
                    self.taint(&expr.value, st);
                }
                self.fixpoint(st, |me, st| me.list(&f.body.list, st));
            }
            C::ForClause(f) => self.for_clause(f, st),
            C::CaseClause(c) => {
                self.expand(&c.value.value, st);
                let mut merged = st.clone();
                for item in &c.cases {
                    for pattern in &item.patterns {
                        self.expand(&pattern.value, st);
                    }
                    if let Some(body) = &item.cmd {
                        let mut branch = st.clone();
                        self.list(body, &mut branch);
                        merged = join(&merged, &branch);
                    }
                }
                *st = merged;
            }
            C::IfClause(i) => {
                self.list(&i.condition, st);
                let mut merged = st.clone();
                let mut then = st.clone();
                self.list(&i.then, &mut then);
                merged = join(&merged, &then);
                for e in i.elses.iter().flatten() {
                    let mut branch = st.clone();
                    if let Some(cond) = &e.condition {
                        self.list(cond, &mut branch);
                    }
                    self.list(&e.body, &mut branch);
                    merged = join(&merged, &branch);
                }
                *st = merged;
            }
            C::WhileClause(w) | C::UntilClause(w) => {
                self.fixpoint(st, |me, st| {
                    me.list(&w.0, st);
                    me.list(&w.1.list, st);
                });
            }
            C::Coprocess(c) => {
                let mut copy = st.clone();
                self.command(&c.body, &mut copy);
            }
            C::ExtendedTest(t) => self.test_expr(&t.expr, st),
        }
    }

    fn test_expr(&mut self, expr: &ast::ExtendedTestExpr, st: &mut State) {
        use ast::ExtendedTestExpr as E;
        match expr {
            E::And(a, b) | E::Or(a, b) => {
                self.test_expr(a, st);
                self.test_expr(b, st);
            }
            E::Not(e) | E::Parenthesized(e) => self.test_expr(e, st),
            E::UnaryTest(_, w) => {
                self.expand(&w.value, st);
            }
            E::BinaryTest(_, a, b) => {
                self.expand(&a.value, st);
                self.expand(&b.value, st);
            }
        }
    }

    fn for_clause(&mut self, f: &ast::ForClauseCommand, st: &mut State) {
        let literal = f.values.as_ref().filter(|values| {
            values.len() <= 64
                && values.iter().all(|w| {
                    !w.value
                        .chars()
                        .any(|c| matches!(c, '$' | '`' | '*' | '?' | '[' | '{' | '~' | '\\'))
                })
        });
        let Some(values) = literal else {
            for w in f.values.iter().flatten() {
                self.expand(&w.value, st);
            }
            st.vars.insert(f.variable_name.clone(), None);
            self.fixpoint(st, |me, st| me.list(&f.body.list, st));
            return;
        };
        for w in values {
            let value = self.expand(&w.value, st);
            st.vars.insert(f.variable_name.clone(), value);
            self.list(&f.body.list, st);
        }
    }

    fn taint(&mut self, expr: &str, st: &mut State) {
        for name in idents(expr) {
            st.vars.insert(name, None);
        }
    }

    fn lookup(&self, name: &str, st: &State) -> Option<String> {
        if let Some(entry) = st.vars.get(name) {
            return entry.clone();
        }
        if st.dynamic || DYNAMIC_VARS.contains(&name) || name.starts_with("BASH") {
            return None;
        }
        Some(self.env.get(name).cloned().unwrap_or_default())
    }

    fn resolve(&self, text: &str, st: &State) -> Option<PathBuf> {
        let path = Path::new(text);
        if path.is_absolute() {
            return Some(path.to_path_buf());
        }
        st.cwd.as_ref().map(|cwd| cwd.join(path))
    }

    fn expand(&mut self, text: &str, st: &mut State) -> Option<String> {
        let pieces = brush_parser::word::parse(text, &self.opts).ok()?;
        self.pieces(&pieces, st)
    }

    fn pieces(&mut self, pieces: &[WordPieceWithSource], st: &mut State) -> Option<String> {
        let mut out = String::new();
        let mut known = true;
        for piece in pieces {
            match &piece.piece {
                WordPiece::Text(t) | WordPiece::SingleQuotedText(t) => out.push_str(t),
                WordPiece::EscapeSequence(s) => out.push_str(s.strip_prefix('\\').unwrap_or(s)),
                WordPiece::DoubleQuotedSequence(inner)
                | WordPiece::GettextDoubleQuotedSequence(inner) => match self.pieces(inner, st) {
                    Some(s) => out.push_str(&s),
                    None => known = false,
                },
                WordPiece::TildeExpansion(TildeExpr::Home) => match self.env.get("HOME") {
                    Some(home) if !st.dynamic => out.push_str(home),
                    _ => known = false,
                },
                WordPiece::ParameterExpansion(expr) => match self.parameter(expr, st) {
                    Some(s) => out.push_str(&s),
                    None => known = false,
                },
                WordPiece::CommandSubstitution(text)
                | WordPiece::BackquotedCommandSubstitution(text) => {
                    let mut copy = st.clone();
                    self.walk_text(text, &mut copy);
                    known = false;
                }
                WordPiece::ArithmeticExpression(expr) => {
                    self.taint(&expr.value, st);
                    known = false;
                }
                _ => known = false,
            }
        }
        known.then_some(out)
    }

    fn parameter(&mut self, expr: &ParameterExpr, st: &mut State) -> Option<String> {
        match expr {
            ParameterExpr::Parameter {
                parameter: Parameter::Named(name),
                indirect: false,
            } => self.lookup(name, st),
            ParameterExpr::AssignDefaultValues {
                parameter: Parameter::Named(name),
                ..
            } => {
                st.vars.insert(name.clone(), None);
                None
            }
            _ => None,
        }
    }

    fn redirect(&mut self, redirect: &ast::IoRedirect, st: &mut State) {
        use ast::IoFileRedirectKind as K;
        use ast::IoFileRedirectTarget as T;
        match redirect {
            ast::IoRedirect::File(_, kind, target) => match target {
                T::Filename(word) => {
                    let text = self.expand(&word.value, st);
                    if matches!(kind, K::Write | K::Append | K::ReadAndWrite | K::Clobber) {
                        self.write_target(text, &word.value, st);
                    }
                }
                T::Duplicate(word) => {
                    let text = self.expand(&word.value, st);
                    if matches!(kind, K::DuplicateOutput) {
                        match text {
                            Some(t) if t == "-" || t.chars().all(|c| c.is_ascii_digit()) => {}
                            other => self.write_target(other, &word.value, st),
                        }
                    }
                }
                T::Fd(_) => {}
                T::ProcessSubstitution(_, sub) => self.subshell(&sub.list, st),
            },
            ast::IoRedirect::OutputAndError(word, _) => {
                let text = self.expand(&word.value, st);
                self.write_target(text, &word.value, st);
            }
            ast::IoRedirect::HereString(_, word) => {
                self.expand(&word.value, st);
            }
            ast::IoRedirect::HereDocument(_, doc) => {
                if doc.requires_expansion
                    && let Ok(pieces) = brush_parser::word::parse_heredoc(&doc.doc.value, &self.opts)
                {
                    self.pieces(&pieces, st);
                }
            }
        }
    }

    fn write_target(&mut self, text: Option<String>, raw: &str, st: &State) {
        let Some(text) = text else {
            self.unknown(format!("redirect target `{raw}` is not statically known"));
            return;
        };
        if text.is_empty() {
            return;
        }
        match self.resolve(&text, st) {
            Some(path) => self.targets.push(Target::Write(path)),
            None => self.unknown(format!("redirect target `{raw}` depends on a changed cwd")),
        }
    }

    fn simple(&mut self, cmd: &ast::SimpleCommand, st: &mut State) {
        let mut assigns: Vec<&ast::Assignment> = Vec::new();
        let mut words: Vec<(String, Option<String>)> = Vec::new();
        let items = cmd
            .prefix
            .iter()
            .flat_map(|p| p.0.iter())
            .map(|i| (true, i))
            .chain(cmd.suffix.iter().flat_map(|s| s.0.iter()).map(|i| (false, i)));
        let mut name_pending = true;
        for (is_prefix, item) in items {
            if !is_prefix && name_pending {
                name_pending = false;
                if let Some(name) = &cmd.word_or_name {
                    let expanded = self.expand(&name.value, st);
                    words.push((name.value.clone(), expanded));
                }
            }
            match item {
                ast::CommandPrefixOrSuffixItem::IoRedirect(r) => self.redirect(r, st),
                ast::CommandPrefixOrSuffixItem::Word(w) => {
                    let expanded = self.expand(&w.value, st);
                    words.push((w.value.clone(), expanded));
                }
                ast::CommandPrefixOrSuffixItem::AssignmentWord(a, w) => {
                    if is_prefix {
                        assigns.push(a);
                    } else {
                        let expanded = self.expand(&w.value, st);
                        words.push((w.value.clone(), expanded));
                    }
                }
                ast::CommandPrefixOrSuffixItem::ProcessSubstitution(_, sub) => {
                    self.subshell(&sub.list, st);
                }
            }
        }
        if name_pending && let Some(name) = &cmd.word_or_name {
            let expanded = self.expand(&name.value, st);
            words.insert(0, (name.value.clone(), expanded));
        }
        if words.is_empty() {
            for a in assigns {
                self.assign(a, st);
            }
            return;
        }
        self.named(&words, st);
    }

    fn assign(&mut self, a: &ast::Assignment, st: &mut State) {
        let ast::AssignmentName::VariableName(name) = &a.name else {
            return;
        };
        match &a.value {
            ast::AssignmentValue::Scalar(w) if !a.append => {
                let value = self.expand(&w.value, st);
                st.vars.insert(name.clone(), value);
            }
            ast::AssignmentValue::Scalar(w) => {
                self.expand(&w.value, st);
                st.vars.insert(name.clone(), None);
            }
            ast::AssignmentValue::Array(items) => {
                for (key, value) in items {
                    if let Some(key) = key {
                        self.expand(&key.value, st);
                    }
                    self.expand(&value.value, st);
                }
                st.vars.insert(name.clone(), None);
            }
        }
    }

    fn named(&mut self, words: &[(String, Option<String>)], st: &mut State) {
        let Some(name) = words[0].1.clone() else {
            self.unknown("command name is not statically known");
            return;
        };
        let args = &words[1..];
        if self.shell.aliases().contains_key(&name) {
            self.unknown(format!("alias `{name}`"));
            return;
        }
        match name.as_str() {
            "cd" => {
                let operand = args.iter().find(|(raw, _)| !raw.starts_with('-') || raw == "-");
                st.cwd = match operand {
                    None => None,
                    Some((_, Some(dir))) if dir != "-" => self.resolve(dir, st),
                    Some(_) => None,
                };
            }
            "pushd" | "popd" => st.cwd = None,
            "eval" => {
                if args.iter().all(|(_, v)| v.is_some()) {
                    let text = args
                        .iter()
                        .filter_map(|(_, v)| v.clone())
                        .collect::<Vec<_>>()
                        .join(" ");
                    self.walk_text(&text, st);
                } else {
                    self.unknown("eval of a dynamic string");
                }
            }
            "." | "source" => self.unknown("source of a file"),
            "trap" => {
                let action = args
                    .iter()
                    .find(|(raw, _)| !raw.starts_with('-'))
                    .map(|(_, v)| v.clone());
                match action {
                    Some(Some(text)) => {
                        let mut later = State {
                            dynamic: true,
                            cwd: (!self.has_cd).then(|| self.entry_cwd.clone()),
                            ..State::default()
                        };
                        self.walk_text(&text, &mut later);
                    }
                    Some(None) => self.unknown("trap action is dynamic"),
                    None => {}
                }
            }
            "export" | "declare" | "typeset" | "local" | "readonly" => {
                for (raw, value) in args {
                    if raw.starts_with('-') {
                        continue;
                    }
                    match raw.split_once('=') {
                        Some((var, _)) if is_ident(var) => {
                            let literal = value
                                .as_ref()
                                .and_then(|v| v.split_once('='))
                                .map(|(_, v)| v.to_string());
                            st.vars.insert(var.to_string(), literal);
                        }
                        Some((var, _)) => {
                            if let Some(base) = var.strip_suffix('+')
                                && is_ident(base)
                            {
                                st.vars.insert(base.to_string(), None);
                            }
                        }
                        None => {}
                    }
                }
            }
            "read" | "mapfile" | "readarray" | "getopts" | "let" | "unset" => {
                st.vars.insert("REPLY".to_string(), None);
                for (raw, _) in args {
                    for ident in idents(raw) {
                        st.vars.insert(ident, None);
                    }
                }
            }
            "printf" => {
                if let Some(pos) = args.iter().position(|(raw, _)| raw == "-v")
                    && let Some((raw, _)) = args.get(pos + 1)
                {
                    st.vars.insert(raw.clone(), None);
                }
            }
            "command" | "builtin" => {
                let rest: Vec<_> = args
                    .iter()
                    .skip_while(|(raw, _)| raw.starts_with('-'))
                    .cloned()
                    .collect();
                if !rest.is_empty() {
                    self.named(&rest, st);
                }
            }
            _ => {
                let body = self.funcs.get(&name).cloned().or_else(|| {
                    self.shell
                        .funcs()
                        .get(&name)
                        .map(|r| r.definition().body.clone())
                });
                if let Some(body) = body {
                    self.call(&name, &body);
                }
            }
        }
    }

    fn call(&mut self, name: &str, body: &ast::FunctionBody) {
        if self.active.len() >= 8 || !self.active.insert(name.to_string()) {
            self.unknown(format!("recursive function `{name}`"));
            return;
        }
        let mut st = State {
            dynamic: true,
            cwd: (!self.has_cd).then(|| self.entry_cwd.clone()),
            ..State::default()
        };
        if let Some(redirects) = &body.1 {
            for r in &redirects.0 {
                self.redirect(r, &mut st);
            }
        }
        self.compound(&body.0, &mut st);
        self.active.remove(name);
    }
}

#[allow(dead_code)]
fn _assert_session_env_is_used(_: &SessionEnv) {}
