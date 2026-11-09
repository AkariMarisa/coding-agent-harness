//! `loom churn stats` — aggregate per-run `logs/churn.json` estimates.
//!
//! The report is diagnostic, not billing. Token and cost columns are the
//! ledger's job; here the first screen answers "where did avoidable tokens go"
//! and only the second screen talks about write counts (`doc/16` §8.4).

use std::fs;
use std::path::Path;

use anyhow::{bail, Context as _, Result};
use loom_store::Project;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Format;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct ChurnFile {
    v: u8,
    run_id: String,
    task_id: String,
    attempt: u32,
    model_alias: String,
    model_id: String,
    estimated: bool,
    steps: u32,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    cost_usd: Option<f64>,
    full_history_bytes: u64,
    projected_history_bytes: u64,
    rewrite_output_tokens: u64,
    history_carry_tokens: u64,
    avoided_input_tokens: u64,
    cache_invalidation_cost_est: f64,
    wasted_tokens_est: u64,
    wasted_cost_usd_est: Option<f64>,
    write_passes: u32,
    rewrite_passes: u32,
    write_paths: Vec<String>,
    churn_ratio: f64,
}

#[derive(Debug, Clone, Default)]
struct Totals {
    runs: u64,
    steps: u64,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    cost_usd: Option<f64>,
    full_history_bytes: u64,
    projected_history_bytes: u64,
    rewrite_output_tokens: u64,
    history_carry_tokens: u64,
    avoided_input_tokens: u64,
    cache_invalidation_cost_est: f64,
    wasted_tokens_est: u64,
    wasted_cost_usd_est: Option<f64>,
    write_passes: u64,
    rewrite_passes: u64,
    write_paths: Vec<String>,
}

impl Totals {
    fn add(&mut self, row: &ChurnFile) {
        self.runs += 1;
        self.steps += row.steps as u64;
        self.input_tokens += row.input_tokens;
        self.cached_input_tokens += row.cached_input_tokens;
        self.output_tokens += row.output_tokens;
        if let Some(cost) = row.cost_usd {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
        }
        self.full_history_bytes += row.full_history_bytes;
        self.projected_history_bytes += row.projected_history_bytes;
        self.rewrite_output_tokens += row.rewrite_output_tokens;
        self.history_carry_tokens += row.history_carry_tokens;
        self.avoided_input_tokens += row.avoided_input_tokens;
        self.cache_invalidation_cost_est += row.cache_invalidation_cost_est;
        self.wasted_tokens_est += row.wasted_tokens_est;
        if let Some(cost) = row.wasted_cost_usd_est {
            self.wasted_cost_usd_est = Some(self.wasted_cost_usd_est.unwrap_or(0.0) + cost);
        }
        self.write_passes += row.write_passes as u64;
        self.rewrite_passes += row.rewrite_passes as u64;
        for path in &row.write_paths {
            if !self.write_paths.contains(path) {
                self.write_paths.push(path.clone());
            }
        }
    }

    fn churn_ratio(&self) -> f64 {
        let denominator = self.input_tokens + self.output_tokens;
        if denominator == 0 {
            0.0
        } else {
            self.wasted_tokens_est as f64 / denominator as f64
        }
    }
}

