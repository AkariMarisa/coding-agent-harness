//! Step-level token attribution and churn accounting (`doc/16` §8.4).
//!
//! Everything here is an **estimate** derived from serialized history bytes and
//! write events; provider usage stays the fact source. The report is written to
//! `logs/churn.json` per run and read back by `loom churn stats`.
//!
//! Why these numbers: a full-file rewrite is paid once as output, then again on
//! every following request while the old version sits in history. Blocking the
//! rewrite saves the output; projecting the old version out of history saves
//! the carry. The two levers are measured separately so the A/B run can say
//! which one actually moved.

use serde::Serialize;

use crate::tools::WriteEvent;

/// Bytes per estimated token for ASCII-heavy JSON history. Kept in one place so
/// project.rs and churn use the same rough ratio.
const BYTES_PER_TOKEN: f64 = 4.0;

/// One step's request/attribution row. Mirrors what lands in `usage.jsonl`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepChurn {
    pub step: u32,
    pub tool_names: Vec<String>,
    pub write_paths: Vec<String>,
    pub rewrite_count: u32,
    pub history_bytes: u64,
    pub projected_bytes: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

impl StepChurn {
    pub fn avoided_tokens_est(&self) -> u64 {
        ((self.history_bytes.saturating_sub(self.projected_bytes)) as f64 / BYTES_PER_TOKEN).round()
            as u64
    }
}

/// Per-run churn report written to `logs/churn.json`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChurnReport {
    pub v: u8,
    pub run_id: String,
    pub task_id: String,
    pub attempt: u32,
    pub model_alias: String,
    pub model_id: String,
    /// Every derived number below is an estimate; usage/billing is the truth.
    pub estimated: bool,
    pub steps: u32,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Serialized history over all steps, before and after projection.
    pub full_history_bytes: u64,
    pub projected_history_bytes: u64,
    /// Output paid for repeated full-file writes.
    pub rewrite_output_tokens: u64,
    /// Old file versions re-sent on later requests.
    pub history_carry_tokens: u64,
    /// Input kept out of later requests by projection.
    pub avoided_input_tokens: u64,
    /// Extra cost of invalidating a warm prefix cache after a rewrite.
    pub cache_invalidation_cost_est: f64,
    pub wasted_tokens_est: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasted_cost_usd_est: Option<f64>,
    pub write_passes: u32,
    pub rewrite_passes: u32,
    pub write_paths: Vec<String>,
    pub churn_ratio: f64,
}

/// Price inputs for converting token waste into dollars. `None` prices mean the
/// dollar estimate is `unknown`, never zero.
#[derive(Debug, Clone, Copy, Default)]
pub struct PriceHint {
    pub input_usd_per_mtok: Option<f64>,
    pub cached_input_usd_per_mtok: Option<f64>,
    pub cache_write_usd_per_mtok: Option<f64>,
    pub output_usd_per_mtok: Option<f64>,
}

impl PriceHint {
    fn input(&self) -> Option<f64> {
        self.cached_input_usd_per_mtok.or(self.input_usd_per_mtok)
    }
}

pub struct ChurnInput<'a> {
    pub run_id: &'a str,
    pub task_id: &'a str,
    pub attempt: u32,
    pub model_alias: &'a str,
    pub model_id: &'a str,
    pub steps: u32,
    pub step_rows: &'a [StepChurn],
    pub write_log: &'a [WriteEvent],
    pub prices: PriceHint,
}

