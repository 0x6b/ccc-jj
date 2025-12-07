use std::{
    io::Write,
    process::{Command, Stdio},
    sync::LazyLock,
};

use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use serde::Deserialize;
use toml::from_str;
use tracing::{debug, trace, warn};

#[derive(Deserialize)]
struct Config {
    prompt: Prompt,
    generator: Generator,
    diff: DiffConfig,
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

#[derive(Deserialize)]
struct DiffConfig {
    collapse_patterns: Vec<String>,
    max_diff_lines: usize,
    max_diff_bytes: usize,
}

/// Get the collapse patterns from config
pub fn collapse_patterns() -> &'static [String] {
    &CONFIG.diff.collapse_patterns
}

/// Get the max diff lines threshold from config
pub fn max_diff_lines() -> usize {
    CONFIG.diff.max_diff_lines
}

/// Get the max diff bytes threshold from config
pub fn max_diff_bytes() -> usize {
    CONFIG.diff.max_diff_bytes
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
    /// `Some(message)` if generation succeeds, `None` if it fails.
    /// If the generated message doesn't follow conventional commit format, the default
    /// commit message prefix is prepended.
    pub fn generate(&self, diff_content: &str) -> Option<String> {
        debug!(diff_len = diff_content.len(), "Starting commit message generation");
        self.try_generate(diff_content).map(|message| {
            let first_line = message.lines().next().unwrap_or("").trim();
            if CONVENTIONAL_COMMIT_RE.is_match(first_line) {
                debug!("Generated message follows conventional commit format");
                message
            } else {
                warn!(first_line = %first_line, "Generated message does not follow conventional commit format, prepending default");
                format!("{}\n\n{message}", CONFIG.generator.default_commit_message)
            }
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
            prompt_len = prompt.len(),
            "Executing Claude CLI via stdin"
        );

        // Use stdin to pass prompt (avoids "Argument list too long" for large diffs)
        let result = Command::new(&self.command)
            .args(&self.args)
            .arg("--model")
            .arg(&self.model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                // Write prompt to stdin
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(prompt.as_bytes())?;
                }
                child.wait_with_output()
            });

        let result = match result {
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
