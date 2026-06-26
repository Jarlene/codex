//! Agent team coordination primitives.

mod decomposition;
mod error;
mod extension;
mod model;
mod prompt;
mod runtime;
mod scheduler;
mod store;
mod summary;
mod team;
mod tool;

pub use decomposition::HeuristicTaskDecomposer;
pub use error::TeamError;
pub use extension::install;
pub use extension::team_agent_spawner;
pub use model::*;
pub use runtime::ClaimedTask;
pub use runtime::PlanImpact;
pub use runtime::PlanReviewInput;
pub use runtime::ReclaimTaskOutcome;
pub use runtime::TaskStatusTransition;
pub use runtime::TaskUpdate;
pub use runtime::TeamRuntime;
pub use runtime::TeamRuntimeHandle;
pub use runtime::TeammateRunRequest;
pub use runtime::TeammateRunResult;
pub use runtime::TeammateRunner;
pub use runtime::TeammateRunnerFuture;
pub use runtime::TerminalTaskData;
pub use scheduler::recommend_team_size;
pub use store::FsTeamStore;
pub use summary::summarize_messages;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
