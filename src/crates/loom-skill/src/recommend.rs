//! Skill recommendation (`doc/15` §3.4): deterministic scoring of the
//! catalog against the repo profile and one task — no embeddings, no model
//! judgment; the human remains the decider (`doc/15` §12.3 rule 1).
//!
//! Scoring follows §3.4: `tag*3 + evidence*2 + role*2 + trust*2 −
//! token_cost*0.02 − conflict − already_loaded`. Two readings are pinned
//! here because the document leaves them open:
//! `token_cost` is scored in **hundreds** of tokens (a literal 900-token
//! cost would be −18 and make every score negative), and the auto-load
//! threshold is **4.0** (at most 5 suggestions, at most 2 auto-loaded, and
//! `experimental` is never auto-loaded).

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, CatalogSkill, SkillStatus};
use crate::profile::StackProfile;

pub const AUTO_THRESHOLD: f64 = 4.0;
pub const MAX_SUGGESTIONS: usize = 5;
pub const MAX_AUTO: usize = 2;
/// Evidence files shown per suggestion (capped contribution: 3 × 2 points).
const EVIDENCE_CAP: usize = 3;
/// Depth-3 walk: enough for `doc/`, `src/`, `scripts/`, bounded cost.
const WALK_DEPTH: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: String,
    pub score: f64,
    pub auto: bool,
    pub tokens: usize,
    /// Why this scored: matched tags, evidence files, role, trust, penalties.
    pub why: Vec<String>,
}

pub struct RecommendInput<'a> {
    pub root: &'a Path,
    pub profile: &'a StackProfile,
    pub catalog: &'a Catalog,
    pub role: &'a str,
    /// Ids the task already declares (`already_loaded_penalty`).
    pub loaded: &'a [String],
}

pub fn recommend(input: &RecommendInput<'_>) -> Vec<Suggestion> {
    let vocabulary: HashSet<&str> = input
        .profile
        .lang
        .iter()
        .chain(&input.profile.infra)
        .chain(&input.profile.tests)
        .map(String::as_str)
        .collect();
    let files = walk_files(input.root);

    let mut candidates: Vec<(CatalogSkill, f64, usize, Vec<String>)> = Vec::new();
    for skill in &input.catalog.skills {
        if skill.status == SkillStatus::Blocked {
            continue;
        }
        let body_path = input
            .root
            .join(".opencode")
            .join("skills")
            .join(&skill.id)
            .join("SKILL.md");
        let Ok(body) = fs::read_to_string(&body_path) else {
            // Stale catalog entry: `loom skill verify` flags it; recommend
            // simply has nothing to score.
            continue;
        };
        let tokens = loom_core::estimate_tokens(&body);
        let mut why = Vec::new();

        let mut tag_hits: Vec<String> = skill
            .triggers
            .lang
            .iter()
            .chain(&skill.tags)
            .filter(|tag| vocabulary.contains(tag.as_str()))
            .cloned()
            .collect();
        tag_hits.sort();
        tag_hits.dedup();
        let tag_match = tag_hits.len();
        if tag_match > 0 {
            why.push(format!("tags matched: {}", tag_hits.join(", ")));
        }

        let evidence: Vec<&String> = files
            .iter()
            .filter(|file| skill.triggers.globs.iter().any(|g| glob_matches(g, file)))
            .take(EVIDENCE_CAP)
            .collect();
        let evidence_match = evidence.len();
        if evidence_match > 0 {
            why.push(format!(
                "evidence: {}",
                evidence
                    .iter()
                    .map(|f| f.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        let role_match = usize::from(skill.triggers.roles.iter().any(|r| r == input.role));
        if role_match == 1 {
            why.push(format!("role: {}", input.role));
        }

        let trust = match skill.status {
            SkillStatus::Core => 1.0,
            SkillStatus::Project => 0.7,
            SkillStatus::Experimental => 0.3,
            SkillStatus::Blocked => 0.0,
        };
        why.push(format!("trust: {}", status_str(skill.status)));

        // §3.4: token_cost in hundreds of tokens (see module docs).
        let token_cost = tokens as f64 / 100.0;
        let mut score = (tag_match as f64) * 3.0
            + (evidence_match as f64) * 2.0
            + (role_match as f64) * 2.0
            + trust * 2.0
            - token_cost * 0.02;

        if input.loaded.iter().any(|id| id == &skill.id) {
            score -= 2.0;
            why.push("already declared on the task (−2)".to_string());
        }

        candidates.push((skill.clone(), round2(score), tokens, why));
    }

    // Highest first, id as the deterministic tie-break.
    candidates.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.id.cmp(&b.0.id))
    });

    let mut chosen: Vec<(CatalogSkill, f64, usize, Vec<String>)> = Vec::new();
    for candidate in candidates {
        if chosen.len() >= MAX_SUGGESTIONS {
            break;
        }
        let conflicts = chosen
            .iter()
            .filter(|other| overlaps(&candidate.0, other))
            .count();
        let mut score = candidate.1;
        let mut why = candidate.3;
        if conflicts > 0 {
            score = round2(score - conflicts as f64);
            why.push(format!("conflicts with an earlier pick (−{conflicts})"));
        }
        if score <= 0.0 {
            continue;
        }
        chosen.push((candidate.0, score, candidate.2, why));
    }

    let mut auto_granted = 0;
    chosen
        .into_iter()
        .map(|(skill, score, tokens, why)| {
            let eligible = auto_granted < MAX_AUTO
                && score >= AUTO_THRESHOLD
                && matches!(skill.status, SkillStatus::Core | SkillStatus::Project);
            if eligible {
                auto_granted += 1;
            }
            Suggestion {
                id: skill.id,
                score,
                auto: eligible,
                tokens,
                why,
            }
        })
        .collect()
}

fn status_str(status: SkillStatus) -> &'static str {
    match status {
        SkillStatus::Core => "core",
        SkillStatus::Project => "project",
        SkillStatus::Experimental => "experimental",
        SkillStatus::Blocked => "blocked",
    }
}

