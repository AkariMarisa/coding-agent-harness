//! Local CI core for target projects: `.loom.ci.toml` parsing and
//! validation, stack detection, and generation.
//!
//! Spec: `doc/15` §4 and the locked implementation subset in `doc/16` §7.
//! The configuration is tracked project policy at the repo root; runtime logs
//! belong to `.loom/ci/` (ignored state). This crate executes nothing and
//! touches no git state — `run` lives beside it and the CLI wires both.

mod config;
mod detect;
mod exec;
mod hooks;
mod report;
mod run;

pub use config::{
    ci_path, load_ci_file, CiFile, Job, Runner, CI_CONFIG_MAX_BYTES, CI_CONFIG_NAME, CI_VERSION,
};
pub use detect::{bootstrap, detect, BootstrapReport, Detection, Evidence};
pub use exec::{missing_tool, which};
pub use hooks::{
    hook_script, hook_status, install, remove, InstallReport, RemoveReport, INSTALLABLE,
    MANAGED_MARKER,
};
pub use report::{
    sha256_hex, tree_fingerprint, tree_id, verify_attestation, Attestation, JobReport, RunReport,
    TreeFingerprint, VerifyOutcome,
};
pub use run::{run, RunOutcome, RunRequest, EXIT_FAILED, EXIT_NEEDS_HUMAN, EXIT_OK, EXIT_TIMEOUT};
