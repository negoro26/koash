use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use shell_kernel::Session;

const VAR: &str = "KOA_ENV_CAPTURE";
const END: &[u8] = b"koa-end\0";
const SHELL_OWNED: &[&str] = &["SHLVL", "_"];

const SCRIPT: &str = r#"__koa_out=$KOA_ENV_CAPTURE
unset KOA_ENV_CAPTURE
__koa_src=$1
shift
trap '__koa_status=$?; { printf "%s\0" "$PWD"; env -0; printf "koa-end\0"; } > "$__koa_out"; exit $__koa_status' EXIT
eval "$__koa_src""#;

pub(crate) struct EnvCapture {
    path: PathBuf,
    sent: BTreeMap<String, String>,
}

impl EnvCapture {
    pub(crate) fn create(session: &Session) -> Option<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("koa-env-{}-{n}", std::process::id()));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .ok()?;
        Some(Self {
            path,
            sent: session.env.materialize(),
        })
    }

    pub(crate) fn argv(&self, shell: &str, raw: &str) -> Vec<String> {
        [shell, "-c", SCRIPT, shell, raw]
            .map(str::to_string)
            .to_vec()
    }

    pub(crate) fn env(&self) -> Vec<(String, String)> {
        vec![(VAR.to_string(), self.path.to_string_lossy().into_owned())]
    }

    pub(crate) fn apply(self, session: &mut Session) {
        let Some(bytes) = read(&self.path) else {
            return;
        };
        let Some(body) = bytes.strip_suffix(END) else {
            return;
        };
        let mut fields = body.split(|&b| b == 0).filter(|f| !f.is_empty());
        let Some(cwd) = fields.next() else {
            return;
        };
        let after: BTreeMap<String, String> = fields
            .filter_map(|f| {
                let text = std::str::from_utf8(f).ok()?;
                let (key, value) = text.split_once('=')?;
                Some((key.to_string(), value.to_string()))
            })
            .filter(|(key, _)| !SHELL_OWNED.contains(&key.as_str()))
            .collect();
        for (key, value) in &after {
            if self.sent.get(key) != Some(value) {
                session.env.insert(key.clone(), value.clone());
            }
        }
        for key in self.sent.keys() {
            if !after.contains_key(key) && !SHELL_OWNED.contains(&key.as_str()) {
                session.env.remove(key);
            }
        }
        if let Ok(cwd) = std::str::from_utf8(cwd) {
            let cwd = Path::new(cwd);
            if cwd.is_absolute() && cwd.is_dir() {
                session.cwd = cwd.to_path_buf();
            }
        }
    }
}

impl Drop for EnvCapture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}
