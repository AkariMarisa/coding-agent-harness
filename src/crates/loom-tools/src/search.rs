use std::collections::HashSet;
use std::fs;

use anyhow::Result;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use loom_core::{Envelope, ErrorCode, Item, ItemType, Limits, Stats, ToolName};
use loom_store::{short_hash, write_artifact, Project, Run};
use regex::RegexBuilder;
use serde::Serialize;

use crate::util::{is_binary, preview};
use crate::{error_outcome, ToolOutcome};

const MAX_SCAN_MATCHES: usize = 5_000;

#[derive(Debug, Clone, Serialize)]
pub struct SearchRequest {
    pub query: String,
    pub scope: Option<String>,
    #[serde(default)]
    pub globs: Vec<String>,
    pub max_results: Option<usize>,
    #[serde(default)]
    pub case_sensitive: bool,
    pub cursor: Option<String>,
}

impl SearchRequest {
    pub fn args_hash(&self) -> String {
        short_hash(&serde_json::to_string(self).unwrap_or_default())
    }
}

pub fn search(
    project: &Project,
    run: &Run,
    request: &SearchRequest,
    limits: &Limits,
) -> Result<ToolOutcome> {
    let args_hash = request.args_hash();
    let query_hash = short_hash(&request.query);

    if request.query.trim().is_empty() {
        return Ok(error_outcome(
            ToolName::Search,
            "查询不能为空",
            ErrorCode::InvalidArgs,
            None,
            args_hash,
            None,
        ));
    }

    let offset = match request.cursor.as_deref() {
        Some(cursor) => match parse_cursor(cursor, &query_hash) {
            Some(offset) => offset,
            None => {
                return Ok(error_outcome(
                    ToolName::Search,
                    "分页游标已过期，请重新搜索",
                    ErrorCode::StaleCursor,
                    None,
                    args_hash,
                    request.scope.clone(),
                ));
            }
        },
        None => 0,
    };

    let scope = match request.scope.as_deref() {
        Some(path) => match project.resolve_path(path) {
            Ok(path) => path,
            Err(error) => {
                return Ok(error_outcome(
                    ToolName::Search,
                    "搜索范围不存在或越界",
                    ErrorCode::InvalidArgs,
                    Some(error.to_string()),
                    args_hash,
                    request.scope.clone(),
                ));
            }
        },
        None => project.root.clone(),
    };

    if !scope.is_dir() {
        return Ok(error_outcome(
            ToolName::Search,
            "搜索范围不是目录",
            ErrorCode::InvalidArgs,
            Some(project.relative(&scope)),
            args_hash,
            request.scope.clone(),
        ));
    }

    let regex = match RegexBuilder::new(&request.query)
        .case_insensitive(!request.case_sensitive)
        .build()
    {
        Ok(regex) => regex,
        Err(error) => {
            return Ok(error_outcome(
                ToolName::Search,
                "正则表达式无效",
                ErrorCode::InvalidArgs,
                Some(error.to_string()),
                args_hash,
                request.scope.clone(),
            ));
        }
    };

    let mut builder = WalkBuilder::new(&scope);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false)
        .max_filesize(Some(1_048_576));

    if !request.globs.is_empty() {
        let mut overrides = OverrideBuilder::new(&scope);
        for glob in &request.globs {
            if let Err(error) = overrides.add(glob) {
                return Ok(error_outcome(
                    ToolName::Search,
                    "glob 无效",
                    ErrorCode::InvalidArgs,
                    Some(error.to_string()),
                    args_hash,
                    request.scope.clone(),
                ));
            }
        }
        match overrides.build() {
            Ok(overrides) => {
                builder.overrides(overrides);
            }
            Err(error) => {
                return Ok(error_outcome(
                    ToolName::Search,
                    "glob 构建失败",
                    ErrorCode::InvalidArgs,
                    Some(error.to_string()),
                    args_hash,
                    request.scope.clone(),
                ));
            }
        }
    }

    builder.filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        !matches!(
            name.as_ref(),
            ".git"
                | ".loom"
                | ".worktrees"
                | "node_modules"
                | "target"
                | "dist"
                | "build"
                | ".venv"
                | "vendor"
        )
    });

    let mut all = Vec::new();
    let mut files = HashSet::new();
    let mut capped = false;

    for entry in builder.build() {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }

        let path = entry.path();
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        if is_binary(&bytes) {
            continue;
        }

        let text = String::from_utf8_lossy(&bytes);
        let blob = loom_store::sha256_hex(&bytes)[..12].to_string();
        let relative = project.relative(path);

        for (index, line) in text.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }

            files.insert(relative.clone());
            all.push(Item {
                item_type: ItemType::Match,
                path: Some(relative.clone()),
                blob: Some(blob.clone()),
                range: Some([index + 1, index + 1]),
                symbol: None,
                kind: None,
                preview: Some(preview(line)),
                score: Some(1.0),
                count: None,
                added: None,
                deleted: None,
                language: None,
            });

            if all.len() >= MAX_SCAN_MATCHES {
                capped = true;
                break;
            }
        }

        if capped {
            break;
        }
    }

    all.sort_by(|left, right| {
        left.path
            .as_deref()
            .unwrap_or_default()
            .cmp(right.path.as_deref().unwrap_or_default())
            .then(left.range.cmp(&right.range))
    });

    let total = all.len();
    let max_results = request
        .max_results
        .unwrap_or(limits.search_results)
        .clamp(1, limits.search_results);

    let mut page: Vec<Item> = all.iter().skip(offset).take(max_results).cloned().collect();

    let scope_label = project.relative(&scope);
    let scope_label = if scope_label.is_empty() {
        ".".to_string()
    } else {
        scope_label
    };

    let summary = if total == 0 {
        "未找到匹配".to_string()
    } else {
        let capped_note = if capped { "（扫描已截断）" } else { "" };
        format!(
            "{} 个文件中 {} 处匹配，已返回 {} 条{}",
            files.len(),
            total,
            page.len(),
            capped_note
        )
    };

    let mut envelope = Envelope::new(ToolName::Search, summary).with_items(page.clone());
    envelope.stats = Some(Stats {
        n: Some(total),
        spill: Some(false),
        ..Stats::default()
    });

    let mut truncated = offset + page.len() < total || capped;
    let needs_artifact = truncated || envelope.bytes() > limits.search_bytes;

    if needs_artifact {
        let mut content = String::new();
        for item in &all {
            let path = item.path.as_deref().unwrap_or_default();
            let line = item.range.map(|range| range[0]).unwrap_or(0);
            let text = item.preview.as_deref().unwrap_or_default();
            content.push_str(&format!("{path}:{line}:{text}\n"));
        }

        match write_artifact(run, "search", &content, &args_hash, total) {
            Ok(artifact_ref) => {
                envelope = envelope.with_ref(artifact_ref);
                if let Some(stats) = envelope.stats.as_mut() {
                    stats.spill = Some(true);
                }
            }
            Err(error) => {
                return Ok(error_outcome(
                    ToolName::Search,
                    "写入 artifact 失败",
                    ErrorCode::Internal,
                    Some(error.to_string()),
                    args_hash,
                    Some(scope_label),
                ));
            }
        }
    }

    while envelope.bytes() > limits.search_bytes && page.len() > 1 {
        page.pop();
        envelope.items = Some(page.clone());
    }

    truncated = offset + page.len() < total || capped;
    envelope.truncated = Some(truncated);
    if truncated {
        envelope.next = Some(format!("o{}-{}", offset + page.len(), query_hash));
    }

    envelope.finalize_stats();
    Ok(ToolOutcome::new(envelope, args_hash, Some(scope_label)))
}

