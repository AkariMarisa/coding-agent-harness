use anyhow::Result;
use loom_context::{get, list, PromptRole};

use super::{Format, PromptCommand};

pub fn prompt(
    project: &loom_store::Project,
    command: PromptCommand,
    format: Format,
) -> Result<i32> {
    // Prompt library is built into the binary; the project only selects lang.
    let _ = project;
    match command {
        PromptCommand::List => {
            let templates = list();
            match format {
                Format::Json => {
                    let entries: Vec<_> = templates
                        .iter()
                        .map(|template| {
                            serde_json::json!({
                                "id": template.id(),
                                "role": template.role.as_str(),
                                "version": template.version,
                                "description": template.description,
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                }
                Format::Pretty => {
                    for template in templates {
                        println!(
                            "{:<18} v{}  {}",
                            template.id(),
                            template.version,
                            template.description
                        );
                    }
                }
            }
            Ok(0)
        }
        PromptCommand::Show { role, raw } => {
            let role = role
                .parse::<PromptRole>()
                .map_err(|message| anyhow::anyhow!(message))?;
            let template = get(role)?;
            if raw {
                print!("{}", template.body);
                if !template.body.ends_with('\n') {
                    println!();
                }
            } else {
                match format {
                    Format::Json => {
                        let payload = serde_json::json!({
                            "id": template.id(),
                            "role": template.role.as_str(),
                            "version": template.version,
                            "description": template.description,
                            "body": template.body,
                        });
                        println!("{}", serde_json::to_string_pretty(&payload)?);
                    }
                    Format::Pretty => {
                        println!(
                            "{} v{} — {}",
                            template.id(),
                            template.version,
                            template.description
                        );
                        println!("{}", "-".repeat(60));
                        print!("{}", template.body);
                        if !template.body.ends_with('\n') {
                            println!();
                        }
                    }
                }
            }
            Ok(0)
        }
    }
}
