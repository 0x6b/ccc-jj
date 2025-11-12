mod commit_message_generator;
mod diff;

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
use jj_lib::object_id::ObjectId;
use jj_lib::repo::{Repo, StoreFactories};
use jj_lib::settings::UserSettings;
use jj_lib::workspace::{default_working_copy_factories, Workspace};
use jj_lib::working_copy::SnapshotOptions;

use commit_message_generator::CommitMessageGenerator;
use diff::get_tree_diff;

#[derive(Parser, Debug)]
#[command(author, version, about = "Auto-commit changes in a jj workspace using Claude for commit messages", long_about = None)]
struct Args {
    /// Path to the workspace (defaults to current directory)
    #[arg(short, long)]
    path: Option<PathBuf>,

    /// Language to use for commit messages
    #[arg(short, long, default_value = "English", env = "CCC_JJ_LANGUAGE")]
    language: String,
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

/// Discover the jj workspace starting from the given directory
fn find_workspace(start_dir: &Path) -> Result<Workspace> {
    // Build config with proper layers (with_defaults includes operation.hostname/username)
    let mut config = StackedConfig::with_defaults();

    // Load user configuration
    load_user_config(&mut config)?;

    let settings = UserSettings::from_config(config)?;
    let store_factories = StoreFactories::default();
    let working_copy_factories = default_working_copy_factories();

    let workspace = Workspace::load(
        &settings,
        start_dir,
        &store_factories,
        &working_copy_factories,
    )
    .context("Failed to load workspace")?;

    println!("Found workspace at: {}", workspace.workspace_root().display());
    Ok(workspace)
}

/// Create a commit with the generated message
async fn create_commit(
    workspace: &Workspace,
    commit_message: &str,
    tree: jj_lib::merged_tree::MergedTree,
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

    println!("Committed change {} with message:", commit_with_description.id().hex());
    println!("{}", commit_message);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Determine workspace path
    let workspace_path = match args.path {
        Some(p) => p,
        None => env::current_dir().context("Failed to get current directory")?,
    };

    // Find workspace
    let workspace = find_workspace(&workspace_path)?;

    // Check if working copy commit needs a description
    println!("Checking for changes...");
    let repo = workspace.repo_loader().load_at_head()?;
    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(workspace.workspace_name())
        .context("workspace should have a working-copy commit")?;
    let wc_commit = repo.store().get_commit(wc_commit_id)?;

    // Snapshot to get the actual filesystem state
    let mut locked_wc = workspace.working_copy().start_mutation()?;
    let snapshot_options = SnapshotOptions {
        base_ignores: jj_lib::gitignore::GitIgnoreFile::empty(),
        progress: None,
        start_tracking_matcher: &jj_lib::matchers::EverythingMatcher,
        max_new_file_size: 1024 * 1024 * 100,
    };
    let (current_tree, _stats) = locked_wc.snapshot(&snapshot_options).await?;

    // Check if the working copy commit has changes (compared to its parent)
    let parent_tree = if !wc_commit.parent_ids().is_empty() {
        let parent_commit = repo.store().get_commit(&wc_commit.parent_ids()[0])?;
        parent_commit.tree()
    } else {
        jj_lib::merged_tree::MergedTree::resolved(repo.store().clone(), repo.store().empty_tree_id().clone())
    };

    // If working copy tree matches parent tree, there's nothing to commit
    if current_tree.tree_ids() == parent_tree.tree_ids() {
        println!("No changes to commit (working copy matches parent)");
        drop(locked_wc);
        return Ok(());
    }

    // If working copy commit already has a description, don't overwrite it
    if !wc_commit.description().is_empty() {
        println!("Working copy commit already has a description, skipping");
        drop(locked_wc);
        return Ok(());
    }

    // Generate diff for commit message (using jj-lib API, not external command)
    println!("Getting diff...");
    let diff = get_tree_diff(&repo, &parent_tree, &current_tree).await?;

    if diff.trim().is_empty() {
        println!("No changes to commit");
        drop(locked_wc);
        return Ok(());
    }

    // Drop the lock before calling Claude (external process)
    drop(locked_wc);

    println!("Generating commit message using Claude...");
    let generator = CommitMessageGenerator::new(&args.language);
    let commit_message = generator.generate(&diff);

    println!("Generated message: {}", commit_message);

    // Create commit with the snapshotted tree
    println!("Creating commit...");
    create_commit(&workspace, &commit_message, current_tree).await?;

    Ok(())
}
