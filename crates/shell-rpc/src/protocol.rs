use serde::{Deserialize, Serialize};
use shell_builtins::LspDiagnostic;
use shell_value::{Records, StructuredDiagnostic};

#[derive(Debug, Clone, Deserialize)]
pub struct RpcRequest {
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    #[serde(default = "default_true")]
    pub allow_network: bool,
    #[serde(default)]
    pub writable_paths: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allow_network: true,
            writable_paths: Vec::new(),
            dry_run: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamRule {
    pub name: String,
    pub pattern: String,
    #[serde(default = "default_true")]
    pub abort_on_match: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecParams {
    pub session_id: String,
    pub command: String,
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub policy: PolicyConfig,
    #[serde(default)]
    pub stream_rules: Vec<StreamRule>,
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionIdParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ForkParams {
    pub session_id: String,
    #[serde(default)]
    pub isolated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RestoreParams {
    pub snapshot: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<StructuredDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RpcEvent<'a> {
    ProcessSpawned {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<serde_json::Value>,
        pid: u32,
        argv: Vec<String>,
        cwd: String,
    },
    StreamChunk {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        records: Option<Records<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        compact: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stdout: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
    },
    PolicyBlocked {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<serde_json::Value>,
        reason: String,
        command: String,
    },
    RuleTriggered {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<serde_json::Value>,
        rule: String,
        matched_line: String,
    },
    Completed {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<serde_json::Value>,
        exit_code: i32,
        duration_ms: u64,
        mutated_files_manifest: Vec<String>,
        truncated: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        spill_uri: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lsp_diagnostics: Vec<LspDiagnostic>,
    },
}
