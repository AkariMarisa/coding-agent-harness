//! Coarse workspace churn sampling for the external-adapter watchdog
//! (`doc/15` §1.6).
//!
//! External agents own their conversation, so Loom cannot project their
//! steps — but it can watch what they do to the working tree. The watchdog
//! only reports a *gross* loop: the same file changed in `changes`
//! **consecutive** samples (a missed sample resets the count), sustained for
//! at least `min_age`. Conservative by design — restarting an external agent
//! may re-read the whole repository, so near-misses are left alone.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use loom_core::ChurnConfig;

/// Never sampled: Loom's own state, git metadata, and the worktree registry
/// — their churn belongs to the control plane, not the agent.
const EXCLUDED_DIRS: [&str; 3] = [".git", ".loom", ".worktrees"];

#[derive(Debug, Clone)]
pub struct ChurnStop {
    /// Repo-relative path of the looping file.
    pub file: String,
    /// Consecutive samples in which it kept changing.
    pub changes: u32,
    /// Sum of size deltas observed while it looped.
    pub bytes_touched: u64,
}

pub struct ChurnWatch {
    root: PathBuf,
    interval: Duration,
    min_age: Duration,
    threshold: u32,
    started: Instant,
    next_sample: Instant,
    /// path → (mtime, size) as of the last sample.
    seen: HashMap<PathBuf, (u64, u64)>,
    /// path → consecutive samples with a change (reset when idle).
    counts: HashMap<PathBuf, u32>,
    /// path → accumulated |size delta| while looping.
    bytes: HashMap<PathBuf, u64>,
}

impl ChurnWatch {
    pub fn new(root: &Path, config: &ChurnConfig) -> Self {
        let now = Instant::now();
        Self {
            root: root.to_path_buf(),
            // 0 is valid here (tests sample on every poll call); real
            // configs are validated to 1..=3600 seconds.
            interval: Duration::from_secs(config.interval_s),
            min_age: Duration::from_secs(config.min_age_s),
            threshold: config.changes.max(2),
            started: now,
            // Baseline immediately, then every interval.
            next_sample: now,
            seen: HashMap::new(),
            counts: HashMap::new(),
            bytes: HashMap::new(),
        }
    }

    /// Take a sample when due; `Some(stop)` once a gross loop is proven.
    pub fn poll(&mut self) -> Option<ChurnStop> {
        let now = Instant::now();
        if now < self.next_sample {
            return None;
        }
        self.next_sample = now + self.interval;

        let mut current: HashMap<PathBuf, (u64, u64)> = HashMap::new();
        let mut changed: HashSet<PathBuf> = HashSet::new();
        self.walk(&self.root, &mut current, &mut changed);

        // Files that went quiet reset: the loop must be *sustained*.
        let stale: Vec<PathBuf> = self
            .counts
            .keys()
            .filter(|path| !changed.contains(*path))
            .cloned()
            .collect();
        for path in stale {
            self.counts.remove(&path);
            self.bytes.remove(&path);
        }
        for path in &changed {
            let count = self.counts.entry(path.clone()).or_insert(0);
            *count = count.saturating_add(1);
            let size = current.get(path).map(|(_, size)| *size).unwrap_or(0);
            let previous = self.seen.get(path).map(|(_, size)| *size).unwrap_or(size);
            let delta = size.abs_diff(previous);
            *self.bytes.entry(path.clone()).or_insert(0) += delta;
        }
        self.seen = current;

        if now.duration_since(self.started) < self.min_age {
            return None;
        }
        let (path, count) = self.counts.iter().max_by_key(|(_, count)| *count)?;
        if *count < self.threshold {
            return None;
        }
        let file = path
            .strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        Some(ChurnStop {
            file,
            changes: *count,
            bytes_touched: self.bytes.get(path).copied().unwrap_or(0),
        })
    }

