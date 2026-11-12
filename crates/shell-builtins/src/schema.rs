use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BuiltinSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub flags: &'static [&'static str],
    pub columns: &'static [&'static str],
}

pub const BUILTIN_SCHEMAS: &[BuiltinSchema] = &[
    BuiltinSchema {
        name: "cd",
        description: "Change working directory",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "pwd",
        description: "Print current working directory",
        flags: &[],
        columns: &["path"],
    },
    BuiltinSchema {
        name: "export",
        description: "Export environment variables",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "env",
        description: "List environment variables",
        flags: &[],
        columns: &["key", "value"],
    },
    BuiltinSchema {
        name: "ls",
        description: "List directory entries as structured records",
        flags: &["-a", "-l"],
        columns: &["name", "ext", "kind", "size", "modified", "permissions"],
    },
    BuiltinSchema {
        name: "stat",
        description: "Inspect file or directory metadata",
        flags: &[],
        columns: &["path", "kind", "size", "modified", "readonly", "mode"],
    },
    BuiltinSchema {
        name: "cat",
        description: "Read file contents with Hashline anchors or raw bytes",
        flags: &["--raw", "-r", "-n"],
        columns: &["file", "line", "hash", "text"],
    },
    BuiltinSchema {
        name: "echo",
        description: "Print arguments as raw text",
        flags: &["-n"],
        columns: &[],
    },
    BuiltinSchema {
        name: "head",
        description: "Take first N records, file lines, or raw lines",
        flags: &["-n", "--raw", "-r"],
        columns: &["file", "line", "hash", "text"],
    },
    BuiltinSchema {
        name: "tail",
        description: "Take last N records, file lines, or raw lines",
        flags: &["-n", "--raw", "-r"],
        columns: &["file", "line", "hash", "text"],
    },
    BuiltinSchema {
        name: "find",
        description: "Walk directory tree respecting ignore rules",
        flags: &["-t", "-type", "-name", "-iname", "-maxdepth", "--hidden"],
        columns: &["path", "ext", "kind", "size"],
    },
    BuiltinSchema {
        name: "grep",
        description: "Search files or input stream with Hashline anchors",
        flags: &[
            "-i",
            "-n",
            "-r",
            "-R",
            "-E",
            "-F",
            "-w",
            "-v",
            "-l",
            "-c",
            "--include",
            "--exclude",
        ],
        columns: &["file", "line", "hash", "text"],
    },
    BuiltinSchema {
        name: "patch",
        description: "Apply hash-verified line edit using line:hash anchor",
        flags: &["--delete"],
        columns: &["file", "line", "old_hash", "new_hash", "status"],
    },
    BuiltinSchema {
        name: "prewalk",
        description: "Emit token-budgeted structural repository skeleton",
        flags: &["--budget"],
        columns: &["file", "rank", "tokens", "skeleton"],
    },
    BuiltinSchema {
        name: "ast",
        description: "Structural symbol and definition search with Hashline anchors",
        flags: &[],
        columns: &["file", "line", "hash", "kind", "text"],
    },
    BuiltinSchema {
        name: "ps",
        description: "List running processes",
        flags: &[],
        columns: &["pid", "name", "state", "cmdline"],
    },
    BuiltinSchema {
        name: "kill",
        description: "Send signal to process",
        flags: &["-9", "-15", "-s"],
        columns: &[],
    },
    BuiltinSchema {
        name: "which",
        description: "Locate executable in PATH",
        flags: &[],
        columns: &["name", "path"],
    },
    BuiltinSchema {
        name: "mkdir",
        description: "Create directories",
        flags: &["-p"],
        columns: &[],
    },
    BuiltinSchema {
        name: "rm",
        description: "Remove files or directories",
        flags: &["-r", "-f"],
        columns: &[],
    },
    BuiltinSchema {
        name: "cp",
        description: "Copy files or directories",
        flags: &["-r"],
        columns: &[],
    },
    BuiltinSchema {
        name: "mv",
        description: "Move or rename files",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "where",
        description: "Filter structured records by predicate",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "select",
        description: "Project specific columns from structured records",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "sort-by",
        description: "Sort structured records by column",
        flags: &["--desc"],
        columns: &[],
    },
    BuiltinSchema {
        name: "first",
        description: "Take first N records",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "last",
        description: "Take last N records",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "from",
        description: "Parse structured input (e.g. from json)",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "to",
        description: "Serialize structured records (e.g. to json, to compact)",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "discover",
        description: "BM25 progressive command discovery by natural language intent",
        flags: &["--limit"],
        columns: &["command", "score", "description", "flags", "columns"],
    },
    BuiltinSchema {
        name: "undo",
        description: "Undo the most recent transactional micro-commit",
        flags: &[],
        columns: &["tx_id", "restored_files"],
    },
    BuiltinSchema {
        name: "rollback",
        description: "Roll back filesystem state to a specific transaction ID",
        flags: &[],
        columns: &["tx_id", "restored_files"],
    },
    BuiltinSchema {
        name: "tx-log",
        description: "List recorded transactional micro-commits",
        flags: &[],
        columns: &["tx_id", "command", "files"],
    },
    BuiltinSchema {
        name: "diag",
        description: "Run project diagnostics (cargo check) and emit structured errors",
        flags: &[],
        columns: &["file", "line", "severity", "code", "message"],
    },
    BuiltinSchema {
        name: "refs",
        description: "Find symbol references across workspace",
        flags: &[],
        columns: &["file", "line", "hash", "text"],
    },
    BuiltinSchema {
        name: "rename",
        description: "Rename symbol across workspace with micro-commit tracking",
        flags: &[],
        columns: &["file", "replacements"],
    },
    BuiltinSchema {
        name: "@py",
        description: "Run Python sub-kernel snippet",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "@js",
        description: "Run JavaScript sub-kernel snippet",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "@sql",
        description: "Run SQLite sub-kernel query",
        flags: &[],
        columns: &[],
    },
    BuiltinSchema {
        name: "help",
        description: "Show builtin commands and flags",
        flags: &[],
        columns: &["command", "description", "flags", "columns"],
    },
];

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "sort_by" | "tx_log") || BUILTIN_SCHEMAS.iter().any(|s| s.name == name)
}

pub fn get_schema(name: &str) -> Option<&'static BuiltinSchema> {
    let canonical = match name {
        "sort_by" => "sort-by",
        "tx_log" => "tx-log",
        other => other,
    };
    BUILTIN_SCHEMAS.iter().find(|s| s.name == canonical)
}