pub fn stats(project: &Project, by: Option<&str>, format: Format) -> Result<i32> {
    if let Some(by) = by {
        if by != "model" {
            bail!("churn stats only supports --by model, got {by}");
        }
    }

    let rows = read_reports(&project.loom_dir().join("runs"))?;
    let mut overall = Totals::default();
    let mut groups: Vec<(String, Totals)> = Vec::new();
    for row in &rows {
        overall.add(row);
        if by == Some("model") {
            let key = if row.model_id.is_empty() || row.model_id == row.model_alias {
                row.model_alias.clone()
            } else {
                format!("{} ({})", row.model_alias, row.model_id)
            };
            match groups.iter_mut().find(|(name, _)| name == &key) {
                Some((_, totals)) => totals.add(row),
                None => {
                    let mut totals = Totals::default();
                    totals.add(row);
                    groups.push((key, totals));
                }
            }
        }
    }

    match format {
        Format::Json => {
            let groups_json = groups
                .iter()
                .map(|(name, totals)| (name.clone(), totals_json(totals)))
                .collect::<serde_json::Map<_, _>>();
            let payload = json!({
                "runs": overall.runs,
                "estimated": true,
                "totals": totals_json(&overall),
                "by_model": groups_json,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        Format::Pretty => {
            if overall.runs == 0 {
                println!("churn: no run logs yet (logs/churn.json is written by native runs)");
                return Ok(0);
            }
            println!(
                "churn: {} run(s), {} step(s), estimates only (usage/ledger is the truth)",
                overall.runs, overall.steps
            );
            println!(
                "tokens:  input={} cached={} output={}",
                overall.input_tokens, overall.cached_input_tokens, overall.output_tokens
            );
            println!(
                "split:   rewrite_output={} history_carry={} avoided_input={}",
                overall.rewrite_output_tokens,
                overall.history_carry_tokens,
                overall.avoided_input_tokens
            );
            println!(
                "waste:   {} tokens ({} est) ratio {:.1}%",
                overall.wasted_tokens_est,
                match overall.wasted_cost_usd_est {
                    Some(cost) => format!("${cost:.6}"),
                    None => "unknown".to_string(),
                },
                overall.churn_ratio() * 100.0,
            );
            if overall.cache_invalidation_cost_est > 0.0 {
                println!(
                    "cache:   ${:.6} invalidation est",
                    overall.cache_invalidation_cost_est
                );
            }
            println!(
                "writes:  {} pass(es), {} rewrite(s), {} file(s)",
                overall.write_passes,
                overall.rewrite_passes,
                overall.write_paths.len()
            );
            if by == Some("model") {
                for (name, totals) in &groups {
                    println!(
                        "  {name:<28} runs={} input={} waste={} ratio={:.1}%",
                        totals.runs,
                        totals.input_tokens,
                        totals.wasted_tokens_est,
                        totals.churn_ratio() * 100.0
                    );
                }
            }
        }
    }
    Ok(0)
}

fn totals_json(totals: &Totals) -> serde_json::Value {
    json!({
        "runs": totals.runs,
        "steps": totals.steps,
        "input_tokens": totals.input_tokens,
        "cached_input_tokens": totals.cached_input_tokens,
        "output_tokens": totals.output_tokens,
        "cost_usd": totals.cost_usd,
        "full_history_bytes": totals.full_history_bytes,
        "projected_history_bytes": totals.projected_history_bytes,
        "rewrite_output_tokens": totals.rewrite_output_tokens,
        "history_carry_tokens": totals.history_carry_tokens,
        "avoided_input_tokens": totals.avoided_input_tokens,
        "cache_invalidation_cost_est": totals.cache_invalidation_cost_est,
        "wasted_tokens_est": totals.wasted_tokens_est,
        "wasted_cost_usd_est": totals.wasted_cost_usd_est,
        "write_passes": totals.write_passes,
        "rewrite_passes": totals.rewrite_passes,
        "write_paths": totals.write_paths,
        "churn_ratio": totals.churn_ratio(),
    })
}

fn read_reports(runs_dir: &Path) -> Result<Vec<ChurnFile>> {
    if !runs_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut reports = Vec::new();
    for entry in fs::read_dir(runs_dir).with_context(|| format!("read {}", runs_dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("logs").join("churn.json");
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let report = serde_json::from_str::<ChurnFile>(&text)
            .with_context(|| format!("parse {}", path.display()))?;
        reports.push(report);
    }
    reports.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(run_id: &str, model: &str, waste: u64, rewrites: u32) -> ChurnFile {
        ChurnFile {
            v: 1,
            run_id: run_id.into(),
            task_id: "T".into(),
            attempt: 1,
            model_alias: model.into(),
            model_id: format!("{model}-id"),
            estimated: true,
            steps: 5,
            input_tokens: 1_000,
            cached_input_tokens: 400,
            output_tokens: 100,
            full_history_bytes: 2_000,
            projected_history_bytes: 1_000,
            rewrite_output_tokens: waste / 2,
            history_carry_tokens: waste,
            avoided_input_tokens: 100,
            wasted_tokens_est: waste,
            write_passes: 3,
            rewrite_passes: rewrites,
            write_paths: vec!["src/a.rs".into()],
            ..ChurnFile::default()
        }
    }

    #[test]
    fn totals_and_model_grouping_add_up() {
        let mut overall = Totals::default();
        let rows = [
            report("R-1", "worker", 200, 1),
            report("R-2", "worker", 400, 2),
        ];
        let mut groups: Vec<(String, Totals)> = Vec::new();
        for row in &rows {
            overall.add(row);
            match groups.iter_mut().find(|(name, _)| name == &row.model_alias) {
                Some((_, totals)) => totals.add(row),
                None => {
                    let mut totals = Totals::default();
                    totals.add(row);
                    groups.push((row.model_alias.clone(), totals));
                }
            }
        }
        assert_eq!(overall.runs, 2);
        assert_eq!(overall.wasted_tokens_est, 600);
        assert_eq!(overall.rewrite_passes, 3);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1.input_tokens, 2_000);
        assert_eq!(overall.write_paths, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn missing_runs_dir_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reports = read_reports(&dir.path().join("absent")).expect("read");
        assert!(reports.is_empty());
    }
}
