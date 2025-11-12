use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
use jj_lib::object_id::ObjectId;
use jj_lib::repo::{Repo, StoreFactories};
use jj_lib::settings::UserSettings;
use jj_lib::transaction::Transaction;
use jj_lib::workspace::{default_working_copy_factories, Workspace};
use jj_lib::working_copy::SnapshotOptions;

#[derive(Parser, Debug)]
#[command(author, version, about = "Auto-commit changes in a jj workspace using Claude for commit messages", long_about = None)]
struct Args {
    /// Path to the workspace (defaults to current directory)
    #[arg(short, long)]
    path: Option<PathBuf>,

    /// Path to the claude CLI executable
    #[arg(short, long, default_value = "claude")]
    claude_path: String,
}

/// Create a base configuration layer with operation hostname and username
fn env_base_layer() -> ConfigLayer {
    let mut layer = ConfigLayer::empty(ConfigSource::EnvBase);

    // Set operation.hostname
    if let Ok(hostname) = whoami::fallible::hostname() {
        layer.set_value("operation.hostname", hostname).unwrap();
    }

    // Set operation.username
    if let Ok(username) = whoami::fallible::username() {
        layer.set_value("operation.username", username).unwrap();
    } else if let Ok(username) = env::var("USER") {
        layer.set_value("operation.username", username).unwrap();
    }

    layer
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
    // Build config with proper layers
    let mut config = StackedConfig::with_defaults();
    config.add_layer(env_base_layer());

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

/// Get the diff of current working copy changes using jj diff
fn get_diff(workspace_root: &Path) -> Result<String> {
    let output = Command::new("jj")
        .current_dir(workspace_root)
        .args(["diff"])
        .output()
        .context("Failed to execute jj diff")?;

    if !output.status.success() {
        anyhow::bail!(
            "jj diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8(output.stdout)?)
}

/// Generate commit message using Claude CLI
fn generate_commit_message(claude_path: &str, diff: &str) -> Result<String> {
    let prompt = format!(
        "You are a commit message generator. Based on the following diff, generate a concise, \
         clear commit message following conventional commits format (type: description). \
         The message should be a single line that summarizes the changes.\n\n\
         Diff:\n{}\n\n\
         Respond with ONLY the commit message, no explanation or additional text.",
        diff
    );

    let child = Command::new(claude_path)
        .args(["-p", &prompt])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn claude process")?;

    let output = child.wait_with_output()?;

    if !output.status.success() {
        anyhow::bail!(
            "claude CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let message = String::from_utf8(output.stdout)?
        .trim()
        .to_string();

    Ok(message)
}

/// Create a commit with the generated message
async fn create_commit(
    workspace: &Workspace,
    commit_message: &str,
) -> Result<()> {
    // Load repo at HEAD
    let repo: Arc<jj_lib::repo::ReadonlyRepo> = workspace.repo_loader().load_at_head()?;

    // Snapshot the working copy to get a Tree id
    let mut locked_wc = workspace.working_copy().start_mutation()?;
    let snapshot_options = SnapshotOptions {
        base_ignores: jj_lib::gitignore::GitIgnoreFile::empty(),
        progress: None,
        start_tracking_matcher: &jj_lib::matchers::EverythingMatcher,
        max_new_file_size: 1024 * 1024 * 100, // 100 MB
    };
    let (tree, _stats) = locked_wc.snapshot(&snapshot_options).await?;

    // Start a transaction and prepare the new commit
    let mut tx: Transaction = repo.start_transaction();
    let mut_repo = tx.repo_mut();

    // Get the working-copy commit and use its parents
    let wc_commit_id = repo
        .view()
        .get_wc_commit_id(workspace.workspace_name())
        .context("workspace should have a working-copy commit")?;
    let wc_commit = repo.store().get_commit(wc_commit_id)?;

    // Build & write the commit
    let new_commit = mut_repo
        .new_commit(wc_commit.parent_ids().to_vec(), tree)
        .set_description(commit_message)
        .write()?;

    // Update the working copy to point at the new commit
    mut_repo.set_wc_commit(workspace.workspace_name().to_owned(), new_commit.id().clone())?;

    // Publish the repo changes and get the new operation
    let new_repo = tx.commit("auto-commit via ccc-jj")?;

    // Finish the working-copy lock with the operation ID
    locked_wc.finish(new_repo.operation().id().clone()).await?;

    println!("Committed change {} with message:", new_commit.id().hex());
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
    let workspace_root = workspace.workspace_root().to_path_buf();

    // Get diff
    println!("Getting diff...");
    let diff = get_diff(&workspace_root)?;

    if diff.trim().is_empty() {
        println!("No changes to commit");
        return Ok(());
    }

    println!("Generating commit message using Claude...");
    let commit_message = generate_commit_message(&args.claude_path, &diff)?;

    println!("Generated message: {}", commit_message);

    // Create commit
    println!("Creating commit...");
    create_commit(&workspace, &commit_message).await?;

    Ok(())
}
