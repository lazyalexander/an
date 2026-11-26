//! Operational settings. Defaults live in `config/an.json` and are compiled in;
//! a host may override any of them in `$AN_HOME/config.json`. Format contracts
//! that admission depends on (manifest size, path budgets, digest shape) stay in
//! code so an image is admitted identically on every host.
use crate::store::root;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{Map, Value};
use std::{fs, io::ErrorKind, time::Duration};

const DEFAULTS: &str = include_str!("../config/an.json");

pub struct Config(Map<String, Value>);

impl Config {
    pub fn aliases(&self) -> &Map<String, Value> {
        self.0["model_aliases"].as_object().unwrap()
    }
    fn number(&self, key: &str) -> u64 {
        self.0[key].as_u64().expect("validated setting")
    }
    pub fn seconds(&self, key: &str) -> Duration {
        Duration::from_secs(self.number(key))
    }
    pub fn limit(&self, key: &str) -> u64 {
        self.number(key)
    }
}

pub fn load() -> Result<Config> {
    let mut settings: Map<String, Value> =
        serde_json::from_str(DEFAULTS).context("Invalid built-in config")?;
    match fs::read(root()?.join("config.json")) {
        Ok(bytes) => {
            let overrides: Map<String, Value> =
                serde_json::from_slice(&bytes).context("Invalid config.json")?;
            for (key, value) in overrides {
                if !settings.contains_key(&key) {
                    bail!("Unknown setting {key} in config.json");
                }
                settings.insert(key, value);
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    for (key, value) in &settings {
        if key == "model_aliases" {
            ensure!(
                value
                    .as_object()
                    .is_some_and(|m| m.iter().all(|(k, v)| !k.is_empty() && alias_valid(v))),
                "Invalid model aliases"
            );
            continue;
        }
        if key == "ollama_port" || key == "image_port" {
            ensure!(
                value.as_u64().is_some_and(|p| p <= 65535),
                "Invalid Ollama port"
            );
        }
        ensure!(
            value.as_u64().is_some_and(|n| n > 0),
            "Setting {key} must be a positive integer"
        );
    }
    Ok(Config(settings))
}

/// A text alias names a local Ollama model. An image alias is an object with
/// `kind: "image"`, a `backend` (`local` loopback service or `openai`), a
/// `model`, and for `openai` the Keychain service holding the API key.
fn alias_valid(value: &Value) -> bool {
    if let Some(name) = value.as_str() {
        return !name.is_empty();
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    object
        .keys()
        .all(|k| matches!(k.as_str(), "kind" | "backend" | "model" | "keychain"))
        && object.get("kind").and_then(Value::as_str) == Some("image")
        && text("model")
        && match object.get("backend").and_then(Value::as_str) {
            Some("local") => !object.contains_key("keychain"),
            Some("openai") => text("keychain"),
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_aliases_name_a_backend_and_keep_keys_in_the_keychain() {
        assert!(alias_valid(&serde_json::json!("qwen3:8b")));
        assert!(alias_valid(
            &serde_json::json!({"kind":"image","backend":"local","model":"m"})
        ));
        assert!(alias_valid(&serde_json::json!(
            {"kind":"image","backend":"openai","model":"m","keychain":"an.openai"})));
        for bad in [
            serde_json::json!({"kind":"image","backend":"openai","model":"m"}),
            serde_json::json!({"kind":"image","backend":"openai","model":"m","keychain":"k","api_key":"sk"}),
            serde_json::json!({"kind":"image","backend":"local","model":"m","keychain":"k"}),
            serde_json::json!({"kind":"video","backend":"local","model":"m"}),
            serde_json::json!(""),
        ] {
            assert!(!alias_valid(&bad), "{bad}");
        }
    }
}
