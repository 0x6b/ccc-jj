use std::{
    io::Write,
    process::{Command, Stdio},
    sync::LazyLock,
};

use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use toml::from_str;
use tracing::{debug, trace, warn};

#[derive(Deserialize)]
struct Config {
    bookmark: BookmarkConfig,
    generator: Generator,
}

#[derive(Deserialize)]
struct BookmarkConfig {
    prompt_template: String,
}

#[derive(Deserialize)]
struct Generator {
    command: String,
    args: Vec<String>,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    from_str(include_str!("../assets/commit-config.toml"))
        .expect("Failed to parse embedded commit-config.toml")
});

static VALID_BOOKMARK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z][a-z0-9]*(-[a-z][a-z0-9]*){1,5}$").expect("Failed to compile bookmark regex")
});

const JSON_SCHEMA: &str = r#"{"type":"object","properties":{"bookmark":{"type":"string","description":"Bookmark name: 2-6 lowercase words separated by hyphens, e.g. 'add-user-auth'"}},"required":["bookmark"]}"#;

pub struct BookmarkGenerator {
    prompt_template: String,
    command: String,
    args: Vec<String>,
    model: String,
}

impl BookmarkGenerator {
    pub fn new(model: &str) -> Self {
        Self {
            prompt_template: CONFIG.bookmark.prompt_template.clone(),
            command: CONFIG.generator.command.clone(),
            args: CONFIG.generator.args.clone(),
            model: model.to_string(),
        }
    }

    pub fn generate(&self, commit_summaries: &str) -> Option<String> {
        debug!(summaries_len = commit_summaries.len(), "Starting bookmark name generation");
        self.try_generate(commit_summaries).and_then(|name| {
            let name = name.trim().to_lowercase();
            if VALID_BOOKMARK_RE.is_match(&name) {
                debug!(bookmark = %name, "Generated valid bookmark name");
                Some(name)
            } else {
                warn!(bookmark = %name, "Generated bookmark name doesn't match expected format");
                None
            }
        })
    }

    fn try_generate(&self, commit_summaries: &str) -> Option<String> {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("✶✸✹✺✹✷")
                .template("{spinner:.yellow} {msg}")
                .ok()?,
        );
        spinner.set_message("Generating bookmark name with Claude...");
        spinner.enable_steady_tick(std::time::Duration::from_millis(200));

        let prompt = self.prompt_template.replace("{commit_summaries}", commit_summaries);
        trace!(prompt_len = prompt.len(), "Prepared prompt for Claude");

        debug!(
            command = %self.command,
            args = ?self.args,
            model = %self.model,
            prompt_len = prompt.len(),
            "Executing Claude CLI via stdin"
        );

        let result = Command::new(&self.command)
            .args(&self.args)
            .arg("--model")
            .arg(&self.model)
            .arg("--json-schema")
            .arg(JSON_SCHEMA)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
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
                    let raw_output = String::from_utf8_lossy(&output.stdout);
                    trace!(raw_output = %raw_output, "Claude CLI raw output");

                    match serde_json::from_str::<Value>(&raw_output) {
                        Ok(json) => {
                            let structured = if let Some(arr) = json.as_array() {
                                arr.iter()
                                    .rfind(|obj| {
                                        obj.get("type").and_then(|v| v.as_str()) == Some("result")
                                    })
                                    .and_then(|obj| obj.get("structured_output"))
                            } else {
                                json.get("structured_output")
                            };

                            if let Some(structured) = structured {
                                let bookmark = structured
                                    .get("bookmark")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .trim();

                                if bookmark.is_empty() {
                                    warn!("Claude CLI returned empty bookmark");
                                    None
                                } else {
                                    trace!(bookmark = %bookmark, "Claude CLI output");
                                    Some(bookmark.to_string())
                                }
                            } else {
                                warn!("Claude CLI JSON missing 'structured_output' field");
                                None
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, raw = %raw_output, "Failed to parse Claude CLI JSON output");
                            None
                        }
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
