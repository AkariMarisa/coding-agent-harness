use std::path::Path;

use loom_core::truncate_chars;

pub(crate) fn language_for(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let language = match ext.as_str() {
        "rs" => "rust",
        "ts" => "ts",
        "tsx" => "tsx",
        "js" => "js",
        "jsx" => "jsx",
        "py" => "py",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "c" => "c",
        "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "sh" | "bash" => "shell",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "md" => "markdown",
        "css" => "css",
        "html" => "html",
        "sql" => "sql",
        "swift" => "swift",
        "lua" => "lua",
        "dart" => "dart",
        _ => return None,
    };
    Some(language.to_string())
}

pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

pub(crate) fn preview(line: &str) -> String {
    truncate_chars(line.trim(), 160).0.to_string()
}
