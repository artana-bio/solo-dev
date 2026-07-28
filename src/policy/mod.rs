//! Policy evaluation.
//!
//! Pure decision logic with no Git or filesystem access, so every rule is
//! testable in isolation and cannot accidentally mutate anything while deciding
//! whether a mutation is allowed.

pub mod allocation;
pub mod paths;
