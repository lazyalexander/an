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
            for (key, mut value) in overrides {
                // Older hosts may share this registry with the visual experiment.
                if matches!(
                    key.as_str(),
                    "image_port" | "image_timeout_s" | "image_output_bytes"
                ) {
                    continue;
                }
                if key == "model_aliases" {
                    if let Some(aliases) = value.as_object_mut() {
                        aliases.retain(|_, model| model["kind"] != "image");
                    }
                }
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
                value.as_object().is_some_and(|m| m
                    .iter()
                    .all(|(k, v)| !k.is_empty() && v.as_str().is_some_and(|s| !s.is_empty()))),
                "Invalid model aliases"
            );
            continue;
        }
        if key == "ollama_port" {
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
