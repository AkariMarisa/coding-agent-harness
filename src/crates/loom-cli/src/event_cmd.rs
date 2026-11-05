use anyhow::Result;
use loom_core::{Event, EventFilter, EventType};
use loom_store::read_events;

use super::{EventCommand, Format};

pub fn events(
    project: &loom_store::Project,
    command: EventCommand,
    run_id: Option<&str>,
    format: Format,
) -> Result<i32> {
    match command {
        EventCommand::List {
            task,
            event_type,
            limit,
        } => {
            let event_type = event_type
                .as_deref()
                .map(|name| name.parse::<EventType>())
                .transpose()
                .map_err(|message| anyhow::anyhow!(message))?;
            let filter = EventFilter {
                run_id: run_id.map(ToOwned::to_owned),
                task_id: task,
                event_type,
            };
            let mut all = read_events(project, &filter)?;
            if let Some(limit) = limit {
                let start = all.len().saturating_sub(limit);
                all = all.split_off(start);
            }

            match format {
                Format::Json => {
                    let lines: Vec<String> = all
                        .iter()
                        .map(|event| serde_json::to_string(event).unwrap_or_default())
                        .collect();
                    println!("{}", lines.join("\n"));
                }
                Format::Pretty => {
                    if all.is_empty() {
                        println!("no events");
                        return Ok(0);
                    }
                    for event in &all {
                        print_event(event);
                    }
                }
            }
            Ok(0)
        }
    }
}

fn print_event(event: &Event) {
    let task = event.task_id.as_deref().unwrap_or("-");
    let attempt = event
        .attempt
        .map(|value| value.to_string())
        .unwrap_or_default();
    let mut data = String::new();
    for (key, value) in &event.data {
        if data.is_empty() {
            data.push(' ');
        } else {
            data.push_str(", ");
        }
        data.push_str(&format!("{key}={value}"));
    }
    println!(
        "{:>18} {:<18} {:<17} {}{}",
        event.run_id,
        task,
        event.event_type.as_str(),
        attempt,
        data
    );
}
