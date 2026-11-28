//! The same source identity is used for graph edges and permission routes.
use super::field;
use anyhow::{ensure, Result};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Serialize)]
#[serde(untagged)]
pub(crate) enum ArtifactRef<'a> {
    Input { input: &'a str },
    Output { node: &'a str, port: &'a str },
}

impl<'a> TryFrom<&'a Value> for ArtifactRef<'a> {
    type Error = anyhow::Error;

    fn try_from(reference: &'a Value) -> Result<Self> {
        if let Some(input) = reference["input"].as_str() {
            ensure!(!input.is_empty(), "Expected input");
            return Ok(Self::Input { input });
        }
        Ok(Self::Output {
            node: field(reference, "node")?,
            port: field(reference, "port")?,
        })
    }
}

impl<'a> ArtifactRef<'a> {
    /// Graph inputs must name a single task input. Historical producer references
    /// can carry extra fields; only the node and port identify the output.
    pub(super) fn declared(reference: &'a Value) -> Result<Self> {
        if reference.get("input").is_some() {
            field(reference, "input")?;
            ensure!(
                reference.as_object().is_some_and(|m| m.len() == 1),
                "Ambiguous input reference"
            );
        }
        Self::try_from(reference)
    }
}