fn parse_cursor(cursor: &str, query_hash: &str) -> Option<usize> {
    let rest = cursor.strip_prefix('o')?;
    let (offset, hash) = rest.split_once('-')?;
    if hash != query_hash {
        return None;
    }
    offset.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::Limits;

    #[test]
    fn finds_matches_and_paginates() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        fs::write(
            dir.path().join("src/a.rs"),
            "fn refresh_token() {}\nfn other() {}\n",
        )
        .expect("write");
        fs::write(dir.path().join("README.md"), "refresh_token in docs\n").expect("write");

        let project = Project::at(dir.path());
        let run = Run::open(project.clone(), Some("test")).expect("run");
        let limits = Limits::default();

        let request = SearchRequest {
            query: "refresh_token".to_string(),
            scope: Some("src".to_string()),
            globs: vec![],
            max_results: Some(1),
            case_sensitive: false,
            cursor: None,
        };

        let outcome = search(&project, &run, &request, &limits).expect("search");
        assert!(outcome.envelope.ok);
        assert_eq!(outcome.envelope.items.as_ref().map(Vec::len), Some(1));
        assert_eq!(outcome.envelope.truncated, Some(false));

        let request = SearchRequest {
            scope: None,
            ..request
        };
        let outcome = search(&project, &run, &request, &limits).expect("search");
        assert_eq!(outcome.envelope.stats.as_ref().and_then(|s| s.n), Some(2));
    }

    #[test]
    fn rejects_bad_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let run = Run::open(project.clone(), Some("test")).expect("run");
        let request = SearchRequest {
            query: "x".to_string(),
            scope: Some("../".to_string()),
            globs: vec![],
            max_results: None,
            case_sensitive: false,
            cursor: None,
        };

        let outcome = search(&project, &run, &request, &Limits::default()).expect("search");
        assert!(!outcome.envelope.ok);
    }
}
