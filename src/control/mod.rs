//! Control state: the authoritative record of a project's workflow.
//!
//! Three responsibilities, deliberately separated: [`repository`] versions
//! authoritative documents in Git, [`journal`] records mutations in flight so
//! an interruption is recoverable, and [`lock`] ensures one writer at a time.

pub mod journal;
pub mod lock;
pub mod repository;
