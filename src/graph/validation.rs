use super::*;

pub(super) fn validate(nodes: &serde_json::Map<String, Value>) -> Result<()> {
    // A document surface has two milestones. Only its designated save run can
    // consume the response before the saved document has been captured.
    let mut dependencies = BTreeMap::new();
    for (id, node) in nodes {
        let mut deps = BTreeSet::new();
        if let Some(after) = node.get("after") {
            for predecessor in after.as_array().context("Expected predecessor list")? {
                let key = predecessor.as_str().context("Invalid predecessor")?;
                ensure!(nodes.contains_key(key), "Unknown dependency {key}");
                deps.insert((key, false));
            }
        }
        for reference in node["inputs"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, r)| r)
        {
            let ArtifactRef::Output {
                node: upstream,
                port,
            } = ArtifactRef::declared(reference)?
            else {
                continue;
            };
            let producer = nodes.get(upstream).context("Unknown source node")?;
            ensure!(
                producer["outputs"]
                    .as_array()
                    .is_some_and(|a| a.contains(&json!(port))),
                "Unknown output {upstream}/{port}"
            );
            deps.insert((
                upstream,
                (producer["save"]["node"] == id.as_str()
                    || producer["continuation"] == id.as_str())
                    && port == "response",
            ));
        }
        if let Some(target) = node.get("continuation") {
            let driver = nodes
                .get(target.as_str().context("Invalid continuation")?)
                .context("Missing continuation")?;
            ensure!(
                node["kind"] == "surface"
                    && node.get("save").is_some()
                    && driver["kind"] == "expand"
                    && driver["inputs"].as_object().is_some_and(|inputs| inputs
                        .values()
                        .any(|r| r["node"] == id.as_str() && r["port"] == "response")),
                "Invalid continuation"
            );
        }
        if let Some(save) = node.get("save") {
            let saver = field(save, "node")?;
            let port = field(save, "port")?;
            ensure!(
                node["kind"] == "surface" && saver != id,
                "Invalid save binding"
            );
            dependencies.insert((id.as_str(), true), deps);
            let mut completion = BTreeSet::from([(id.as_str(), true)]);
            if let Some(driver) = node["continuation"].as_str() {
                completion.insert((driver, false));
            }
            if let Some(run) = nodes.get(saver) {
                ensure!(
                    run["kind"] == "run"
                        && run["outputs"]
                            .as_array()
                            .is_some_and(|a| a.contains(&json!(port)))
                        && run["inputs"]
                            .as_object()
                            .into_iter()
                            .flatten()
                            .any(|(_, r)| r["node"] == id.as_str() && r["port"] == "response"),
                    "Invalid save run"
                );
                completion.insert((saver, false));
            }
            dependencies.insert((id.as_str(), false), completion);
        } else {
            dependencies.insert((id.as_str(), false), deps);
        }
    }
    while !dependencies.is_empty() {
        let ready: BTreeSet<_> = dependencies
            .iter()
            .filter(|(_, d)| d.is_empty())
            .map(|(k, _)| *k)
            .collect();
        ensure!(!ready.is_empty(), "Dependency cycle");
        dependencies.retain(|id, deps| {
            deps.retain(|d| !ready.contains(d));
            !ready.contains(id)
        });
    }
    Ok(())
}

pub(super) fn validate_schema(schema: &Value) -> Result<()> {
    ensure!(
        schema["type"] == "object",
        "Expected response object schema"
    );
    validate_rule(schema)
}

fn validate_rule(rule: &Value) -> Result<()> {
    let kind = field(rule, "type")?;
    let keywords: &[&str] = match kind {
        "object" => &["properties", "required", "additionalProperties"],
        "array" => &["items"],
        "string" | "integer" | "number" | "boolean" => &[],
        _ => anyhow::bail!("Unsupported response type {kind}"),
    };
    for key in rule.as_object().context("Expected schema object")?.keys() {
        ensure!(
            matches!(key.as_str(), "type" | "description" | "title" | "enum")
                || keywords.contains(&key.as_str()),
            "Unsupported response constraint {key}"
        );
    }
    if let Some(values) = rule.get("enum") {
        ensure!(
            values.as_array().is_some_and(|values| !values.is_empty()),
            "Invalid response enum"
        )
    }
    if kind == "array" {
        validate_rule(&rule["items"])?;
    }
    if kind == "object" {
        let props = rule["properties"]
            .as_object()
            .context("Expected response properties")?;
        if let Some(extra) = rule.get("additionalProperties") {
            ensure!(extra == false, "Response objects use declared fields")
        }
        if let Some(required) = rule.get("required") {
            for key in required
                .as_array()
                .context("Expected required field names")?
            {
                ensure!(
                    props.contains_key(key.as_str().context("Expected field name")?),
                    "Unknown required field"
                );
            }
        }
        for property in props.values() {
            validate_rule(property)?;
        }
    }
    Ok(())
}

pub(crate) fn response_valid(rule: &Value, value: &Value) -> Result<()> {
    if let Some(options) = rule["enum"].as_array() {
        ensure!(
            options
                .iter()
                .any(|option| crate::panels::option_equal(option, value)),
            "Invalid response choice"
        );
    }
    if let Some(n) = value.as_f64() {
        ensure!(
            rule["minimum"].as_f64().is_none_or(|min| n >= min)
                && rule["maximum"].as_f64().is_none_or(|max| n <= max),
            "Number outside allowed range"
        );
    }
    match field(rule, "type")? {
        "object" => {
            let fields = value.as_object().context("Expected response object")?;
            for key in rule["required"].as_array().into_iter().flatten() {
                ensure!(fields.contains_key(key.as_str().unwrap()), "Missing {key}");
            }
            for (key, v) in fields {
                let property = rule["properties"]
                    .get(key)
                    .with_context(|| format!("Unknown response field {key}"))?;
                response_valid(property, v)
                    .with_context(|| format!("Invalid response field {key}"))?;
            }
        }
        "array" => {
            for item in value.as_array().context("Expected response array")? {
                response_valid(&rule["items"], item)?;
            }
        }
        kind => ensure!(
            match kind {
                "string" => value.is_string(),
                "integer" => value.as_f64().is_some_and(|n| n.fract() == 0.0),
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                _ => false,
            },
            "Expected {kind}"
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_drawing_response_is_checked_before_acceptance() {
        let mut schema = json!({"type":"object","required":["strokes"],"properties":{"strokes":{"type":"array","items":{"type":"array","items":{"type":"number"}}}}});
        validate_schema(&schema).unwrap();
        response_valid(&schema, &json!({"strokes":[[1,2],[3,4]]})).unwrap();
        assert!(response_valid(&schema, &json!({"strokes":[[1,"2"]]})).is_err());
        schema["properties"]["strokes"]["maxItems"] = json!(2);
        assert!(validate_schema(&schema).is_err());
    }
}
