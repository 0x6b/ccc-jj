use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use toml::from_str;

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
}

impl CommitMessageGenerator {
    /// Creates a new commit message generator
    pub fn new() -> Self {
        Self {
            prompt_template: CONFIG.prompt.template.clone(),
            command: CONFIG.generator.command.clone(),
            args: CONFIG.generator.args.clone(),
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
        self.try_generate(diff_content)
            .map(|message| {
                if CONVENTIONAL_COMMIT_RE.is_match(message.lines().next().unwrap_or("").trim()) {
                    message
                } else {
                    format!("{}\n\n{message}", CONFIG.generator.default_commit_message)
                }
            })
            .unwrap_or_else(|| CONFIG.generator.default_commit_message.to_string())
    }

    fn try_generate(&self, diff_content: &str) -> Option<String> {
        let prompt = self.prompt_template.replace("{diff_content}", diff_content);

        eprintln!("=== Full prompt being sent to Claude ===");
        eprintln!("{}", prompt);
        eprintln!("=== End of prompt ===\n");

        Command::new(&self.command)
            .args(&self.args)
            .arg(&prompt)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|message| !message.is_empty())
    }
}

impl Default for CommitMessageGenerator {
    fn default() -> Self {
        Self::new()
    }
}
