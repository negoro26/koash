pub mod builtins;
pub mod code_intel;
pub mod discover;
pub mod lsp;
pub mod pipeline;
pub mod schema;
pub mod subkernel;

pub use builtins::run_builtin;
pub use lsp::{LspDiagnostic, post_edit_diagnostics};
pub use pipeline::{
    execute_hybrid_pipeline, execute_program_surface, execute_program_surface_sync,
    execute_program_tty, execute_program_value, pipeline_uses_builtin, program_needs_async,
};
pub use schema::{BUILTIN_SCHEMAS, BuiltinSchema, get_schema, is_builtin};
