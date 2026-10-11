//! Git blame support.
//!
//! gitoxide does not implement blame yet, so this module shells out to the `git`
//! binary in porcelin mode and parses the result. Blame is only computed when the
//! workspace is trusted, mirroring the gating of the other git operations.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Blame information for a single line of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlameLine {
    /// Full commit object id.
    pub commit: String,
    /// Author name.
    pub author: String,
    /// Author time as a Unix timestamp (seconds).
    pub author_time: i64,
    /// Commit summary (first line of the commit message).
    pub summary: String,
}

impl BlameLine {
    /// The abbreviated commit id, as displayed in the blame annotation.
    pub fn short_commit(&self) -> &str {
        let len = self.commit.len().min(8);
        &self.commit[..len]
    }
}

/// Runs `git blame --line-porcelain` for `file` and returns one entry per line.
pub fn blame_file(file: &Path, trust_full: bool) -> Result<Vec<BlameLine>> {
    debug_assert!(file.is_absolute());

    // Blame shells out to the user's git binary, so only do so for trusted workspaces.
    if !trust_full {
        bail!("workspace is not trusted, refusing to run git blame");
    }

    let repo_dir = file.parent().context("file has no parent directory")?;

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(["blame", "--line-porcelain", "--"])
        .arg(file)
        .output()
        .context("failed to run `git blame`; is git installed and on PATH?")?;

    if !output.status.success() {
        bail!(
            "git blame failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(parse_blame(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse the output of `git blame --line-porcelain`.
///
/// The format repeats the full commit metadata for every line, so each entry is:
/// a `<sha> <orig_line> <final_line> <count>` header, a set of `key value` metadata
/// lines, and finally a tab-prefixed content line that terminates the entry.
fn parse_blame(output: &str) -> Vec<BlameLine> {
    #[derive(Default)]
    struct Partial {
        commit: String,
        final_line: usize,
        author: String,
        author_time: i64,
        summary: String,
    }

    let mut result: Vec<Option<BlameLine>> = Vec::new();
    let mut current: Option<Partial> = None;

    for line in output.lines() {
        if line.starts_with('\t') {
            if let Some(partial) = current.take() {
                let idx = partial.final_line.saturating_sub(1);
                if result.len() <= idx {
                    result.resize_with(idx + 1, || None);
                }
                result[idx] = Some(BlameLine {
                    commit: partial.commit,
                    author: partial.author,
                    author_time: partial.author_time,
                    summary: partial.summary,
                });
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("author-mail ") {
            let _ = rest;
        } else if let Some(rest) = line.strip_prefix("author-time ") {
            if let Some(partial) = current.as_mut() {
                partial.author_time = rest.trim().parse().unwrap_or(0);
            }
        } else if let Some(rest) = line.strip_prefix("author ") {
            if let Some(partial) = current.as_mut() {
                partial.author = rest.to_string();
            }
        } else if let Some(rest) = line.strip_prefix("summary ") {
            if let Some(partial) = current.as_mut() {
                partial.summary = rest.to_string();
            }
        } else {
            // Candidate header: `<sha> <orig_line> <final_line> [<count>]`.
            let mut parts = line.split(' ');
            let hash = parts.next().unwrap_or("");
            let _orig = parts.next();
            let final_line = parts.next().and_then(|s| s.parse::<usize>().ok());
            if hash.len() >= 4 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                if let Some(final_line) = final_line {
                    current = Some(Partial {
                        commit: hash.to_string(),
                        final_line,
                        ..Default::default()
                    });
                }
            }
        }
    }

    result.into_iter().map(Option::unwrap_or_default).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_line_porcelain() {
        // Two lines, second one from the same commit but it is repeated for each line.
        let output = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 1\n\
author Alice\n\
author-mail <alice@example.com>\n\
author-time 1700000000\n\
author-tz +0000\n\
committer Alice\n\
committer-mail <alice@example.com>\n\
committer-time 1700000000\n\
committer-tz +0000\n\
summary initial commit\n\
filename src/main.rs\n\
\tfn main() {\n\
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 2 2 1\n\
author Bob\n\
author-mail <bob@example.com>\n\
author-time 1710000000\n\
author-tz +0000\n\
committer Bob\n\
committer-mail <bob@example.com>\n\
committer-time 1710000000\n\
committer-tz +0000\n\
summary fix a bug\n\
filename src/main.rs\n\
\t    println!(\"hi\");\n";

        let lines = parse_blame(output);
        assert_eq!(lines.len(), 2);

        assert_eq!(lines[0].author, "Alice");
        assert_eq!(lines[0].author_time, 1700000000);
        assert_eq!(lines[0].summary, "initial commit");
        assert_eq!(lines[0].short_commit(), "aaaaaaaa");

        assert_eq!(lines[1].author, "Bob");
        assert_eq!(lines[1].summary, "fix a bug");
        assert_eq!(lines[1].short_commit(), "bbbbbbbb");
    }

    #[test]
    fn ignores_extra_metadata_lines() {
        let output = "cccccccccccccccccccccccccccccccccccccccc 3 1 1\n\
boundary\n\
previous cccccccccccccccccccccccccccccccccccccccc src/main.rs\n\
author Carol\n\
author-time 1720000000\n\
summary touch file\n\
\tlet x = 1;\n";
        let lines = parse_blame(output);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].author, "Carol");
        assert_eq!(lines[0].commit, "cccccccccccccccccccccccccccccccccccccccc");
    }

    #[test]
    fn blame_file_reads_from_git() {
        use std::path::Path;
        use std::process::Command;

        fn git(dir: &Path, args: &[&str]) {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .env_remove("GIT_DIR")
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
                .env("GIT_CONFIG_VALUE_0", "false")
                .env("GIT_AUTHOR_NAME", "author")
                .env("GIT_AUTHOR_EMAIL", "author@example.com")
                .env("GIT_COMMITTER_NAME", "committer")
                .env("GIT_COMMITTER_EMAIL", "committer@example.com")
                .output()
                .expect("run git");
            assert!(status.status.success(), "git {args:?} failed");
        }

        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        git(dir.path(), &["config", "user.email", "test@helix.org"]);
        git(dir.path(), &["config", "user.name", "helix-test"]);

        let file = dir.path().join("file.txt");
        std::fs::write(&file, "first\nsecond\n").unwrap();
        git(dir.path(), &["add", "file.txt"]);
        git(dir.path(), &["commit", "-m", "initial commit"]);

        let lines = blame_file(&file, true).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].author, "author");
        assert_eq!(lines[0].summary, "initial commit");
        assert_eq!(lines[0], lines[1]);

        // Untrusted workspaces refuse to run git.
        assert!(blame_file(&file, false).is_err());
    }
}
