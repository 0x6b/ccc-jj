use std::sync::LazyLock;

use serde::Deserialize;
use toml::from_str;

#[derive(Deserialize)]
pub struct Config {
    pub prompt: PromptConfig,
    pub generator: GeneratorConfig,
    pub bookmark: BookmarkConfig,
    pub diff: DiffConfig,
}

#[derive(Deserialize)]
pub struct PromptConfig {
    pub template: String,
}

#[derive(Deserialize)]
pub struct GeneratorConfig {
    pub command: String,
    pub args: Vec<String>,
    pub default_commit_message: String,
}

#[derive(Deserialize)]
pub struct BookmarkConfig {
    pub prompt_template: String,
}

#[derive(Deserialize)]
pub struct DiffConfig {
    pub collapse_patterns: Vec<String>,
    pub max_diff_lines: usize,
    pub max_diff_bytes: usize,
    pub max_total_diff_lines: usize,
    pub max_total_diff_bytes: usize,
}

pub static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    from_str(include_str!("../assets/commit-config.toml"))
        .expect("Failed to parse embedded commit-config.toml")
});
