//! Minimal YAML frontmatter for `SKILL.md` (`name` + `description` only).
//!
//! v1 is deliberately strict: single-line values, exactly the two keys the
//! trigger budgets need. A folded/multi-line value is an error rather than
//! a silent truncation — `doc/16` §9.3 wants a trustworthy
//! `description_chars`.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
}

pub fn parse_frontmatter(text: &str) -> Result<Frontmatter, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err("missing opening `---` frontmatter".to_string());
    }

    let mut fields: HashMap<String, String> = HashMap::new();
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(
                "multi-line frontmatter values are not supported; keep name/description on one line"
                    .to_string(),
            );
        }
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_string(), unquote(value.trim()));
        }
    }
    if !closed {
        return Err("unterminated frontmatter (no closing `---`)".to_string());
    }

    let name = fields
        .get("name")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "frontmatter `name` must be present and non-empty".to_string())?;
    let description = fields
        .get("description")
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "frontmatter `description` must be present and non-empty".to_string())?;

    Ok(Frontmatter {
        name: name.to_string(),
        description: description.to_string(),
    })
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if value.len() >= 2
        && ((bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\''))
    {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_and_bare_values() {
        let fm = parse_frontmatter(
            "---\nname: docs-writing\ndescription: \"Use when writing. Do not use for chat.\"\n---\nbody\n",
        )
        .expect("parse");
        assert_eq!(fm.name, "docs-writing");
        assert_eq!(fm.description, "Use when writing. Do not use for chat.");
        assert_eq!(fm.description.chars().count(), 38);
    }

    #[test]
    fn rejects_missing_or_empty_keys() {
        assert!(parse_frontmatter("no frontmatter at all").is_err());
        assert!(
            parse_frontmatter("---\nname: x\n---\n").is_err(),
            "no description"
        );
        assert!(parse_frontmatter("---\nname: \ndescription: y\n---\n").is_err());
        assert!(
            parse_frontmatter("---\nname: x\ndescription: y\n").is_err(),
            "unterminated"
        );
    }

    #[test]
    fn rejects_multi_line_values_instead_of_truncating() {
        let err = parse_frontmatter("---\nname: x\ndescription: |\n  folded\n---\n")
            .expect_err("multiline");
        assert!(err.contains("multi-line"));
    }
}
