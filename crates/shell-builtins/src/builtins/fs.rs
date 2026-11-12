use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use super::resolve_path;

fn build_ls_record(name: String, path: &Path, meta: &std::fs::Metadata) -> StructuredRecord {
    let kind = if meta.is_dir() {
        "dir"
    } else if meta.is_symlink() {
        "symlink"
    } else {
        "file"
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| format_unix_iso(d.as_secs()))
        .unwrap_or_default();
    let perms = format!("{:o}", meta.permissions().mode() & 0o777);

    let mut rec = StructuredRecord::new();
    rec.insert("name".to_string(), JsonVal::String(name));
    rec.insert("ext".to_string(), JsonVal::String(ext));
    rec.insert("kind".to_string(), JsonVal::String(kind.to_string()));
    rec.insert("size".to_string(), json!(meta.len()));
    rec.insert("modified".to_string(), JsonVal::String(modified));
    rec.insert("permissions".to_string(), JsonVal::String(perms));
    rec
}

pub(crate) fn ls(session: &Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let mut show_all = false;
    let mut targets: Vec<&str> = Vec::new();
    let mut parse_flags = true;

    for arg in args {
        if parse_flags && arg == "--" {
            parse_flags = false;
        } else if parse_flags && arg.starts_with('-') && arg.len() > 1 {
            if arg.contains('a') {
                show_all = true;
            }
        } else {
            targets.push(arg.as_str());
        }
    }

    if targets.is_empty() {
        targets.push(".");
    }

    let mut records = Vec::new();
    for target in targets {
        let resolved = if target == "." {
            session.cwd.clone()
        } else {
            resolve_path(session, target)
        };
        let meta = std::fs::symlink_metadata(&resolved)
            .map_err(|e| StructuredDiagnostic::from_io("E_LS", &e, None))?;
        if !meta.is_dir() {
            records.push(build_ls_record(target.to_string(), &resolved, &meta));
            continue;
        }

        let entries = std::fs::read_dir(&resolved)
            .map_err(|e| StructuredDiagnostic::from_io("E_LS", &e, None))?;
        for entry_res in entries {
            let Ok(entry) = entry_res else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_all && name.starts_with('.') {
                continue;
            }
            let Ok(entry_meta) = entry.metadata() else {
                continue;
            };
            records.push(build_ls_record(name, &entry.path(), &entry_meta));
        }
    }

    records.sort_by(|a, b| {
        let na = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let nb = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        na.cmp(nb)
    });

    Ok(Value::from_records(records))
}

pub(crate) fn stat(session: &Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    if args.is_empty() {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "stat requires at least one path",
            None,
            Some("usage: stat <path>...".to_string()),
        ));
    }

    let mut records = Vec::new();
    for arg in args {
        let p = resolve_path(session, arg);
        let meta = std::fs::symlink_metadata(&p)
            .map_err(|e| StructuredDiagnostic::from_io("E_STAT", &e, None))?;
        let kind = if meta.is_dir() {
            "dir"
        } else if meta.is_symlink() {
            "symlink"
        } else {
            "file"
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| format_unix_iso(d.as_secs()))
            .unwrap_or_default();
        let perms = meta.permissions();

        let mut rec = StructuredRecord::new();
        rec.insert("path".to_string(), JsonVal::String(arg.clone()));
        rec.insert("kind".to_string(), JsonVal::String(kind.to_string()));
        rec.insert("size".to_string(), json!(meta.len()));
        rec.insert("modified".to_string(), JsonVal::String(modified));
        rec.insert("readonly".to_string(), json!(perms.readonly()));
        rec.insert(
            "mode".to_string(),
            JsonVal::String(format!("{:o}", perms.mode() & 0o777)),
        );
        records.push(rec);
    }
    Ok(Value::from_records(records))
}

