//! Filesystem layout and persistence for `.loom/`.

mod artifact;
mod project;
mod run;
mod seen;
mod telemetry;

pub use artifact::{read_artifact, write_artifact};
pub use project::{InitReport, Project};
pub use run::Run;
pub use seen::Seen;
pub use telemetry::{append_event, now_ms, totals, ToolEvent, Totals};

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn short_hash(text: &str) -> String {
    sha256_hex(text.as_bytes())[..12].to_string()
}
