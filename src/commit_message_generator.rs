use std::process::Command;
use std::sync::LazyLock;

use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};
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
    agent: Agent,
}

#[derive(Deserialize)]
struct Agent {
    name: String,
    description: String,
    prompt: String,
    tools: String,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    from_str(include_str!("../assets/commit-config.toml"))
        .expect("Failed to parse embedded commit-config.toml")
});

static AGENTS_JSON: LazyLock<String> = LazyLock::new(|| {
    let mut agents = Map::new();
    agents.insert(CONFIG.generator.agent.name.clone(), CONFIG.generator.agent.to_json());
    Value::Object(agents).to_string()
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
    agents_json: &'static str,
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
            agents_json: &AGENTS_JSON,
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

        let mut command = Command::new(&self.command);
        command.args(&self.args);
        command.arg("--model");
        command.arg(&self.model);
        command.arg("--agents");
        command.arg(self.agents_json);
        command.arg(&prompt);

        let result = command
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|message| !message.is_empty());

        spinner.finish_and_clear();
        result
    }
}

impl Agent {
    fn to_json(&self) -> Value {
        let mut fields = Map::new();
        fields.insert("description".to_string(), Value::String(self.description.clone()));
        fields.insert("prompt".to_string(), Value::String(self.prompt.clone()));
        fields.insert("tools".to_string(), Value::String(self.tools.clone()));
        Value::Object(fields)
    }
}

impl Default for CommitMessageGenerator {
    fn default() -> Self {
        Self::new("English", "haiku")
    }
}
