use std::sync::LazyLock;

use regex::Regex;
use tracing::{debug, error, trace, warn};

use crate::{
    claude_client::{ClaudeRequest, invoke_claude},
    config::CONFIG,
    text_formatter::format_text,
};

static CONVENTIONAL_COMMIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z]+(?:\([^)]+\))?(?:!)?:\s.+")
        .expect("Failed to compile conventional commit regex")
});

const JSON_SCHEMA: &str = r#"{"type":"object","properties":{"title":{"type":"string","description":"Commit title in conventional commit format, max 50 chars"},"body":{"type":"string","description":"Optional commit body explaining what and why"}},"required":["title"]}"#;

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
            let message = if CONVENTIONAL_COMMIT_RE.is_match(first_line) {
                debug!("Generated message follows conventional commit format");
                message
            } else {
                error!(first_line = %first_line, "Generated message does not follow conventional commit format, prepending default");
                format!("{}\n\n{message}", CONFIG.generator.default_commit_message)
            };
            format_text(&message, 72)
        })
    }

    fn try_generate(&self, diff_content: &str) -> Option<String> {
        let prompt = self
            .prompt_template
            .replace("{language}", &self.language)
            .replace("{diff_content}", diff_content);
        trace!(prompt_len = prompt.len(), "Prepared prompt for Claude");

        let request = ClaudeRequest {
            command: &self.command,
            args: &self.args,
            model: &self.model,
            json_schema: JSON_SCHEMA,
            prompt: &prompt,
            spinner_message: "Generating commit message with Claude...",
        };

        let structured = invoke_claude(&request)?;

        let title = structured.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
        let body = structured.get("body").and_then(|v| v.as_str()).unwrap_or("").trim();

        if title.is_empty() {
            warn!("Claude CLI returned empty title");
            return None;
        }

        let message =
            if body.is_empty() { title.to_string() } else { format!("{title}\n\n{body}") };
        trace!(message = %message, "Claude CLI output");
        Some(message)
    }
}

impl Default for CommitMessageGenerator {
    fn default() -> Self {
        Self::new("English", "haiku")
    }
}
