//! Command implementations.

/// Environment variable supplying `--control` when the flag is omitted.
///
/// Every command that operates on an existing project reads it. `project init`
/// deliberately does not: that flag decides where a control repository is
/// *created*, and defaulting it from a variable someone exported for a
/// different project is how a project gets initialized into the wrong place.
pub const CONTROL_ENV: &str = "CHANGE_HARNESS_CONTROL";

pub mod acceptance;
pub mod archive;
pub(crate) mod assurance_probe_fixture;
pub(crate) mod assurance_probe_result;
pub(crate) mod assurance_probe_scenarios;
pub mod assurance_probes;
pub mod audit;
pub mod audit_evidence;
pub mod backup;
pub mod card;
pub mod cycle;
pub mod demo;
pub mod disposition;
pub mod doctor;
pub mod gate;
pub mod handoff;
pub mod integration;
pub mod lesson;
pub mod mutation;
pub mod project;
pub mod project_snapshot;
pub mod review;
pub mod transaction;
pub mod work;
