use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub command: String,
    pub cwd: String,
    pub exit_code: i32,
    pub timestamp: u64,
}

pub struct History {
    path: PathBuf,
    entries: Vec<HistoryEntry>,
}

impl History {
    pub fn load_default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let path = PathBuf::from(home).join(".koa_history");
        let mut entries = Vec::new();
        if let Ok(file) = File::open(&path) {
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(entry) = serde_json::from_str::<HistoryEntry>(&line) {
                    entries.push(entry);
                }
            }
        }
        Self { path, entries }
    }

    pub fn append(&mut self, command: &str, cwd: &str, exit_code: i32) {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return;
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = HistoryEntry {
            command: trimmed.to_string(),
            cwd: cwd.to_string(),
            exit_code,
            timestamp,
        };
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.path)
            && let Ok(json) = serde_json::to_string(&entry)
        {
            let _ = writeln!(file, "{json}");
        }
        self.entries.push(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, idx: usize) -> Option<&HistoryEntry> {
        self.entries.get(idx)
    }
}
