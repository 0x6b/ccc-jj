use std::fmt::Write;

use anyhow::Result;
use futures::StreamExt;
use jj_lib::backend::TreeValue;
use jj_lib::repo::{ReadonlyRepo, Repo};
use similar::TextDiff;
use tokio::io::AsyncReadExt;
use tokio::try_join;
use tracing::{debug, trace};

const MAX_LINES: usize = 50;
const CONTEXT_LINES: usize = 2;

/// Read file content from store
async fn read_file_content(
    repo: &ReadonlyRepo,
    path: &jj_lib::repo_path::RepoPath,
    id: &jj_lib::backend::FileId,
) -> Result<Vec<u8>> {
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
    path: &jj_lib::repo_path::RepoPath,
    path_str: &str,
    id: &jj_lib::backend::FileId,
    is_added: bool,
    max_lines: usize,
) -> Result<String> {
    let (status, from, to) = if is_added {
        ("new file", "/dev/null".to_string(), format!("b/{path_str}"))
    } else {
        (
            "deleted file",
            format!("a/{path_str}"),
            "/dev/null".to_string(),
        )
    };

    let mut output = format!(
        "diff --git a/{0} b/{0}\n{status}\n--- {from}\n+++ {to}\n",
        path_str
    );
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

/// Get the diff between two trees using jj-lib
pub async fn get_tree_diff(
    repo: &ReadonlyRepo,
    from_tree: &jj_lib::merged_tree::MergedTree,
    to_tree: &jj_lib::merged_tree::MergedTree,
) -> Result<String> {
    debug!("Starting tree diff");
    let mut output = String::new();
    let mut stream = from_tree.diff_stream(to_tree, &jj_lib::matchers::EverythingMatcher);
    let mut file_count = 0;

    while let Some(entry) = stream.next().await {
        let path_str = entry.path.as_internal_file_string();
        let values = entry.values?;

        let diff_output = match (values.before.as_resolved(), values.after.as_resolved()) {
            (None, Some(Some(TreeValue::File { id, .. }))) => {
                trace!(path = %path_str, "Processing added file");
                format_added_removed_diff(repo, &entry.path, &path_str, id, true, MAX_LINES).await?
            }

            (Some(Some(TreeValue::File { id, .. })), None) => {
                trace!(path = %path_str, "Processing deleted file");
                format_added_removed_diff(repo, &entry.path, &path_str, id, false, MAX_LINES)
                    .await?
            }

            (
                Some(Some(TreeValue::File { id: before_id, .. })),
                Some(Some(TreeValue::File { id: after_id, .. })),
            ) => {
                trace!(path = %path_str, "Processing modified file");
                let (before_content, after_content) = try_join!(
                    read_file_content(repo, &entry.path, before_id),
                    read_file_content(repo, &entry.path, after_id)
                )?;

                match (
                    String::from_utf8(before_content),
                    String::from_utf8(after_content),
                ) {
                    (Ok(before_text), Ok(after_text)) => {
                        let diff = TextDiff::from_lines(&before_text, &after_text);
                        format!(
                            "diff --git a/{0} b/{0}\n{1}",
                            path_str,
                            diff.unified_diff()
                                .context_radius(CONTEXT_LINES)
                                .header(&format!("a/{path_str}"), &format!("b/{path_str}"))
                        )
                    }
                    _ => {
                        trace!(path = %path_str, "Binary file modified");
                        format!("diff --git a/{0} b/{0}\n(binary file modified)\n", path_str)
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

    debug!(file_count, output_len = output.len(), "Tree diff complete");
    Ok(output)
}
