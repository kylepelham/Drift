//! Conversations: their shape, their storage and the turn loop that advances them.

mod arrivals;
mod assemble;
mod attach;
pub mod backup;
pub mod branch;
mod changes;
pub mod clarify;
pub(crate) mod command;
pub mod compaction;
mod convert;
pub mod drive;
mod early;
mod generation;
mod oneshot;
mod processes;
pub mod prompt;
pub mod revert;
pub mod snapshot;
pub mod tasks;
mod title;
pub mod tree;
pub(crate) mod trust;
pub mod turn;
pub mod types;
