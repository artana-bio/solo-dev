//! Control state: the authoritative record of a project's workflow.
//!
//! Four responsibilities, deliberately separated: [`repository`] versions
//! authoritative documents in Git, [`event_store`] appends the authoritative
//! transition record, [`journal`] tracks mutations in flight so an interruption
//! is recoverable, and [`lock`] ensures one writer at a time.
//!
//! Events and journal entries differ in kind, not degree. An event is a fact
//! about what happened and is committed; a journal entry is a breadcrumb about
//! what is happening and is not. See D-029.

pub mod event_store;
pub mod journal;
pub mod lock;
pub mod repository;
