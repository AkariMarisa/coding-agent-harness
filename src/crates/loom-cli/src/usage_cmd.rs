use std::fs;

use anyhow::{Context as _, Result};
use loom_core::{CostSource, UsageRecord, UsageSource};
use loom_store::{append_usage, now_ms, Project};
use serde_json::json;
use serde_json::Value;

use super::{Format, UsageIngestArgs};

pub fn ingest(
    project: &Project,
    args: UsageIngestArgs,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    let text =
        fs::read_to_string(&args.file).with_context(|| format!("read {}", args.file.display()))?;
    let config = project.load_config()?;
    let default_source = args
        .source
        .parse::<UsageSource>()
        .map_err(|message| anyhow::anyhow!(message))?;

    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row = serde_json::from_str::<Value>(line)
            .with_context(|| format!("parse {} line {}", args.file.display(), index + 1))?;

        let row_run = string_field(&row, &["run_id", "run"])
            .or_else(|| run_id.map(ToOwned::to_owned))
            .or_else(|| args.run.clone())
            .unwrap_or_else(|| "local".to_string());
        let row_task = string_field(&row, &["task_id", "task"]).or_else(|| args.task.clone());

        let mut record = UsageRecord::new(row_run);
        record.ts_ms = u64_field(&row, &["ts_ms"]).unwrap_or_else(now_ms);
        record.task_id = row_task;
        record.attempt = u64_field(&row, &["attempt"]).map(|value| value as u32);
        record.tier = string_field(&row, &["tier"])
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|message: String| anyhow::anyhow!(message))?;
        record.input_tokens = u64_field(&row, &["input_tokens", "prompt_tokens", "input"]);
        record.cached_input_tokens = u64_field(
            &row,
            &[
                "cached_input_tokens",
                "cache_read_input_tokens",
                "cached_tokens",
            ],
        )
        .or_else(|| nested_u64(&row, &["prompt_tokens_details", "cached_tokens"]));
        record.output_tokens = u64_field(&row, &["output_tokens", "completion_tokens", "output"]);
        record.reasoning_tokens = u64_field(&row, &["reasoning_tokens"])
            .or_else(|| nested_u64(&row, &["completion_tokens_details", "reasoning_tokens"]));
        record.cost_usd = number_field(&row, &["cost_usd", "cost", "response_cost"]);
        record.latency_ms = u64_field(&row, &["latency_ms", "duration_ms"])
            .or_else(|| number_field(&row, &["latency_s"]).map(|secs| (secs * 1000.0) as u64));
        record.status =
            u64_field(&row, &["status", "http_status", "status_code"]).map(|value| value as u16);
        // Step-level churn attribution (`doc/16` §8.4); optional for
        // one-shot rows and third-party JSONL.
        record.step = u64_field(&row, &["step"]).map(|value| value as u32);
        record.tool_names = string_list(&row, "tool_names");
        record.write_paths = string_list(&row, "write_paths");
        record.rewrite_count = u64_field(&row, &["rewrite_count"]).map(|value| value as u32);
        record.history_bytes = u64_field(&row, &["history_bytes"]);
        record.projected_bytes = u64_field(&row, &["projected_bytes"]);

        let source = string_field(&row, &["source"])
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|message: String| anyhow::anyhow!(message))?
            .unwrap_or(default_source);
        record.source = source;

        let cost_source = string_field(&row, &["cost_source"])
            .or_else(|| args.cost_source.clone())
            .or_else(|| record.cost_usd.map(|_| "provider".to_string()));
        record.cost_source = cost_source
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|message: String| anyhow::anyhow!(message))?
            .unwrap_or(CostSource::Unknown);

        // Resolve model alias / capability bits.
        let model_field = string_field(&row, &["model", "model_name", "model_id"]);
        let capability = model_field.as_deref().and_then(|value| {
            config.model(value).or_else(|| {
                config
                    .models
                    .iter()
                    .find(|model| model.model_id.as_deref() == Some(value))
            })
        });
        if let Some(model_field) = model_field.as_deref() {
            if capability.is_some() {
                record.model_alias = Some(model_field.to_string());
            } else {
                record.model_id = Some(model_field.to_string());
            }
        }
        if let Some(capability) = capability {
            if record.tier.is_none() {
                record.tier = Some(capability.tier);
            }
            if record.wire.is_none() {
                record.wire = Some(capability.wire);
            }
            if record.model_id.is_none() {
                record.model_id = capability.model_id.clone();
            }
            if record.provider.is_none() {
                record.provider = capability.provider.clone();
            }
        }

        record.fill_estimated_cost(capability);

        append_usage(project, &record).with_context(|| {
            format!(
                "append usage from {} line {}",
                args.file.display(),
                index + 1
            )
        })?;
        records.push(record);
    }

    let total_cost: f64 = records.iter().filter_map(|record| record.cost_usd).sum();
    let input: u64 = records
        .iter()
        .filter_map(|record| record.input_tokens)
        .sum();
    let output: u64 = records
        .iter()
        .filter_map(|record| record.output_tokens)
        .sum();

    match format {
        Format::Json => {
            let payload = json!({
                "file": args.file.display().to_string(),
                "imported": records.len(),
                "input_tokens": input,
                "output_tokens": output,
                "cost_usd": total_cost,
                "unattributed": records.iter().filter(|record| !record.is_attributed()).count(),
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => {
            println!(
                "imported {} rows from {}",
                records.len(),
                args.file.display()
            );
            println!("tokens:  input={input} output={output}");
            println!("cost:    ${total_cost:.6}");
            let unattributed = records
                .iter()
                .filter(|record| !record.is_attributed())
                .count();
            if unattributed > 0 {
                println!("warning: {unattributed} rows are unattributed (missing task_id)");
            }
        }
    }
    Ok(0)
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .filter(|text| !text.trim().is_empty())
    })
}

fn string_list(value: &Value, key: &str) -> Option<Vec<String>> {
    let items = value.get(key)?.as_array()?;
    let list: Vec<String> = items
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect();
    (!list.is_empty()).then_some(list)
}

fn number_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(as_number))
}

fn u64_field(value: &Value, keys: &[&str]) -> Option<u64> {
    number_field(value, keys).map(|number| number.max(0.0) as u64)
}

fn nested_number(value: &Value, path: &[&str]) -> Option<f64> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    as_number(current)
}

fn nested_u64(value: &Value, path: &[&str]) -> Option<u64> {
    nested_number(value, path).map(|number| number.max(0.0) as u64)
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}
