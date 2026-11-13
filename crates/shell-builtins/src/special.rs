use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, tcsetpgrp};
use shell_kernel::{JobState, Session, expand_word, wait_foreground};
use shell_syntax::SimpleCommand;

use crate::sink::Sink;

/// Waits for a foreground group and settles the job table: a stopped group lands in it,
/// a finished one leaves it. Returns the pipeline status.
pub(crate) fn finish_foreground(
    session: &mut Session,
    sink: &Sink,
    pgid: Pid,
    pids: &[Pid],
    watch: Option<Pid>,
    summary: &str,
    existing_job: Option<usize>,
) -> i32 {
    let outcome = wait_foreground(&session.kill, pgid, pids, watch);
    session.restore_terminal();
    let Some(stopped) = outcome.stopped else {
        if let Some(id) = existing_job {
            session.jobs.retain(|j| j.id != id);
        }
        return outcome.watched_status;
    };
    let id = match existing_job {
        Some(id) => {
            if let Some(job) = session.jobs.iter_mut().find(|j| j.id == id) {
                job.state = JobState::Stopped;
                job.pids = stopped.alive;
            }
            id
        }
        None => session.add_job(pgid, stopped.alive, summary.to_string(), JobState::Stopped),
    };
    sink.stderr_line(&format!("\n[{id}]+ Stopped                 {summary}"));
    128 + stopped.signal
}

fn job_index(session: &Session, arg: Option<&String>, default: Option<usize>) -> Option<usize> {
    match arg.and_then(|s| s.trim_start_matches('%').parse::<usize>().ok()) {
        Some(id) => session.jobs.iter().position(|j| j.id == id),
        None => default,
    }
}

/// Shell-state commands. They only make sense as the sole command of a list item, so they are
/// never stages of a pipeline. `None` means `simple` is not one of them.
pub(crate) fn run_special(session: &mut Session, simple: &SimpleCommand, sink: &Sink) -> Option<i32> {
    let args: Vec<String> = simple
        .words
        .iter()
        .map(|w| expand_word(w, &session.env, session.last_status))
        .collect();
    match args.first()?.as_str() {
        "unset" => {
            for arg in &args[1..] {
                session.env.remove(arg);
            }
            Some(0)
        }
        "exit" => {
            let code = args
                .get(1)
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(session.last_status);
            session.should_exit = Some(code);
            Some(code)
        }
        "jobs" => {
            let mut out = String::new();
            for job in &session.jobs {
                let state = match job.state {
                    JobState::Running => "Running",
                    JobState::Stopped => "Stopped",
                    JobState::Done(_) => "Done",
                };
                out.push_str(&format!("[{}]+ {state:<23} {}\n", job.id, job.command));
            }
            if !out.is_empty() {
                sink.stdout(out.into_bytes());
            }
            Some(0)
        }
        "fg" => {
            let default = session.jobs.len().checked_sub(1);
            let Some(idx) = job_index(session, args.get(1), default) else {
                sink.stderr_line("fg: no current job");
                return Some(1);
            };
            let job = session.jobs[idx].clone();
            sink.stderr_line(&job.command);
            if session.interactive {
                let _ = tcsetpgrp(std::io::stdin(), job.pgid);
            }
            for _ in &job.pids {
                session.kill.track(job.pgid);
            }
            let _ = killpg(job.pgid, Some(Signal::SIGCONT));
            Some(finish_foreground(
                session,
                sink,
                job.pgid,
                &job.pids,
                job.pids.last().copied(),
                &job.command,
                Some(job.id),
            ))
        }
        "bg" => {
            let default = session.jobs.iter().rposition(|j| j.state == JobState::Stopped);
            let Some(idx) = job_index(session, args.get(1), default) else {
                sink.stderr_line("bg: no stopped job");
                return Some(1);
            };
            let job = &mut session.jobs[idx];
            job.state = JobState::Running;
            let _ = killpg(job.pgid, Some(Signal::SIGCONT));
            sink.stderr_line(&format!("[{}]+ {} &", job.id, job.command));
            Some(0)
        }
        _ => None,
    }
}
