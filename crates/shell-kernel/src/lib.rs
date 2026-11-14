pub mod cow;
pub mod env;
pub mod exec;
pub mod kill;
pub mod session;
pub mod word_expand;

pub use cow::{CowWorkspace, MutationState};
pub use env::SessionEnv;
pub use exec::{
    ChildGroup, ChildSetup, ExternalSpec, ForegroundOutcome, ResolvedSpawn, SpawnError, Stopped,
    group_stopped, prepare_child, reap_background_jobs, resume_group, spawn_external,
    spawn_resolved, wait_foreground,
};
pub use kill::KillSwitch;
pub use session::{Job, JobState, Session, TxRecord};
pub use word_expand::expand_word;
