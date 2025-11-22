use std::process::Command;
use std::sync::LazyLock;

use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use serde::Deserialize;
use toml::from_str;
use tracing::{debug, trace, warn};

#[derive(Deserialize)]
struct Config {
    prompt: Prompt,
    generator: Generator,
}

#[derive(Deserialize)]
struct Prompt {
    template: String,
}

#[derive(Deserialize)]
struct Generator {
    command: String,
    args: Vec<String>,
    default_commit_message: String,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    from_str(include_str!("../assets/commit-config.toml"))
        .expect("Failed to parse embedded commit-config.toml")
});

static CONVENTIONAL_COMMIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z]+(?:\([^)]+\))?(?:!)?:\s.+")
        .expect("Failed to compile conventional commit regex")
});

/// Generates commit messages using Claude CLI based on diff content
pub struct CommitMessageGenerator {
    prompt_template: String,
    command: String,
    args: Vec<String>,
    language: String,
    model: String,
}

impl CommitMessageGenerator {
    /// Creates a new commit message generator
    ///
    /// # Arguments
    /// - `language` - The language to use for generating commit messages
    /// - `model` - The Claude model to use for generation
    pub fn new(language: &str, model: &str) -> Self {
        Self {
            prompt_template: CONFIG.prompt.template.clone(),
            command: CONFIG.generator.command.clone(),
            args: CONFIG.generator.args.clone(),
            language: language.to_string(),
            model: model.to_string(),
        }
    }

    /// Generates a commit message from the provided diff content
    ///
    /// # Arguments
    /// - `diff_content` - The diff content to analyze for message generation
    ///
    /// # Returns
    /// A generated commit message string. If generation fails or the result doesn't follow a
    /// conventional commit format, returns a default commit message.
    pub fn generate(&self, diff_content: &str) -> String {
        debug!(diff_len = diff_content.len(), "Starting commit message generation");
        self.try_generate(diff_content)
            .map(|message| {
                let first_line = message.lines().next().unwrap_or("").trim();
                if CONVENTIONAL_COMMIT_RE.is_match(first_line) {
                    debug!("Generated message follows conventional commit format");
                    message
                } else {
                    warn!(first_line = %first_line, "Generated message does not follow conventional commit format, prepending default");
                    format!("{}\n\n{message}", CONFIG.generator.default_commit_message)
                }
            })
            .unwrap_or_else(|| {
                warn!("Failed to generate commit message, using default");
                CONFIG.generator.default_commit_message.to_string()
            })
    }

    fn try_generate(&self, diff_content: &str) -> Option<String> {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("✶✸✹✺✹✷")
                .template("{spinner:.yellow} {msg}")
                .ok()?,
        );
        spinner.set_message("Generating commit message with Claude...");
        spinner.enable_steady_tick(std::time::Duration::from_millis(200));

        let prompt = self
            .prompt_template
            .replace("{language}", &self.language)
            .replace("{diff_content}", diff_content);
        trace!(prompt_len = prompt.len(), "Prepared prompt for Claude");

        debug!(
            command = %self.command,
            args = ?self.args,
            model = %self.model,
            "Executing Claude CLI"
        );

        let mut command = Command::new(&self.command);
        command.args(&self.args);
        command.arg("--model");
        command.arg(&self.model);
        command.arg(&prompt);

        let result = match command.output() {
            Ok(output) => {
                debug!(
                    status = %output.status,
                    stdout_len = output.stdout.len(),
                    stderr_len = output.stderr.len(),
                    "Claude CLI completed"
                );
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    warn!(status = %output.status, stderr = %stderr, "Claude CLI failed");
                    None
                } else {
                    let message = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if message.is_empty() {
                        warn!("Claude CLI returned empty output");
                        None
                    } else {
                        trace!(message = %message, "Claude CLI output");
                        Some(message)
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to execute Claude CLI");
                None
            }
        };

        spinner.finish_and_clear();
        result
    }
}

impl Default for CommitMessageGenerator {
    fn default() -> Self {
        Self::new("English", "haiku")
    }
}
