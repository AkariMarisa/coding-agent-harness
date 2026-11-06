use std::process::ExitCode;

use loom_native::NATIVE_VERSION;

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("loom-native {NATIVE_VERSION}");
        return ExitCode::SUCCESS;
    }

    match loom_native::run_from_env() {
        Ok(outcome) => {
            let payload = serde_json::json!({
                "status": outcome.status,
                "steps": outcome.steps,
                "input_tokens": outcome.usage.input_tokens,
                "cached_input_tokens": outcome.usage.cached_input_tokens,
                "output_tokens": outcome.usage.output_tokens,
                "cost_usd": outcome.usage.cost_usd,
                "verify": outcome.verify.iter().map(|item| serde_json::json!({
                    "id": item.id,
                    "result": item.result,
                    "exit": item.exit,
                })).collect::<Vec<_>>(),
                "summary": outcome.summary,
            });
            match serde_json::to_string_pretty(&payload) {
                Ok(text) => println!("{text}"),
                Err(error) => eprintln!("loom-native: serialize outcome: {error}"),
            }
            ExitCode::from(outcome.exit_code.clamp(0, 255) as u8)
        }
        Err(error) => {
            eprintln!("loom-native: {error:#}");
            ExitCode::from(4)
        }
    }
}
