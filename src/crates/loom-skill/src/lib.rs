//! Skills governance (`doc/15` §3): zero-LLM repo profile, local catalog,
//! scoring, integrity lock, and L0 static checks.
//!
//! Scope follows the adoption assessment (`doc/16` §9): no second skill
//! runtime and no network — OpenCode keeps discovery/loading, this crate
//! governs what exists, what is trusted, and what the packet may advertise.

mod profile;

pub use profile::{profile_path, scan, ScanOutcome, StackProfile, STACK_PROFILE_NAME};
