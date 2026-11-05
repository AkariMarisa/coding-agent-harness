use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use loom_core::{UsageAggregate, UsageFilter, UsageRecord};

use crate::Project;

/// Append-only request ledger. The JSONL file is the fact source; aggregates
/// are recomputed from it and never stored separately.
pub fn ledger_path(project: &Project) -> PathBuf {
    project.loom_dir().join("ledger").join("requests.jsonl")
}

pub fn append_usage(project: &Project, record: &UsageRecord) -> Result<()> {
    record
        .validate()
        .map_err(|message| anyhow::anyhow!("invalid usage record: {message}"))?;

    let path = ledger_path(project);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| "create ledger dir")?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    let line = serde_json::to_string(record)?;
    writeln!(file, "{line}").with_context(|| "append usage record")?;
    Ok(())
}

/// Read every valid row. Malformed lines fail loudly so accounting errors are
/// not hidden by a partial aggregate.
pub fn read_usage(project: &Project) -> Result<Vec<UsageRecord>> {
    let path = ledger_path(project);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str::<UsageRecord>(line).with_context(|| {
            format!("parse {} line {}", path.display(), index.saturating_add(1))
        })?;
        record.validate().map_err(|message| {
            anyhow::anyhow!(
                "validate {} line {}: {message}",
                path.display(),
                index.saturating_add(1)
            )
        })?;
        records.push(record);
    }
    Ok(records)
}

pub fn aggregate_usage(project: &Project, filter: &UsageFilter) -> Result<UsageAggregate> {
    let records = read_usage(project)?;
    Ok(loom_core::aggregate_usage(&records, filter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{CostSource, Tier, UsageSource};

    fn sample(run_id: &str, task_id: Option<&str>, cost: Option<f64>) -> UsageRecord {
        UsageRecord {
            run_id: run_id.into(),
            task_id: task_id.map(ToOwned::to_owned),
            tier: Some(Tier::Worker),
            model_alias: Some("worker".into()),
            input_tokens: Some(1000),
            cached_input_tokens: Some(400),
            output_tokens: Some(200),
            cost_usd: cost,
            cost_source: if cost.is_some() {
                CostSource::Provider
            } else {
                CostSource::Unknown
            },
            source: UsageSource::Litellm,
            ..UsageRecord::default()
        }
    }

    #[test]
    fn missing_ledger_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        assert!(read_usage(&project).expect("read").is_empty());
    }

    #[test]
    fn appends_and_reads_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        append_usage(&project, &sample("R-1", Some("T-1"), Some(0.01))).expect("append");
        append_usage(&project, &sample("R-1", Some("T-2"), None)).expect("append");

        let rows = read_usage(&project).expect("read");
        assert_eq!(rows.len(), 2);
        assert!(ledger_path(&project).exists());

        let filter = UsageFilter {
            task_id: Some("T-1".into()),
            ..UsageFilter::default()
        };
        let aggregate = aggregate_usage(&project, &filter).expect("aggregate");
        assert_eq!(aggregate.requests, 1);
    }

    #[test]
    fn rejects_invalid_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let mut record = sample("", None, Some(0.0));
        assert!(append_usage(&project, &record).is_err());

        record = sample("R-1", None, Some(-1.0));
        record.run_id = "R-1".into();
        assert!(append_usage(&project, &record).is_err());
    }

    #[test]
    fn malformed_line_fails_loudly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let path = ledger_path(&project);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "{not json}\n").expect("write");
        assert!(read_usage(&project).is_err());
    }

    #[test]
    fn unknown_version_fails_on_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = Project::at(dir.path());
        let path = ledger_path(&project);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &path,
            r#"{"v":2,"ts_ms":0,"run_id":"R-1","cost_source":"unknown","source":"unknown"}"#,
        )
        .expect("write");
        let error = read_usage(&project).expect_err("v2 must not be silently mixed");
        assert!(format!("{error:#}").contains("unsupported usage version"));
    }
}