    fn walk(
        &self,
        dir: &Path,
        current: &mut HashMap<PathBuf, (u64, u64)>,
        changed: &mut HashSet<PathBuf>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let name = entry.file_name();
                if EXCLUDED_DIRS.iter().any(|excluded| name == *excluded) {
                    continue;
                }
                self.walk(&path, current, changed);
            } else if file_type.is_file() {
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let Ok(modified) = meta.modified() else {
                    continue;
                };
                let mtime = modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0);
                let key = (mtime, meta.len());
                match self.seen.get(&path) {
                    // The first appearance of a file counts as a change
                    // (created or first observed mid-run).
                    None => {
                        changed.insert(path.clone());
                    }
                    Some(previous) if *previous != key => {
                        changed.insert(path.clone());
                    }
                    Some(_) => {}
                }
                current.insert(path, key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn config(changes: u32, interval_s: u64, min_age_s: u64) -> ChurnConfig {
        ChurnConfig {
            watchdog: true,
            changes,
            interval_s,
            min_age_s,
        }
    }

    fn sleep_ms(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    #[test]
    fn sustained_rewrite_of_one_file_trips_the_watchdog() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("loop.txt");
        fs::write(&target, "0").expect("write");
        let mut watch = ChurnWatch::new(dir.path(), &config(3, 0, 0));

        // Sample once to establish the baseline (count 1, below threshold).
        assert!(watch.poll().is_none(), "baseline never trips");
        for round in 1.. {
            sleep_ms(5);
            fs::write(&target, format!("round {round}")).expect("write");
            if let Some(stop) = watch.poll() {
                assert_eq!(stop.file, "loop.txt");
                assert!(stop.changes >= 3, "sustained count: {}", stop.changes);
                return;
            }
            assert!(round < 50, "watchdog never fired");
        }
    }

    #[test]
    fn one_off_edits_reset_and_never_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("edit.txt");
        fs::write(&target, "0").expect("write");
        let mut watch = ChurnWatch::new(dir.path(), &config(3, 0, 0));
        assert!(watch.poll().is_none(), "baseline");

        // One edit, then quiet: the count must reset on the idle sample.
        sleep_ms(5);
        fs::write(&target, "1").expect("write");
        assert!(watch.poll().is_none(), "one change is below threshold");
        sleep_ms(5);
        assert!(watch.poll().is_none(), "idle sample resets the count");
        sleep_ms(5);
        assert!(watch.poll().is_none(), "still quiet");
        assert!(watch.counts.is_empty(), "no lingering counts");
    }

    #[test]
    fn min_age_delays_the_verdict() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("slow.txt");
        fs::write(&target, "0").expect("write");
        // High min_age: even a hot loop must wait.
        let mut watch = ChurnWatch::new(dir.path(), &config(2, 0, 3_600));
        assert!(watch.poll().is_none());
        for round in 0..10 {
            sleep_ms(5);
            fs::write(&target, format!("{round}")).expect("write");
            assert!(watch.poll().is_none(), "min_age holds at round {round}");
        }
    }

    #[test]
    fn loom_state_and_git_are_never_sampled() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".loom/ci")).expect("mkdir");
        fs::create_dir_all(dir.path().join(".git/objects")).expect("mkdir");
        fs::write(dir.path().join(".loom/events.jsonl"), "a").expect("write");
        fs::write(dir.path().join(".git/objects/obj"), "b").expect("write");
        fs::write(dir.path().join("real.txt"), "0").expect("write");

        let mut watch = ChurnWatch::new(dir.path(), &config(2, 0, 0));
        assert!(watch.poll().is_none(), "baseline");
        sleep_ms(5);
        // churn only the excluded areas
        fs::write(dir.path().join(".loom/events.jsonl"), "aa").expect("write");
        fs::write(dir.path().join(".git/objects/obj"), "bb").expect("write");
        assert!(watch.poll().is_none(), "excluded dirs never trip");
        assert!(
            watch.counts.is_empty(),
            "excluded dirs are not even counted"
        );
    }
}
