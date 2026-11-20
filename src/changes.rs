//! Changed lines, from a unified diff or from git. Only added lines of the new file count.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    /// Path to the changed lines of the new file, as inclusive ranges.
    files: HashMap<String, Vec<(u32, u32)>>,
}

impl Changes {
    /// Reads a unified diff. Paths are taken from the `+++` lines, without the `b/` prefix.
    pub fn from_diff(diff: &str) -> Self {
        let mut changes = Self::default();
        let mut path: Option<String> = None;
        // Lines that the current hunk still has, in the old and the new file. Outside a hunk both
        // are zero. A content line that starts with `+++` is therefore never read as a header.
        let (mut old_left, mut new_left, mut new_line) = (0u32, 0u32, 0u32);
        for line in diff.lines() {
            if old_left == 0 && new_left == 0 {
                if let Some(target) = line.strip_prefix("+++ ") {
                    let target = target.split('\t').next().unwrap_or(target);
                    path = (target != "/dev/null")
                        .then(|| target.strip_prefix("b/").unwrap_or(target).to_string());
                } else if let Some((old_count, new_start, new_count)) = hunk_header(line) {
                    (old_left, new_left, new_line) = (old_count, new_count, new_start);
                }
                continue;
            }
            match line.as_bytes().first() {
                Some(b'+') => {
                    if let Some(path) = &path {
                        changes.add(path, new_line, new_line);
                    }
                    new_left = new_left.saturating_sub(1);
                    new_line += 1;
                }
                Some(b'-') => old_left = old_left.saturating_sub(1),
                Some(b'\\') => {}
                _ => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                    new_line += 1;
                }
            }
        }
        changes
    }

    /// The working tree against `base`, with untracked files as fully changed. Paths are relative
    /// to `root`.
    pub fn from_git(root: &Path, base: &str) -> Result<Self> {
        if base.starts_with('-') {
            bail!("not a revision: {base}");
        }
        let diff = git(
            root,
            &[
                "diff",
                "-U0",
                "--no-color",
                "--no-ext-diff",
                "--relative",
                base,
                "--",
            ],
        )?;
        let mut changes = Self::from_diff(&diff);
        for path in git(root, &["ls-files", "--others", "--exclude-standard"])?.lines() {
            changes.add(path, 1, u32::MAX);
        }
        Ok(changes)
    }

    pub fn touches(&self, path: &str, start_line: u32, end_line: u32) -> bool {
        self.files.get(path).is_some_and(|ranges| {
            ranges
                .iter()
                .any(|(start, end)| *start <= end_line && start_line <= *end)
        })
    }

    fn add(&mut self, path: &str, start: u32, end: u32) {
        let ranges = self.files.entry(path.to_string()).or_default();
        match ranges.last_mut() {
            Some(last) if last.1.checked_add(1) == Some(start) => last.1 = end,
            _ => ranges.push((start, end)),
        }
    }
}

/// `@@ -a,b +c,d @@` gives `(b, c, d)`. A missing count is 1.
fn hunk_header(line: &str) -> Option<(u32, u32, u32)> {
    let mut parts = line.strip_prefix("@@ -")?.split(' ');
    let old = parts.next()?;
    let new = parts.next()?.strip_prefix('+')?;
    let count = |range: &str| match range.split_once(',') {
        Some((_, count)) => count.parse().ok(),
        None => Some(1),
    };
    let new_start = new.split(',').next()?.parse().ok()?;
    Some((count(old)?, new_start, count(new)?))
}

fn git(root: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off"])
        .args(arguments)
        .output()
        .context("could not run git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            arguments[0],
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/lib/a.ex b/lib/a.ex
index 1..2 100644
--- a/lib/a.ex
+++ b/lib/a.ex
@@ -10,3 +10,4 @@ defmodule A do
 context
-removed
+added one
+added two
 context
@@ -30 +31,0 @@
-only removed
diff --git a/lib/gone.ex b/lib/gone.ex
--- a/lib/gone.ex
+++ /dev/null
@@ -1,2 +0,0 @@
-x
-y
diff --git a/lib/new.ex b/lib/new.ex
--- /dev/null
+++ b/lib/new.ex
@@ -0,0 +1,2 @@
+++ not a header
+second
";

    #[test]
    fn only_added_lines_are_changes() {
        let changes = Changes::from_diff(DIFF);
        let expected = HashMap::from([
            ("lib/a.ex".to_string(), vec![(11, 12)]),
            ("lib/new.ex".to_string(), vec![(1, 2)]),
        ]);
        assert_eq!(changes.files, expected);
    }

    #[test]
    fn touches_when_a_range_overlaps_the_unit() {
        let changes = Changes::from_diff(DIFF);
        assert!(changes.touches("lib/a.ex", 12, 20));
        assert!(changes.touches("lib/a.ex", 1, 11));
        assert!(!changes.touches("lib/a.ex", 13, 20));
        assert!(!changes.touches("lib/a.ex", 1, 10));
        assert!(!changes.touches("lib/other.ex", 1, 100));
        assert!(!changes.touches("lib/gone.ex", 1, 2));
    }

    #[test]
    fn a_hunk_header_without_counts_means_one_line() {
        assert_eq!(hunk_header("@@ -5 +7 @@"), Some((1, 7, 1)));
        assert_eq!(hunk_header("@@ -5,0 +7,3 @@ tail"), Some((0, 7, 3)));
        assert_eq!(hunk_header("not a header"), None);
    }
}
