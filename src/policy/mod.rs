//! Policy evaluation.
//!
//! Pure decision logic with no Git or filesystem access, so every rule is
//! testable in isolation and cannot accidentally mutate anything while deciding
//! whether a mutation is allowed.

pub mod actors;
pub mod allocation;
pub mod convergence;
pub mod credential_shape;
pub mod paths;
pub mod progressive_validation;
pub mod verification;
