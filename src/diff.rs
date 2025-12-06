use std::fmt::Write;

use anyhow::Result;
use futures::StreamExt;
use globset::{Glob, GlobSet, GlobSetBuilder};
use jj_lib::{
    backend::{FileId, TreeValue},
    merged_tree::MergedTree,
    repo::{ReadonlyRepo, Repo},
    repo_path::RepoPath,
};
use similar::TextDiff;
use tokio::{io::AsyncReadExt, try_join};
use tracing::{debug, trace, warn};

const MAX_LINES: usize = 50;
const CONTEXT_LINES: usize = 2;

/// Build a GlobSet from pattern strings
pub fn build_collapse_matcher(patterns: &[String]) -> Option<GlobSet> {
    if patterns.is_empty() {
        return None;
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        match Glob::new(pattern) {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(e) => {
                warn!(pattern = %pattern, error = %e, "Invalid collapse pattern, skipping");
            }
        }
    }

    match builder.build() {
        Ok(set) => Some(set),
        Err(e) => {
            warn!(error = %e, "Failed to build collapse matcher");
            None
        }
    }
}

/// Read file content from store
async fn read_file_content(repo: &ReadonlyRepo, path: &RepoPath, id: &FileId) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    repo.store()
        .read_file(path, id)
        .await?
        .read_to_end(&mut content)
        .await?;
    Ok(content)
}

/// Format file diff (added/removed) with line truncation
async fn format_added_removed_diff(
    repo: &ReadonlyRepo,
    path: &RepoPath,
    path_str: &str,
    id: &FileId,
    is_added: bool,
    max_lines: usize,
) -> Result<String> {
    let (status, from, to) = if is_added {
        ("new file", "/dev/null".to_string(), format!("b/{path_str}"))
    } else {
        ("deleted file", format!("a/{path_str}"), "/dev/null".to_string())
    };

    let mut output = format!("diff --git a/{path_str} b/{path_str}\n{status}\n--- {from}\n+++ {to}\n");
    let content = read_file_content(repo, path, id).await?;

    match String::from_utf8(content) {
        Ok(text) => {
            let lines: Vec<_> = text.lines().collect();
            let prefix = if is_added { '+' } else { '-' };

            lines.iter().take(max_lines).for_each(|line| {
                let _ = writeln!(output, "{prefix}{line}");
            });

            if lines.len() > max_lines {
                let _ = writeln!(output, "... ({} more lines)", lines.len() - max_lines);
            }
        }
        Err(_) => writeln!(output, "(binary file)")?,
    }

    Ok(output)
}

/// Format a collapsed summary for files matching collapse patterns
fn format_collapsed_summary(path_str: &str, added: usize, removed: usize, status: &str) -> String {
    format!(
        "diff --git a/{0} b/{0}\n{1} (+{2} -{3} lines, collapsed)\n",
        path_str, status, added, removed
    )
}

/// Get the diff between two trees using jj-lib
pub async fn get_tree_diff(
    repo: &ReadonlyRepo,
    from_tree: &MergedTree,
    to_tree: &MergedTree,
    collapse_matcher: Option<&GlobSet>,
) -> Result<String> {
    debug!("Starting tree diff");
    let mut output = String::new();
    let mut stream = from_tree.diff_stream(to_tree, &jj_lib::matchers::EverythingMatcher);
    let mut file_count = 0;
    let mut collapsed_count = 0;

    while let Some(entry) = stream.next().await {
        let path_str = entry.path.as_internal_file_string();
        let values = entry.values?;

        // Check if this file should be collapsed
        let should_collapse = collapse_matcher
            .map(|m| m.is_match(path_str))
            .unwrap_or(false);

        let diff_output = match (values.before.as_resolved(), values.after.as_resolved()) {
            (None, Some(Some(TreeValue::File { id, .. }))) => {
                trace!(path = %path_str, collapsed = should_collapse, "Processing added file");
                if should_collapse {
                    let content = read_file_content(repo, &entry.path, id).await?;
                    let line_count = String::from_utf8_lossy(&content).lines().count();
                    collapsed_count += 1;
                    format_collapsed_summary(path_str, line_count, 0, "new file")
                } else {
                    format_added_removed_diff(repo, &entry.path, path_str, id, true, MAX_LINES)
                        .await?
                }
            }

            (Some(Some(TreeValue::File { id, .. })), None) => {
                trace!(path = %path_str, collapsed = should_collapse, "Processing deleted file");
                if should_collapse {
                    let content = read_file_content(repo, &entry.path, id).await?;
                    let line_count = String::from_utf8_lossy(&content).lines().count();
                    collapsed_count += 1;
                    format_collapsed_summary(path_str, 0, line_count, "deleted file")
                } else {
                    format_added_removed_diff(repo, &entry.path, path_str, id, false, MAX_LINES)
                        .await?
                }
            }

            (
                Some(Some(TreeValue::File { id: before_id, .. })),
                Some(Some(TreeValue::File { id: after_id, .. })),
            ) => {
                trace!(path = %path_str, collapsed = should_collapse, "Processing modified file");
                let (before_content, after_content) = try_join!(
                    read_file_content(repo, &entry.path, before_id),
                    read_file_content(repo, &entry.path, after_id)
                )?;

                match (
                    String::from_utf8(before_content),
                    String::from_utf8(after_content),
                ) {
                    (Ok(before_text), Ok(after_text)) => {
                        if should_collapse {
                            let diff = TextDiff::from_lines(&before_text, &after_text);
                            let added = diff.iter_all_changes().filter(|c| c.tag() == similar::ChangeTag::Insert).count();
                            let removed = diff.iter_all_changes().filter(|c| c.tag() == similar::ChangeTag::Delete).count();
                            collapsed_count += 1;
                            format_collapsed_summary(path_str, added, removed, "modified")
                        } else {
                            let diff = TextDiff::from_lines(&before_text, &after_text);
                            format!(
                                "diff --git a/{0} b/{0}\n{1}",
                                path_str,
                                diff.unified_diff()
                                    .context_radius(CONTEXT_LINES)
                                    .header(&format!("a/{path_str}"), &format!("b/{path_str}"))
                            )
                        }
                    }
                    _ => {
                        trace!(path = %path_str, "Binary file modified");
                        format!("diff --git a/{path_str} b/{path_str}\n(binary file modified)\n")
                    }
                }
            }
            _ => String::new(),
        };

        if !diff_output.is_empty() {
            file_count += 1;
            output.push_str(&diff_output);
        }
    }

    debug!(file_count, collapsed_count, output_len = output.len(), "Tree diff complete");
    Ok(output)
}
