use std::str::FromStr;

/// Rough token estimate. Deliberately dependency-free; calibrated later with
/// provider usage. ASCII is ~4 bytes/token; CJK is ~1 token/char.
pub fn estimate_tokens(text: &str) -> usize {
    let mut ascii = 0usize;
    let mut cjk = 0usize;
    let mut other = 0usize;

    for ch in text.chars() {
        if ch.is_ascii() {
            ascii += 1;
        } else if is_cjk(ch) {
            cjk += 1;
        } else {
            other += 1;
        }
    }

    ascii.div_ceil(4) + cjk + other.div_ceil(2)
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x2EFF
            | 0x3000..=0x303F
            | 0x3040..=0x30FF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF00..=0xFFEF
            | 0x20000..=0x2FA1F
    )
}

/// Truncate at a UTF-8 char boundary. Returns the slice and whether it was cut.
pub fn truncate_bytes(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }

    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// Truncate by character count. Returns the slice and whether it was cut.
pub fn truncate_chars(text: &str, max_chars: usize) -> (&str, bool) {
    if text.chars().count() <= max_chars {
        return (text, false);
    }

    let end = text
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    (&text[..end], true)
}

/// Parse a 1-based inclusive `start:end` range.
pub fn parse_range(input: &str) -> Result<(usize, usize), String> {
    let (start, end) = input
        .split_once(':')
        .ok_or_else(|| "range must be start:end".to_string())?;

    let start = usize::from_str(start.trim()).map_err(|_| "invalid range start".to_string())?;
    let end = usize::from_str(end.trim()).map_err(|_| "invalid range end".to_string())?;

    if start == 0 || end == 0 {
        return Err("range is 1-based".to_string());
    }
    if end < start {
        return Err("range end must be >= start".to_string());
    }

    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_ascii_and_cjk() {
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("中文字"), 3);
        assert!(estimate_tokens("中文 abc") >= 3);
    }

    #[test]
    fn truncates_on_char_boundary() {
        let (text, cut) = truncate_bytes("中文abc", 4);
        assert!(cut);
        assert_eq!(text, "中");
    }

    #[test]
    fn parses_range() {
        assert_eq!(parse_range("1:200").expect("range"), (1, 200));
        assert!(parse_range("0:1").is_err());
        assert!(parse_range("9:2").is_err());
        assert!(parse_range("1").is_err());
    }
}
