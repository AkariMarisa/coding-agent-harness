use anyhow::Result;
use loom_core::{
    parse_range, truncate_bytes, Envelope, ErrorCode, Item, ItemType, Limits, Stats, ToolName,
};
use loom_store::{read_artifact as load_artifact, short_hash, Run};
use serde::Serialize;

use crate::{error_outcome, ToolOutcome};

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRequest {
    pub artifact_ref: String,
    pub range: Option<String>,
    pub max_lines: Option<usize>,
}

impl ArtifactRequest {
    pub fn args_hash(&self) -> String {
        short_hash(&serde_json::to_string(self).unwrap_or_default())
    }
}

pub fn read_artifact(run: &Run, request: &ArtifactRequest, limits: &Limits) -> Result<ToolOutcome> {
    let args_hash = request.args_hash();
    let scope = Some(request.artifact_ref.clone());

    let body = match load_artifact(run, &request.artifact_ref) {
        Ok(body) => body,
        Err(error) => {
            return Ok(error_outcome(
                ToolName::Artifact,
                "artifact 不存在或引用无效",
                ErrorCode::NotFound,
                Some(error.to_string()),
                args_hash,
                scope,
            ));
        }
    };

    let lines: Vec<&str> = body.lines().collect();
    let total = lines.len();

    let requested = match request.range.as_deref() {
        Some(range) => match parse_range(range) {
            Ok(range) => Some(range),
            Err(error) => {
                return Ok(error_outcome(
                    ToolName::Artifact,
                    "行范围无效",
                    ErrorCode::InvalidArgs,
                    Some(error),
                    args_hash,
                    scope,
                ));
            }
        },
        None => None,
    };

    let max_lines = request
        .max_lines
        .unwrap_or(limits.read_lines)
        .clamp(1, limits.read_lines);

    let (start, requested_end) = match requested {
        Some((start, end)) => (start, end.min(start.saturating_add(max_lines - 1))),
        None => (1, max_lines),
    };

    if start > total && total > 0 {
        return Ok(error_outcome(
            ToolName::Artifact,
            "起始行超出 artifact 范围",
            ErrorCode::InvalidArgs,
            Some(format!("artifact has {total} lines")),
            args_hash,
            scope,
        ));
    }

    let end = requested_end.min(total);
    let selected = if total == 0 || start > end {
        String::new()
    } else {
        lines[start - 1..end].join("\n")
    };

    let (content, cut) = truncate_bytes(&selected, limits.read_bytes);
    let returned_lines = content.lines().count();
    let actual_end = if returned_lines == 0 {
        start.saturating_sub(1)
    } else {
        start + returned_lines - 1
    };

    let truncated =
        cut || requested.is_some_and(|(_, requested_end)| actual_end < requested_end.min(total));

    let summary = if total == 0 {
        format!("artifact 为空（{}）", request.artifact_ref)
    } else {
        format!(
            "已读取 {} 第 {}-{} 行（共 {} 行）",
            request.artifact_ref, start, actual_end, total
        )
    };

    let mut envelope = Envelope::new(ToolName::Artifact, summary).with_text(content.to_string());
    envelope.truncated = Some(truncated);
    envelope.stats = Some(Stats {
        n: Some(total),
        spill: Some(false),
        ..Stats::default()
    });
    envelope.items = Some(vec![Item {
        path: Some(request.artifact_ref.clone()),
        range: (returned_lines > 0).then_some([start, actual_end]),
        count: Some(total),
        ..Item::new(ItemType::Log)
    }]);

    envelope.finalize_stats();
    Ok(ToolOutcome::new(envelope, args_hash, scope))
}
