use serde::{Deserialize, Serialize};

/// Soft defaults are configurable down. Hard caps are compiled in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub call_bytes: usize,
    pub invocation_bytes: usize,
    pub run_bytes: usize,
    pub search_results: usize,
    pub search_bytes: usize,
    pub read_lines: usize,
    pub read_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            call_bytes: 8 * 1024,
            invocation_bytes: 24 * 1024,
            run_bytes: 128 * 1024,
            search_results: 20,
            search_bytes: 4 * 1024,
            read_lines: 120,
            read_bytes: 8 * 1024,
        }
    }
}

impl Limits {
    pub const HARD_CALL_BYTES: usize = 16 * 1024;
    pub const HARD_INVOCATION_BYTES: usize = 48 * 1024;
    pub const HARD_RUN_BYTES: usize = 256 * 1024;
    pub const HARD_SEARCH_RESULTS: usize = 50;
    pub const HARD_SEARCH_BYTES: usize = 8 * 1024;
    pub const HARD_READ_LINES: usize = 200;
    pub const HARD_READ_BYTES: usize = 12 * 1024;

    /// Clamp every value to a safe range. Config can lower, never raise past hard caps.
    pub fn sanitized(&self) -> Self {
        Self {
            call_bytes: clamp(self.call_bytes, 512, Self::HARD_CALL_BYTES),
            invocation_bytes: clamp(
                self.invocation_bytes,
                self.call_bytes.max(512),
                Self::HARD_INVOCATION_BYTES,
            ),
            run_bytes: clamp(
                self.run_bytes,
                self.invocation_bytes.max(512),
                Self::HARD_RUN_BYTES,
            ),
            search_results: clamp(self.search_results, 1, Self::HARD_SEARCH_RESULTS),
            search_bytes: clamp(self.search_bytes, 256, Self::HARD_SEARCH_BYTES),
            read_lines: clamp(self.read_lines, 10, Self::HARD_READ_LINES),
            read_bytes: clamp(self.read_bytes, 512, Self::HARD_READ_BYTES),
        }
    }
}

fn clamp(value: usize, min: usize, max: usize) -> usize {
    value.max(min).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_caps_win() {
        let limits = Limits {
            call_bytes: usize::MAX,
            invocation_bytes: usize::MAX,
            run_bytes: usize::MAX,
            search_results: usize::MAX,
            search_bytes: usize::MAX,
            read_lines: usize::MAX,
            read_bytes: usize::MAX,
        }
        .sanitized();

        assert_eq!(limits.call_bytes, Limits::HARD_CALL_BYTES);
        assert_eq!(limits.invocation_bytes, Limits::HARD_INVOCATION_BYTES);
        assert_eq!(limits.run_bytes, Limits::HARD_RUN_BYTES);
        assert_eq!(limits.search_results, Limits::HARD_SEARCH_RESULTS);
        assert_eq!(limits.search_bytes, Limits::HARD_SEARCH_BYTES);
        assert_eq!(limits.read_lines, Limits::HARD_READ_LINES);
        assert_eq!(limits.read_bytes, Limits::HARD_READ_BYTES);
    }
}
