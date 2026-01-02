mod commit_message_generator;
mod diff;

use std::{
    env,
    env::var,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use commit_message_generator::{
    CommitMessageGenerator, collapse_patterns, max_diff_bytes, max_diff_lines,
    max_total_diff_bytes, max_total_diff_lines,
};
use diff::{FileChangeSummary, build_collapse_matcher, get_file_change_summary, get_tree_diff};
use gethostname::gethostname;
use jj_lib::{
    config::{ConfigLayer, ConfigResolutionContext, ConfigSource, StackedConfig, resolve},
    gitignore::GitIgnoreFile,
    merged_tree::MergedTree,
    object_id::ObjectId,
    repo::{Repo, StoreFactories},
    settings::UserSettings,
    working_copy::SnapshotOptions,
    workspace::{Workspace, default_working_copy_factories},
};
use tracing::{debug, info, trace};
use tracing_subscriber::fmt;

#[derive(Parser, Debug)]
#[command(author, version, about = "Auto-commit changes in a jj workspace using Claude for commit messages", long_about = None)]
struct Args {
    /// Path to the workspace (defaults to current directory)
    #[arg(short, long)]
    path: Option<PathBuf>,

    /// Language to use for commit messages
    #[arg(short, long, default_value = "English", env = "CCC_JJ_LANGUAGE")]
    language: String,

    /// Model to use for generating a commit message
    #[arg(short, long, default_value = "haiku", env = "CCC_JJ_MODEL")]
    model: String,
}

/// Load user configuration from standard jj config locations
fn load_user_config(config: &mut StackedConfig) -> Result<()> {
    if let Some(home_dir) = dirs::home_dir() {
        // Try to load from ~/.jjconfig.toml
        let home_config = home_dir.join(".jjconfig.toml");
        if home_config.exists() {
            let layer = ConfigLayer::load_from_file(ConfigSource::User, home_config)?;
            config.add_layer(layer);
        }

        // Try to load from ~/.config/jj/config.toml (XDG-style on Unix)
        let xdg_config = home_dir.join(".config").join("jj").join("config.toml");
        if xdg_config.exists() {
            let layer = ConfigLayer::load_from_file(ConfigSource::User, xdg_config)?;
            config.add_layer(layer);
        }
    }

    // Also try platform-specific config directory (for Windows/macOS)
    if let Some(config_dir) = dirs::config_dir() {
        let platform_config = config_dir.join("jj").join("config.toml");
        if platform_config.exists() {
            let layer = ConfigLayer::load_from_file(ConfigSource::User, platform_config)?;
            config.add_layer(layer);
        }
    }

    Ok(())
}

/// Load gitignore files from global and workspace locations
fn load_base_ignores(workspace_root: &Path) -> Result<Arc<GitIgnoreFile>> {
    let mut git_ignores = GitIgnoreFile::empty();

    // Try to get global excludes file from git config
    let global_excludes = get_global_git_excludes_file();

    if let Some(excludes_path) = global_excludes {
        // Chain the global excludes file (ignore errors if file doesn't exist)
        git_ignores = git_ignores.chain_with_file("", excludes_path).unwrap_or(git_ignores);
    }

    // Load workspace root .gitignore
    let workspace_gitignore = workspace_root.join(".gitignore");
    git_ignores = git_ignores
        .chain_with_file("", workspace_gitignore)
        .unwrap_or(git_ignores);

    Ok(git_ignores)
}

/// Get the global git excludes file path
fn get_global_git_excludes_file() -> Option<PathBuf> {
    // First, try to get from git config
    if let Ok(output) = Command::new("git")
        .args(["config", "--global", "--get", "core.excludesFile"])
        .output()
        && output.status.success()
        && let Ok(path_str) = std::str::from_utf8(&output.stdout)
    {
        let path_str = path_str.trim();
        if !path_str.is_empty() {
            // Expand ~ to home directory if present
            let expanded = if let Some(stripped) = path_str.strip_prefix("~/") {
                if let Some(home) = dirs::home_dir() {
                    home.join(stripped)
                } else {
                    PathBuf::from(path_str)
                }
            } else {
                PathBuf::from(path_str)
            };
            return Some(expanded);
        }
    }

    // Fall back to XDG_CONFIG_HOME/git/ignore or ~/.config/git/ignore
    if let Ok(xdg_config) = var("XDG_CONFIG_HOME")
        && !xdg_config.is_empty()
    {
        let path = PathBuf::from(xdg_config).join("git").join("ignore");
        if path.exists() {
            return Some(path);
        }
    }

    // Final fallback: ~/.config/git/ignore
    if let Some(home) = dirs::home_dir() {
        let path = home.join(".config").join("git").join("ignore");
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Discover the jj workspace starting from the given directory
fn find_workspace(start_dir: &Path) -> Result<Workspace> {
    // First, find the workspace root directory
    let mut current_dir = start_dir;
    let workspace_root = loop {
        if current_dir.join(".jj").exists() {
            break current_dir;
        }

        match current_dir.parent() {
            Some(parent) => current_dir = parent,
            None => bail!(
                "No Jujutsu workspace found in '{}' or any parent directory",
                start_dir.display()
            ),
        }
    };

    // Build config with proper layers (with_defaults includes operation.hostname/username)
    let mut config = StackedConfig::with_defaults();

    // Load user configuration
    load_user_config(&mut config)?;

    // Load repository-specific configuration
    let repo_config_path = workspace_root.join(".jj").join("repo").join("config.toml");
    if repo_config_path.exists() {
        let layer = ConfigLayer::load_from_file(ConfigSource::Repo, repo_config_path)?;
        config.add_layer(layer);
    }

    // Resolve conditional scopes (e.g., --when.repositories)
    let hostname = gethostname().to_str().map(|s| s.to_owned()).unwrap_or_default();
    let home_dir = dirs::home_dir();
    let context = ConfigResolutionContext {
        home_dir: home_dir.as_deref(),
        repo_path: Some(workspace_root),
        workspace_path: Some(workspace_root),
        command: None,
        hostname: hostname.as_str(),
    };
    let resolved_config = resolve(&config, &context)?;

    // Now create settings with resolved config
    let settings = UserSettings::from_config(resolved_config)?;
    let store_factories = StoreFactories::default();
    let working_copy_factories = default_working_copy_factories();

    // Load the workspace with the complete settings
    Workspace::load(&settings, workspace_root, &store_factories, &working_copy_factories)
        .context("Failed to load workspace")
}

/// Create a commit with the generated message
async fn create_commit(
    workspace: &Workspace,
    commit_message: &str,
    tree: MergedTree,
    file_changes: &FileChangeSummary,
) -> Result<()> {
    let repo = workspace.repo_loader().load_at_head()?;

    // Start transaction
    let mut tx = repo.start_transaction();
    let mut_repo = tx.repo_mut();

    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(workspace.workspace_name())
        .context("workspace should have a working-copy commit")?;
    let wc_commit = repo.store().get_commit(wc_commit_id)?;

    // Rewrite the working copy commit with the description and snapshotted tree
    let commit_with_description = mut_repo
        .rewrite_commit(&wc_commit)
        .set_tree(tree.clone())
        .set_description(commit_message)
        .write()?;

    // Rebase descendants (handles the rewrite)
    mut_repo.rebase_descendants()?;

    // Create a new empty working copy commit on top
    let new_wc_commit = mut_repo
        .new_commit(vec![commit_with_description.id().clone()], tree)
        .write()?;

    mut_repo.set_wc_commit(workspace.workspace_name().to_owned(), new_wc_commit.id().clone())?;

    let new_repo = tx.commit("auto-commit via ccc-jj")?;

    // Finish the working copy with the new state
    let locked_wc = workspace.working_copy().start_mutation()?;
    locked_wc.finish(new_repo.operation().id().clone()).await?;

    let author = commit_with_description.author();
    println!(
        "Committed change {} by {} <{}>",
        commit_with_description.id().hex(),
        author.name,
        author.email
    );

    // Collect all lines for the box
    let mut lines: Vec<&str> = vec![];
    for line in commit_message.lines() {
        lines.push(line);
    }

    // Calculate box width based on content
    let content_width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let box_width = content_width.max(40); // minimum width of 40

    // Print the box
    println!("╭{}╮", "─".repeat(box_width + 2));
    for line in &lines {
        let padding = box_width - line.chars().count();
        println!("│ {line}{} │", " ".repeat(padding));
    }
    println!("╰{}╯", "─".repeat(box_width + 2));

    // Print file changes below the box
    print!("{file_changes}");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    let args = Args::parse();
    debug!(?args, "Parsed arguments");

    // Determine workspace path
    let workspace_path = match args.path {
        Some(p) => p,
        None => env::current_dir().context("Failed to get current directory")?,
    };
    info!(?workspace_path, "Starting workspace discovery");

    // Find workspace
    let workspace = find_workspace(&workspace_path)?;
    info!(workspace_root = ?workspace.workspace_root(), "Found workspace");

    // Check if working copy commit needs a description
    let repo = workspace.repo_loader().load_at_head()?;
    debug!("Loaded repository at head");

    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(workspace.workspace_name())
        .context("workspace should have a working-copy commit")?;
    let wc_commit = repo.store().get_commit(wc_commit_id)?;
    debug!(wc_commit_id = %wc_commit_id.hex(), "Working copy commit");

    // Snapshot to get the actual filesystem state
    debug!("Starting working copy mutation");
    let mut locked_wc = workspace.working_copy().start_mutation()?;

    // Load gitignore files (global and workspace .gitignore)
    let base_ignores = load_base_ignores(workspace.workspace_root())?;
    debug!("Loaded base ignores");

    let snapshot_options = SnapshotOptions {
        base_ignores,
        progress: None,
        start_tracking_matcher: &jj_lib::matchers::EverythingMatcher,
        force_tracking_matcher: &jj_lib::matchers::NothingMatcher,
        max_new_file_size: 1024 * 1024 * 100,
    };
    debug!("Taking snapshot of working copy");
    let (current_tree, _stats) = locked_wc.snapshot(&snapshot_options).await?;
    debug!("Snapshot complete");

    // Check if the working copy commit has changes (compared to its parent)
    let parent_tree = if !wc_commit.parent_ids().is_empty() {
        let parent_commit = repo.store().get_commit(&wc_commit.parent_ids()[0])?;
        parent_commit.tree()
    } else {
        jj_lib::merged_tree::MergedTree::resolved(
            repo.store().clone(),
            repo.store().empty_tree_id().clone(),
        )
    };

    // If working copy tree matches parent tree, there's nothing to commit
    if current_tree.tree_ids() == parent_tree.tree_ids() {
        println!("No changes detected, nothing to commit");
        drop(locked_wc);
        return Ok(());
    }
    debug!("Changes detected in working copy");

    // If working copy commit already has a description, don't overwrite it
    if !wc_commit.description().is_empty() {
        info!(description = %wc_commit.description(), "Working copy already has description, skipping");
        drop(locked_wc);
        return Ok(());
    }

    // Generate diff for commit message (using jj-lib API, not external command)
    debug!("Generating diff");
    let collapse_matcher = build_collapse_matcher(collapse_patterns());
    let diff = get_tree_diff(
        &repo,
        &parent_tree,
        &current_tree,
        collapse_matcher.as_ref(),
        max_diff_lines(),
        max_diff_bytes(),
    )
    .await?;
    debug!(diff_len = diff.len(), "Diff generated");
    trace!(diff = %diff, "Full diff content");

    if diff.trim().is_empty() {
        println!("Empty diff, nothing to commit");
        drop(locked_wc);
        return Ok(());
    }

    // Check total diff size before sending to Claude
    let diff_lines = diff.lines().count();
    let diff_bytes = diff.len();
    let max_lines = max_total_diff_lines();
    let max_bytes = max_total_diff_bytes();

    if diff_lines > max_lines || diff_bytes > max_bytes {
        drop(locked_wc);
        bail!(
            "Diff too large to generate commit message: {diff_lines} lines / {diff_bytes} bytes (limits: {max_lines} lines / {max_bytes} bytes). \
            Consider committing in smaller chunks or using `jj describe` to set the message manually."
        );
    }

    // Drop the lock before calling Claude (external process)
    drop(locked_wc);

    // Generate a commit message and create commit
    info!(language = %args.language, model = %args.model, "Generating commit message with Claude");
    let generator = CommitMessageGenerator::new(&args.language, &args.model);
    let commit_message = match generator.generate(&diff) {
        Some(msg) => msg,
        None => {
            bail!("Failed to generate commit message, aborting commit");
        }
    };
    debug!(commit_message = %commit_message, "Generated commit message");

    // Get file change summary for display
    let file_changes = get_file_change_summary(&parent_tree, &current_tree).await;

    info!("Creating commit");
    create_commit(&workspace, &commit_message, current_tree, &file_changes).await?;
    info!("Commit created successfully");

    Ok(())
}
