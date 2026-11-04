use std::fs;
use std::path::Path;

use anyhow::Result;
use loom_core::{
    parse_range, truncate_bytes, Envelope, ErrorCode, Item, ItemType, Limits, Stats, ToolName,
};
use loom_store::{short_hash, write_artifact, Project, Run, Seen};
use serde::Serialize;

use crate::util::{is_binary, language_for};
use crate::{error_outcome, ToolOutcome};

const MAX_FILE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Serialize)]
pub struct ReadRequest {
    pub path: String,
    pub range: Option<String>,
    pub max_lines: Option<usize>,
}

impl ReadRequest {
    pub fn args_hash(&self) -> String {
        short_hash(&serde_json::to_string(self).unwrap_or_default())
    }
}

pub fn read(
    project: &Project,
    run: &Run,
    request: &ReadRequest,
    limits: &Limits,
) -> Result<ToolOutcome> {
    let args_hash = request.args_hash();
    let scope = Some(request.path.clone());

    let path = match project.resolve_path(&request.path) {
        Ok(path) => path,
        Err(error) => {
            return Ok(error_outcome(
                ToolName::Read,
                "文件不存在或路径越界",
                ErrorCode::NotFound,
                Some(error.to_string()),
                args_hash,
                scope,
            ));
        }
    };

    if !path.is_file() {
        return Ok(error_outcome(
            ToolName::Read,
            "不是普通文件",
            ErrorCode::InvalidArgs,
            Some(request.path.clone()),
            args_hash,
            scope,
        ));
    }

    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(error_outcome(
                ToolName::Read,
                "无法读取文件元数据",
                ErrorCode::Permission,
                Some(error.to_string()),
                args_hash,
                scope,
            ));
        }
    };

    if metadata.len() > MAX_FILE_BYTES && request.range.is_none() {
        return Ok(error_outcome(
            ToolName::Read,
            "文件过大，请指定行范围",
            ErrorCode::TooLarge,
            Some("read --range 1:200".to_string()),
            args_hash,
            scope,
        ));
    }

    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Ok(error_outcome(
                ToolName::Read,
                "读取文件失败",
                ErrorCode::Permission,
                Some(error.to_string()),
                args_hash,
                scope,
            ));
        }
    };

    if is_binary(&bytes) {
        return Ok(error_outcome(
            ToolName::Read,
            "二进制文件不返回内容",
            ErrorCode::Binary,
            None,
            args_hash,
            scope,
        ));
    }

    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let blob = loom_store::sha256_hex(&bytes)[..12].to_string();

    let requested = match request.range.as_deref() {
        Some(range) => match parse_range(range) {
            Ok(range) => Some(range),
            Err(error) => {
                return Ok(error_outcome(
                    ToolName::Read,
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
            ToolName::Read,
            "起始行超出文件范围",
            ErrorCode::InvalidArgs,
            Some(format!("file has {total} lines")),
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

    let truncated = cut
        || (requested.is_none() && actual_end < total)
        || requested.is_some_and(|(_, requested_end)| actual_end < requested_end.min(total));

    let dedupe_key = format!("read|{}|{}|{}|{}", request.path, blob, start, end);
    let seen = Seen::new(run);
    if let Some(previous_ref) = seen.get(&dedupe_key)? {
        let previous_path = run.dir().join(&previous_ref);
        if previous_path.exists() {
            let mut envelope = Envelope::new(
                ToolName::Read,
                format!("内容未变，本次运行已返回过（{}）", request.path),
            )
            .with_ref(previous_ref);
            envelope.unchanged = Some(true);
            envelope.stats = Some(Stats {
                n: Some(total),
                spill: Some(true),
                dedupe: Some(true),
                ..Stats::default()
            });
            envelope.finalize_stats();
            return Ok(ToolOutcome::new(envelope, args_hash, scope));
        }
    }

    let artifact_ref = match write_artifact(
        run,
        "read",
        &selected,
        &args_hash,
        if total == 0 { 0 } else { end - start + 1 },
    ) {
        Ok(artifact_ref) => artifact_ref,
        Err(error) => {
            return Ok(error_outcome(
                ToolName::Read,
                "写入 artifact 失败",
                ErrorCode::Internal,
                Some(error.to_string()),
                args_hash,
                scope,
            ));
        }
    };
    seen.mark(&dedupe_key, &artifact_ref)?;

    let summary = if total == 0 {
        format!("文件为空（{}）", request.path)
    } else {
        let suffix = if truncated {
            "，已截断；请用 --range 续读"
        } else {
            ""
        };
        format!(
            "已读取 {} 第 {}-{} 行（共 {} 行）{}",
            request.path, start, actual_end, total, suffix
        )
    };

    let mut envelope = Envelope::new(ToolName::Read, summary)
        .with_text(content.to_string())
        .with_ref(artifact_ref);
    envelope.truncated = Some(truncated);
    envelope.stats = Some(Stats {
        n: Some(total),
        spill: Some(true),
        ..Stats::default()
    });

    let item = Item {
        item_type: ItemType::File,
        path: Some(request.path.clone()),
        blob: Some(blob),
        range: (returned_lines > 0).then_some([start, actual_end]),
        symbol: None,
        kind: None,
        preview: None,
        score: None,
        count: Some(total),
        added: None,
        deleted: None,
        language: language_for(Path::new(&request.path)),
    };
    envelope.items = Some(vec![item]);

    envelope.finalize_stats();
    Ok(ToolOutcome::new(envelope, args_hash, scope))
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::Limits;

    #[test]
    fn reads_range_and_dedupes() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\n").expect("write");
        let project = Project::at(dir.path());
        let run = Run::open(project.clone(), Some("test")).expect("run");

        let request = ReadRequest {
            path: "a.txt".to_string(),
            range: Some("2:3".to_string()),
            max_lines: None,
        };
        let first = read(&project, &run, &request, &Limits::default()).expect("read");
        assert!(first.envelope.ok);
        assert_eq!(first.envelope.text.as_deref(), Some("two\nthree"));

        let second = read(&project, &run, &request, &Limits::default()).expect("read");
        assert_eq!(second.envelope.unchanged, Some(true));
        assert!(second.envelope.text.is_none());
    }
}
