use std::io::Write;
use std::sync::mpsc::SyncSender;

use shell_surface::{SurfaceMode, render_surface};
use shell_value::{RecordBatch, StructuredDiagnostic, Value};

#[derive(Debug)]
pub enum Chunk {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Batch(RecordBatch),
    Diag(Box<StructuredDiagnostic>),
}

/// Where a program's output goes. `Tty` is a frontend that owns fds 0/1/2; `Capture` never touches them.
pub enum Sink {
    Tty { mode: SurfaceMode },
    Capture(SyncSender<Chunk>),
}

impl Sink {
    pub fn is_capture(&self) -> bool {
        matches!(self, Sink::Capture(_))
    }

    /// Forwards a chunk produced by a reader thread; only meaningful for capture sinks.
    pub(crate) fn chunk(&self, chunk: Chunk) {
        if let Sink::Capture(tx) = self {
            let _ = tx.send(chunk);
        }
    }

    pub fn emit(&self, value: Value) {
        match self {
            Sink::Tty { mode } => {
                let mut out = std::io::stdout().lock();
                render_surface(value, &mut out, *mode);
            }
            Sink::Capture(tx) => match value {
                Value::Empty => {}
                Value::Bytes(bytes) => {
                    let _ = tx.send(Chunk::Stdout(bytes));
                }
                Value::Batch(batch) => {
                    let _ = tx.send(Chunk::Batch(batch));
                }
            },
        }
    }

    pub fn stdout(&self, bytes: Vec<u8>) {
        match self {
            Sink::Tty { .. } => {
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(&bytes);
                let _ = out.flush();
            }
            Sink::Capture(tx) => {
                let _ = tx.send(Chunk::Stdout(bytes));
            }
        }
    }

    pub fn stderr(&self, bytes: Vec<u8>) {
        match self {
            Sink::Tty { .. } => {
                let mut err = std::io::stderr().lock();
                let _ = err.write_all(&bytes);
            }
            Sink::Capture(tx) => {
                let _ = tx.send(Chunk::Stderr(bytes));
            }
        }
    }

    pub fn stderr_line(&self, line: &str) {
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        self.stderr(bytes);
    }

    pub fn diag(&self, diag: &StructuredDiagnostic) {
        match self {
            Sink::Tty { .. } => self.stderr_line(&format!("koa: {diag}")),
            Sink::Capture(tx) => {
                let _ = tx.send(Chunk::Diag(Box::new(diag.clone())));
            }
        }
    }
}

/// Exit status a builtin failure maps to.
pub fn diag_status(diag: &StructuredDiagnostic) -> i32 {
    match diag.code.as_str() {
        "E_ARGS" => 2,
        "E_KILLED" => 124,
        _ => 1,
    }
}