pub(crate) fn find(session: &Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let mut root = session.cwd.clone();
    let mut root_label: Option<String> = None;
    let mut include_hidden = false;
    let mut filter_type: Option<String> = None;
    let mut name_matcher: Option<GlobMatcher> = None;
    let mut max_depth: Option<usize> = None;
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--hidden" => include_hidden = true,
            "-t" | "-type" => {
                if let Some(t) = iter.next() {
                    filter_type = Some(t.clone());
                }
            }
            "-name" | "-iname" => {
                let ci = arg == "-iname";
                if let Some(pat) = iter.next() {
                    let matcher = GlobBuilder::new(pat)
                        .case_insensitive(ci)
                        .build()
                        .map_err(|e| StructuredDiagnostic::new("E_GLOB", e.to_string(), None, None))?
                        .compile_matcher();
                    name_matcher = Some(matcher);
                }
            }
            "-maxdepth" => {
                if let Some(val) = iter.next()
                    && let Ok(d) = val.parse::<usize>()
                {
                    max_depth = Some(d);
                }
            }
            s if !s.starts_with('-') => {
                root = resolve_path(session, s);
                root_label = Some(s.to_string());
            }
            _ => {}
        }
    }

    let mut builder = WalkBuilder::new(&root);
    builder.hidden(!include_hidden);
    if let Some(d) = max_depth {
        builder.max_depth(Some(d));
    }
    let walker = builder.build();
    let mut records = Vec::new();
    for entry_res in walker {
        let Ok(entry) = entry_res else {
            continue;
        };
        if entry.path() == root {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let kind = if meta.is_dir() { "dir" } else { "file" };
        if let Some(ref ft) = filter_type {
            let want_dir = ft == "d" || ft == "dir";
            let want_file = ft == "f" || ft == "file";
            if (want_dir && kind != "dir") || (want_file && kind != "file") {
                continue;
            }
        }
        if let Some(ref matcher) = name_matcher {
            let fname = entry.file_name().to_string_lossy();
            if !matcher.is_match(fname.as_ref()) {
                continue;
            }
        }
        let rel = entry
            .path()
            .strip_prefix(&root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();
        let display_path = match &root_label {
            Some(lbl) if lbl != "." => format!("{}/{rel}", lbl.trim_end_matches('/')),
            _ => rel,
        };
        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        let mut rec = StructuredRecord::new();
        rec.insert("path".to_string(), JsonVal::String(display_path));
        rec.insert("ext".to_string(), JsonVal::String(ext));
        rec.insert("kind".to_string(), JsonVal::String(kind.to_string()));
        rec.insert("size".to_string(), json!(meta.len()));
        records.push(rec);
    }

    Ok(Value::from_records(records))
}

pub(crate) fn mkdir(session: &mut Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let mut recursive = false;
    let mut targets = Vec::new();
    let mut parse_flags = true;
    for arg in args {
        if parse_flags && arg == "--" {
            parse_flags = false;
        } else if parse_flags && arg.starts_with('-') && arg.len() > 1 {
            if arg.contains('p') {
                recursive = true;
            }
        } else {
            targets.push(arg.clone());
        }
    }
    for t in targets {
        let p = resolve_path(session, &t);
        let res = if recursive {
            std::fs::create_dir_all(&p)
        } else {
            std::fs::create_dir(&p)
        };
        res.map_err(|e| StructuredDiagnostic::from_io("E_MKDIR", &e, None))?;
        session.record_mutation(p.to_string_lossy().to_string());
    }
    Ok(Value::Empty)
}

pub(crate) fn rm(session: &mut Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let mut recursive = false;
    let mut force = false;
    let mut targets = Vec::new();
    for arg in args {
        if arg.starts_with('-') {
            if arg.contains('r') || arg.contains('R') {
                recursive = true;
            }
            if arg.contains('f') {
                force = true;
            }
        } else {
            targets.push(arg.clone());
        }
    }
    for t in targets {
        let p = resolve_path(session, &t);
        if !p.exists() {
            if force {
                continue;
            }
            return Err(StructuredDiagnostic::new(
                "E_RM",
                format!("{}: no such file or directory", p.display()),
                None,
                None,
            ));
        }
        let res = if p.is_dir() {
            if recursive {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_dir(&p)
            }
        } else {
            std::fs::remove_file(&p)
        };
        res.map_err(|e| StructuredDiagnostic::from_io("E_RM", &e, None))?;
        session.record_mutation(p.to_string_lossy().to_string());
    }
    Ok(Value::Empty)
}

pub(crate) fn cp(session: &mut Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let mut recursive = false;
    let mut positional = Vec::new();
    for arg in args {
        if arg == "-r" || arg == "-R" {
            recursive = true;
        } else {
            positional.push(arg.clone());
        }
    }
    if positional.len() < 2 {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "cp requires source and destination",
            None,
            None,
        ));
    }
    let src = resolve_path(session, &positional[0]);
    let dst = resolve_path(session, &positional[1]);
    if src.is_dir() {
        if !recursive {
            return Err(StructuredDiagnostic::new(
                "E_CP",
                format!("{} is a directory (use -r)", src.display()),
                None,
                None,
            ));
        }
        copy_dir_recursive(&src, &dst)
            .map_err(|e| StructuredDiagnostic::from_io("E_CP", &e, None))?;
    } else {
        let final_dst = if dst.is_dir() {
            match src.file_name() {
                Some(name) => dst.join(name),
                None => dst.clone(),
            }
        } else {
            dst.clone()
        };
        std::fs::copy(&src, &final_dst)
            .map_err(|e| StructuredDiagnostic::from_io("E_CP", &e, None))?;
    }
    session.record_mutation(dst.to_string_lossy().to_string());
    Ok(Value::Empty)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

pub(crate) fn mv(session: &mut Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    if args.len() < 2 {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "mv requires source and destination",
            None,
            None,
        ));
    }
    let src = resolve_path(session, &args[0]);
    let dst = resolve_path(session, &args[1]);
    let final_dst = if dst.is_dir() {
        match src.file_name() {
            Some(name) => dst.join(name),
            None => dst.clone(),
        }
    } else {
        dst
    };
    std::fs::rename(&src, &final_dst)
        .map_err(|e| StructuredDiagnostic::from_io("E_MV", &e, None))?;
    session.record_mutation(src.to_string_lossy().to_string());
    session.record_mutation(final_dst.to_string_lossy().to_string());
    Ok(Value::Empty)
}

fn format_unix_iso(secs: u64) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}