fn overlaps(a: &CatalogSkill, b: &(CatalogSkill, f64, usize, Vec<String>)) -> bool {
    let left: HashSet<&str> = a
        .tags
        .iter()
        .chain(&a.triggers.lang)
        .map(String::as_str)
        .collect();
    b.0.tags
        .iter()
        .chain(&b.0.triggers.lang)
        .map(String::as_str)
        .any(|tag| left.contains(tag))
}

/// Files a skill's globs may match: a depth-3 walk with the `ignore`
/// crate's defaults (gitignore respected, hidden directories skipped —
/// `.opencode/` and `.loom/` are not project evidence).
fn walk_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .max_depth(Some(WALK_DEPTH))
        .build()
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(path) = entry.path().strip_prefix(root) else {
            continue;
        };
        files.push(path.to_string_lossy().replace('\\', "/"));
    }
    files.sort();
    files
}

/// v1 glob vocabulary: exact paths, `*.ext`, and `**/*.ext` — the forms the
/// catalog actually uses. Anything else matches nothing (never a guess).
fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    if pattern == path {
        return true;
    }
    let suffix = pattern
        .strip_prefix("**/")
        .or_else(|| pattern.strip_prefix("*./"))
        .unwrap_or(&pattern);
    if let Some(ext) = suffix.strip_prefix("*.") {
        return path.ends_with(&format!(".{ext}"));
    }
    if let Some(rest) = suffix.strip_prefix("**/") {
        return path.ends_with(rest) && rest.starts_with('/');
    }
    false
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, CatalogSkill, Triggers};

    fn skill(
        id: &str,
        tags: &[&str],
        lang: &[&str],
        roles: &[&str],
        status: SkillStatus,
    ) -> CatalogSkill {
        CatalogSkill {
            id: id.to_string(),
            version: None,
            source: "internal".to_string(),
            license: "MIT".to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            triggers: Triggers {
                lang: lang.iter().map(|t| t.to_string()).collect(),
                globs: vec!["**/*.rs".to_string()],
                roles: roles.iter().map(|t| t.to_string()).collect(),
            },
            status,
        }
    }

    fn fixture() -> (tempfile::TempDir, StackProfile) {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write");
        fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        fs::write(dir.path().join("src/lib.rs"), "pub fn x() {}\n").expect("write");
        for id in ["alpha", "beta", "gamma"] {
            let path = dir
                .path()
                .join(".opencode")
                .join("skills")
                .join(id)
                .join("SKILL.md");
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(&path, format!("# {id}\n\nUse when testing {id}.\n")).expect("write");
        }
        let profile = crate::scan(dir.path()).expect("scan").profile;
        (dir, profile)
    }

    fn run(
        root: &Path,
        profile: &StackProfile,
        skills: Vec<CatalogSkill>,
        role: &str,
        loaded: &[String],
    ) -> Vec<Suggestion> {
        let catalog = Catalog { v: 1, skills };
        recommend(&RecommendInput {
            root,
            profile,
            catalog: &catalog,
            role,
            loaded,
        })
    }

    #[test]
    fn scores_tags_evidence_role_and_trust() {
        let (dir, profile) = fixture();
        let suggestions = run(
            dir.path(),
            &profile,
            vec![
                skill(
                    "alpha",
                    &["rust"],
                    &["rust"],
                    &["review"],
                    SkillStatus::Core,
                ),
                skill(
                    "beta",
                    &["rust"],
                    &["rust"],
                    &["review"],
                    SkillStatus::Experimental,
                ),
                skill(
                    "gamma",
                    &["python"],
                    &["python"],
                    &["survey"],
                    SkillStatus::Project,
                ),
            ],
            "review",
            &[],
        );
        assert_eq!(suggestions.len(), 3, "positive scores only, all shown");

        let alpha = suggestions.iter().find(|s| s.id == "alpha").expect("alpha");
        // tag 3 + evidence (**/*.rs → src/lib.rs) 2 + role 2 + trust 2 − tokens.
        assert!(alpha.score >= 8.5, "expected ≥8.5, got {}", alpha.score);
        assert!(alpha.auto, "core + high score is auto-eligible");
        assert!(alpha.why.iter().any(|w| w.contains("tags matched: rust")));

        let experimental = suggestions.iter().find(|s| s.id == "beta").expect("beta");
        assert!(
            !experimental.auto,
            "experimental is never auto-loaded even at the same score"
        );
    }

    #[test]
    fn blocked_skills_never_appear_and_missing_bodies_are_skipped() {
        let (dir, profile) = fixture();
        // delta is blocked; ghost has no SKILL.md on disk — both vanish.
        let suggestions = run(
            dir.path(),
            &profile,
            vec![
                skill(
                    "delta",
                    &["rust"],
                    &["rust"],
                    &["review"],
                    SkillStatus::Blocked,
                ),
                skill(
                    "ghost",
                    &["rust"],
                    &["rust"],
                    &["review"],
                    SkillStatus::Core,
                ),
                skill(
                    "alpha",
                    &["rust"],
                    &["rust"],
                    &["review"],
                    SkillStatus::Project,
                ),
            ],
            "review",
            &[],
        );
        let ids: Vec<&str> = suggestions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha"], "blocked skipped, ghost has no body");
    }

    #[test]
    fn already_declared_is_penalized_and_conflicts_are_adjusted() {
        let (dir, profile) = fixture();
        let both = || {
            vec![
                skill(
                    "alpha",
                    &["rust"],
                    &["rust"],
                    &["review"],
                    SkillStatus::Project,
                ),
                skill(
                    "beta",
                    &["rust"],
                    &["rust"],
                    &["review"],
                    SkillStatus::Project,
                ),
            ]
        };

        // Loaded: alpha loses 2 points — the penalty is explained.
        let suggestions = run(
            dir.path(),
            &profile,
            both(),
            "review",
            &["alpha".to_string()],
        );
        let alpha = suggestions.iter().find(|s| s.id == "alpha").expect("alpha");
        assert!(
            alpha.why.iter().any(|w| w.contains("already declared")),
            "penalty is explained: {:?}",
            alpha.why
        );

        // Nothing loaded: the id tie-break picks alpha first, so beta —
        // sharing the `rust` tag — is adjusted by the conflict penalty.
        let suggestions = run(dir.path(), &profile, both(), "review", &[]);
        let beta = suggestions.iter().find(|s| s.id == "beta").expect("beta");
        assert!(
            beta.why
                .iter()
                .any(|w| w.contains("conflicts with an earlier pick")),
            "conflict is explained: {:?}",
            beta.why
        );
    }

    #[test]
    fn suggestions_capped_at_five() {
        let (dir, profile) = fixture();
        let skills = (0..8)
            .map(|i| {
                skill(
                    &format!("s{i}"),
                    &["rust"],
                    &["rust"],
                    &["review"],
                    SkillStatus::Project,
                )
            })
            .collect::<Vec<_>>();
        // Bodies exist only for alpha/beta/gamma — create the rest.
        for i in 0..8 {
            let path = dir
                .path()
                .join(".opencode")
                .join("skills")
                .join(format!("s{i}"))
                .join("SKILL.md");
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(&path, "# body\n").expect("write");
        }
        let suggestions = run(dir.path(), &profile, skills, "review", &[]);
        assert!(suggestions.len() <= MAX_SUGGESTIONS);
        let autos = suggestions.iter().filter(|s| s.auto).count();
        assert!(autos <= MAX_AUTO, "at most 2 auto-loaded");
    }

    #[test]
    fn glob_vocabulary_is_the_documented_three_forms() {
        assert!(glob_matches("**/*.rs", "src/lib.rs"));
        assert!(
            glob_matches("**/*.rs", "lib.rs"),
            "**/ also matches the root"
        );
        assert!(glob_matches("*.md", "doc/a.md"));
        assert!(glob_matches("src/lib.rs", "src/lib.rs"), "exact path");
        assert!(!glob_matches("**/*.rs", "src/main.md"));
        assert!(!glob_matches("scripts/x.sh", "src/x.sh"));
    }
}
