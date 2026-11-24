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
        ensure!(
            value.as_u64().is_some_and(|n| n > 0),
            "Setting {key} must be a positive integer"
        );
    }
    Ok(Config(settings))
}