pub fn build(input: ChurnInput<'_>) -> ChurnReport {
    let rewrite_events: Vec<&WriteEvent> = input
        .write_log
        .iter()
        .filter(|event| event.count > 1)
        .collect();

    let rewrite_output_tokens: u64 = rewrite_events
        .iter()
        .map(|event| bytes_to_tokens(event.bytes as u64))
        .sum();

    // A rewrite at step S is re-sent on every later request, so its carry is
    // payload × remaining steps. This is the number projection actually attacks.
    let history_carry_tokens: u64 = rewrite_events
        .iter()
        .map(|event| {
            let remaining = input.steps.saturating_sub(event.step) as u64;
            bytes_to_tokens(event.bytes as u64).saturating_mul(remaining)
        })
        .sum();

    let full_history_bytes: u64 = input.step_rows.iter().map(|row| row.history_bytes).sum();
    let projected_history_bytes: u64 = input.step_rows.iter().map(|row| row.projected_bytes).sum();
    let avoided_input_tokens: u64 = input
        .step_rows
        .iter()
        .map(StepChurn::avoided_tokens_est)
        .sum();

    let rewritten_bytes: u64 = rewrite_events.iter().map(|event| event.bytes as u64).sum();
    let cache_invalidation_cost_est = match input.prices.cache_write_usd_per_mtok {
        Some(price) => bytes_to_tokens(rewritten_bytes) as f64 * price / 1_000_000.0,
        None => 0.0,
    };

    let wasted_tokens_est = rewrite_output_tokens
        .saturating_add(history_carry_tokens)
        .saturating_sub(avoided_input_tokens);

    let wasted_cost_usd_est = match (input.prices.input(), input.prices.output_usd_per_mtok) {
        (Some(input_price), Some(output_price)) => {
            let rewrite_cost = rewrite_output_tokens as f64 * output_price / 1_000_000.0;
            let carry_cost = history_carry_tokens as f64 * input_price / 1_000_000.0;
            let avoided_cost = avoided_input_tokens as f64 * input_price / 1_000_000.0;
            Some((rewrite_cost + carry_cost - avoided_cost).max(0.0) + cache_invalidation_cost_est)
        }
        _ => None,
    };

    let mut write_paths: Vec<String> = input
        .write_log
        .iter()
        .map(|event| event.path.clone())
        .collect();
    write_paths.sort();
    write_paths.dedup();

    let (input_tokens, cached_input_tokens, output_tokens) =
        input
            .step_rows
            .iter()
            .fold((0u64, 0u64, 0u64), |(input_sum, cached, output), row| {
                (
                    input_sum + row.input_tokens,
                    cached + row.cached_input_tokens,
                    output + row.output_tokens,
                )
            });

    let churn_ratio = if input_tokens + output_tokens == 0 {
        0.0
    } else {
        wasted_tokens_est as f64 / (input_tokens + output_tokens) as f64
    };

    ChurnReport {
        v: 1,
        run_id: input.run_id.to_string(),
        task_id: input.task_id.to_string(),
        attempt: input.attempt,
        model_alias: input.model_alias.to_string(),
        model_id: input.model_id.to_string(),
        estimated: true,
        steps: input.steps,
        input_tokens,
        cached_input_tokens,
        output_tokens,
        cost_usd: None,
        full_history_bytes,
        projected_history_bytes,
        rewrite_output_tokens,
        history_carry_tokens,
        avoided_input_tokens,
        cache_invalidation_cost_est,
        wasted_tokens_est,
        wasted_cost_usd_est,
        write_passes: input.write_log.len() as u32,
        rewrite_passes: rewrite_events.len() as u32,
        write_paths,
        churn_ratio,
    }
}

fn bytes_to_tokens(bytes: u64) -> u64 {
    (bytes as f64 / BYTES_PER_TOKEN).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &str, bytes: usize, count: u32, step: u32) -> WriteEvent {
        WriteEvent {
            path: path.to_string(),
            bytes,
            count,
            step,
        }
    }

    #[test]
    fn separates_rewrite_output_from_history_carry() {
        let writes = vec![
            write("src/a.rs", 4000, 1, 1),
            write("src/a.rs", 4000, 2, 3),
            write("src/a.rs", 4000, 3, 5),
        ];
        let rows = vec![StepChurn {
            step: 1,
            history_bytes: 10_000,
            projected_bytes: 8_000,
            input_tokens: 2_500,
            output_tokens: 1_000,
            ..StepChurn::default()
        }];
        let report = build(ChurnInput {
            run_id: "R",
            task_id: "T",
            attempt: 1,
            model_alias: "worker",
            model_id: "deepseek-chat",
            steps: 6,
            step_rows: &rows,
            write_log: &writes,
            prices: PriceHint {
                input_usd_per_mtok: Some(0.27),
                cached_input_usd_per_mtok: Some(0.07),
                cache_write_usd_per_mtok: Some(0.27),
                output_usd_per_mtok: Some(1.10),
            },
        });

        // Two rewrites × 1000 tokens of output.
        assert_eq!(report.rewrite_output_tokens, 2_000);
        // Step 3 rewrite carries 3 requests, step 5 carries 1: 1000 × 4.
        assert_eq!(report.history_carry_tokens, 4_000);
        assert_eq!(report.avoided_input_tokens, 500);
        assert_eq!(report.wasted_tokens_est, 5_500);
        assert_eq!(report.write_passes, 3);
        assert_eq!(report.rewrite_passes, 2);
        assert_eq!(report.write_paths, vec!["src/a.rs".to_string()]);
        assert!(report.wasted_cost_usd_est.unwrap() > 0.0);
        assert!(report.estimated);
    }

    #[test]
    fn unknown_prices_stay_unknown() {
        let report = build(ChurnInput {
            run_id: "R",
            task_id: "T",
            attempt: 1,
            model_alias: "worker",
            model_id: "unknown",
            steps: 2,
            step_rows: &[],
            write_log: &[],
            prices: PriceHint::default(),
        });
        assert_eq!(report.wasted_cost_usd_est, None);
        assert_eq!(report.churn_ratio, 0.0);
    }

    #[test]
    fn projection_benefit_reduces_waste_but_never_goes_negative() {
        let rows = vec![StepChurn {
            step: 1,
            history_bytes: 10_000,
            projected_bytes: 1_000,
            input_tokens: 100,
            output_tokens: 10,
            ..StepChurn::default()
        }];
        let report = build(ChurnInput {
            run_id: "R",
            task_id: "T",
            attempt: 1,
            model_alias: "worker",
            model_id: "m",
            steps: 1,
            step_rows: &rows,
            write_log: &[],
            prices: PriceHint::default(),
        });
        assert_eq!(report.wasted_tokens_est, 0);
        assert_eq!(report.avoided_input_tokens, 2_250);
    }
}
