//! On-demand skill bodies (`doc/15` §3.6): the packet advertises, this
//! delivers — one call, one body, hard-capped so a pull can never blow the
//! context budget.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// ≤4KB per skill (`doc/15` §3.6); the 8KB aggregate applies to callers
/// that assemble several bodies, which v1 has none of.
pub const SHOW_BODY_CAP: usize = 4096;

#[derive(Debug)]
pub struct SkillBody {
    pub id: String,
    /// Repo-relative path with forward slashes.
    pub path: String,
    /// The body, capped at [`SHOW_BODY_CAP`] bytes on a char boundary.
    pub body: String,
    pub truncated: bool,
    pub total_bytes: usize,
    pub tokens: usize,
    pub references: Vec<String>,
}

/// Read one declared skill's body. Ids follow the `TaskSpec.skills` rules
/// (plain path names), so `show` can never traverse out of the skills dir.
pub fn read_skill(root: &Path, id: &str) -> Result<SkillBody> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        || id.starts_with('.')
        || id.contains("..")
    {
        anyhow::bail!("invalid skill id {id:?} (plain name: letters, digits, '-', '_', '.')");
    }

    let path = root
        .join(".opencode")
        .join("skills")
        .join(id)
        .join("SKILL.md");
    let full = fs::read_to_string(&path).with_context(|| {
        format!(
            "skill {id} not found at {} — declared ids resolve under .opencode/skills/",
            path.display()
        )
    })?;

    let total_bytes = full.len();
    let mut cap = SHOW_BODY_CAP.min(total_bytes);
    while cap > 0 && !full.is_char_boundary(cap) {
        cap -= 1;
    }
    let truncated = cap < total_bytes;
    let body = full[..cap].to_string();

    let references_dir = path.parent().expect("skill dir").join("references");
    let mut references = Vec::new();
    if let Ok(entries) = fs::read_dir(&references_dir) {
        for entry in entries.flatten() {
            let candidate = entry.path();
            if candidate.is_file() {
                references.push(
                    candidate
                        .strip_prefix(root)
                        .unwrap_or(&candidate)
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        references.sort();
    }

    Ok(SkillBody {
        id: id.to_string(),
        path: path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/"),
        body,
        truncated,
        total_bytes,
        tokens: loom_core::estimate_tokens(&full),
        references,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, content).expect("write");
    }

    #[test]
    fn reads_body_and_lists_references() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            &dir.path().join(".opencode/skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: Use when testing.\n---\n\nBody here.\n",
        );
        write(
            &dir.path().join(".opencode/skills/alpha/references/x.md"),
            "# x\n",
        );

        let skill = read_skill(dir.path(), "alpha").expect("read");
        assert!(skill.body.contains("Body here."));
        assert!(!skill.truncated);
        assert_eq!(skill.total_bytes, skill.body.len());
        assert!(skill.tokens > 0);
        assert_eq!(
            skill.references,
            vec![".opencode/skills/alpha/references/x.md"]
        );
        assert_eq!(skill.path, ".opencode/skills/alpha/SKILL.md");
    }

    #[test]
    fn caps_the_body_at_four_kib_on_a_char_boundary() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Multi-byte content so a naive byte cut would panic.
        let mut big = String::from("---\nname: a\ndescription: d\n---\n");
        big.push_str(&"é".repeat(4_000));
        write(&dir.path().join(".opencode/skills/a/SKILL.md"), &big);

        let skill = read_skill(dir.path(), "a").expect("read");
        assert!(skill.truncated);
        assert!(skill.body.len() <= SHOW_BODY_CAP);
        assert!(skill.total_bytes > SHOW_BODY_CAP);
        assert!(skill.body.is_char_boundary(skill.body.len()), "clean cut");
        assert!(
            skill.tokens > skill.body.len() / 8,
            "tokens cover the full body"
        );
    }

    #[test]
    fn rejects_traversal_and_missing_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read_skill(dir.path(), "../secrets").is_err());
        assert!(read_skill(dir.path(), "a/b").is_err());
        assert!(read_skill(dir.path(), "").is_err());
        let missing = read_skill(dir.path(), "ghost").expect_err("missing");
        assert!(format!("{missing:#}").contains("not found"));
    }
}
