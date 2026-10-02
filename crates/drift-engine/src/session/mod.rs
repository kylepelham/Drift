//! Conversations: their shape, their storage and the turn loop that advances them.

mod assemble;
mod attach;
pub mod branch;
mod changes;
pub(crate) mod command;
pub mod clarify;
pub mod compaction;
mod convert;
mod early;
mod oneshot;
pub mod prompt;
pub mod revert;
pub mod snapshot;
pub mod tasks;
mod title;
mod trust;
pub mod tree;
pub mod turn;
pub mod types;
