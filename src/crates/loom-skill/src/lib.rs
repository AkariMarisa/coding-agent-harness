//! Skills governance (`doc/15` §3): zero-LLM repo profile, local catalog,
//! scoring, integrity lock, and L0 static checks.
//!
//! Scope follows the adoption assessment (`doc/16` §9): no second skill
//! runtime and no network — OpenCode keeps discovery/loading, this crate
//! governs what exists, what is trusted, and what the packet may advertise.

mod catalog;
mod lock;
mod profile;
mod recommend;
mod show;
mod verify;

pub use catalog::{
    catalog_path, load_catalog, Catalog, CatalogSkill, SkillStatus, Triggers, CATALOG_REL,
    LICENSE_ALLOWLIST,
};
pub use lock::{load_lock, lock_path, write_lock, LockEntry, SkillsLock, LOCK_REL};
pub use profile::{profile_path, scan, ScanOutcome, StackProfile, STACK_PROFILE_NAME};
pub use recommend::{
    recommend, RecommendInput, Suggestion, AUTO_THRESHOLD, MAX_AUTO, MAX_SUGGESTIONS,
};
pub use show::{read_skill, SkillBody, SHOW_BODY_CAP};
pub use verify::{verify, Finding, Level, VerifyOutcome, SKILLS_DIR_REL};
