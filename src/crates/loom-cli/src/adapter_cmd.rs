use anyhow::Result;
use loom_adapter::{canary, list, AdapterKind};
use serde_json::json;

use super::dispatch_cmd;
use super::{AdapterCommand, Format};

pub fn adapter(
    project: &loom_store::Project,
    command: AdapterCommand,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    match command {
        AdapterCommand::List => {
            let presets = list();
            match format {
                Format::Json => {
                    let entries: Vec<_> = presets
                        .iter()
                        .map(|preset| {
                            json!({
                                "adapter": preset.kind.as_str(),
                                "executable": preset.executable,
                                "available": preset.is_available(),
                                "description": preset.description,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                }
                Format::Pretty => {
                    for preset in presets {
                        println!(
                            "{:<10} {:<10} {}",
                            preset.kind.as_str(),
                            if preset.is_available() {
                                "available"
                            } else {
                                "missing"
                            },
                            preset.description
                        );
                    }
                }
            }
            Ok(0)
        }
        AdapterCommand::Canary {
            adapter,
            live,
            task,
        } => {
            let kind: AdapterKind = adapter
                .parse()
                .map_err(|message: String| anyhow::anyhow!(message))?;

            let report = if live {
                let task = task.ok_or_else(|| anyhow::anyhow!("--live requires --task"))?;
                let prepared = dispatch_cmd::prepare(
                    project,
                    &task,
                    kind.as_str(),
                    None,
                    Some(60),
                    false,
                    run_id,
                    1,
                )?;
                canary(kind, true, Some(&prepared.request))?
            } else {
                canary(kind, false, None)?
            };

            match format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&report.to_json())?),
                Format::Pretty => {
                    println!(
                        "canary {} [{}]",
                        report.adapter.as_str(),
                        if report.ok { "ok" } else { "FAIL" }
                    );
                    for check in &report.checks {
                        println!(
                            "  [{}] {:<16} {}",
                            if check.ok { "ok" } else { "FAIL" },
                            check.name,
                            check.detail
                        );
                    }
                }
            }
            Ok(if report.ok { 0 } else { 1 })
        }
    }
}
