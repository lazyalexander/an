use anyhow::{anyhow, bail, ensure, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use yaml_rust2::{
    parser::{Event, Parser},
    scanner::TScalarStyle,
};
pub const LIMIT: u64 = 65536;
pub fn parse(raw: &[u8]) -> Result<Value> {
    ensure!(raw.len() <= LIMIT as usize, "spool.yaml exceeds 64 KiB");
    let text = std::str::from_utf8(raw)?;
    let mut parser = Parser::new_from_str(text);
    ensure!(
        parser.next_token()?.0 == Event::StreamStart,
        "Expected YAML stream"
    );
    ensure!(
        parser.next_token()?.0 == Event::DocumentStart,
        "Expected YAML document"
    );
    let first = parser.next_token()?.0;
    let value = node(&mut parser, first, 0, &mut 0)?;
    ensure!(
        parser.next_token()?.0 == Event::DocumentEnd && parser.next_token()?.0 == Event::StreamEnd,
        "Expected one YAML document"
    );
    let mut keys = vec!["schema", "name", "summary", "inputs", "outputs"];
    match value["schema"].as_str() {
        Some("an.spool/v1") => (),
        Some("an.spool/v2") => {
            keys.push("panels");
            crate::panels::validate(&value["panels"])?;
        }
        _ => bail!("Unsupported manifest schema"),
    }
    fields(&value, &keys, "manifest")?;
    name(&value["name"], "name")?;
    description(&value["summary"], "summary")?;
    for direction in ["inputs", "outputs"] {
        let mut seen = HashSet::new();
        for (i, p) in value[direction]
            .as_array()
            .ok_or_else(|| anyhow!("{direction}: expected list"))?
            .iter()
            .enumerate()
        {
            let path = format!("{direction}[{i}]");
            let mut keys = vec!["name", "description", "media_types"];
            if direction == "inputs" {
                keys.push("required");
                ensure!(
                    p["required"].is_boolean(),
                    "{path}.required: expected boolean"
                );
            }
            fields(p, &keys, &path)?;
            name(&p["name"], &format!("{path}.name"))?;
            ensure!(seen.insert(p["name"].as_str()), "{path}.name: duplicate");
            description(&p["description"], &format!("{path}.description"))?;
            let mut types = HashSet::new();
            let media = p["media_types"]
                .as_array()
                .ok_or_else(|| anyhow!("{path}.media_types: expected list"))?;
            ensure!(!media.is_empty(), "{path}.media_types: expected types");
            for m in media {
                let m = m.as_str().unwrap_or("");
                ensure!(
                    m.split_once('/').is_some_and(|(a, b)| token(a) && token(b))
                        && types.insert(m.to_ascii_lowercase()),
                    "{path}.media_types: invalid or duplicate media type"
                );
            }
        }
    }
    Ok(value)
}
fn node<I: Iterator<Item = char>>(
    parser: &mut Parser<I>,
    event: Event,
    depth: usize,
    count: &mut usize,
) -> Result<Value> {
    *count += 1;
    ensure!(*count <= 4096, "Manifest exceeds structure budget");
    match event {
        Event::Scalar(value, style, 0, None) => Ok(if style == TScalarStyle::Plain {
            plain(&value)
        } else {
            json!(value)
        }),
        Event::SequenceStart(0, None) => {
            ensure!(depth < 16, "Manifest exceeds structure budget");
            let mut items = vec![];
            loop {
                let event = parser.next_token()?.0;
                if event == Event::SequenceEnd {
                    break;
                }
                items.push(node(parser, event, depth + 1, count)?);
            }
            Ok(Value::Array(items))
        }
        Event::MappingStart(0, None) => {
            ensure!(depth < 16, "Manifest exceeds structure budget");
            let mut map = serde_json::Map::new();
            loop {
                let (key, mark) = parser.next_token()?;
                if key == Event::MappingEnd {
                    break;
                }
                let key = node(parser, key, depth + 1, count)?;
                let key = key
                    .as_str()
                    .ok_or_else(|| anyhow!("Expected string key at line {}", mark.line()))?;
                ensure!(
                    key != "<<" && !map.contains_key(key),
                    "Unsupported or duplicate key {key} at line {}",
                    mark.line()
                );
                let value = parser.next_token()?.0;
                map.insert(key.into(), node(parser, value, depth + 1, count)?);
            }
            Ok(Value::Object(map))
        }
        _ => bail!("Unexpected YAML node, anchor, alias or explicit tag"),
    }
}
// Resolve numeric decision options while preserving v1 string and boolean checks.
fn plain(value: &str) -> Value {
    static NON_STRING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
        r"(?x)^(?:[-+]?(?:0b[01_]+|0[0-7_]+|0|[1-9][0-9_]*|0x[[:xdigit:]_]+|[1-9][0-9_]*(?::[0-5]?[0-9])+)
        |[-+]?[0-9][0-9_]*\.[0-9_]*(?:[eE][-+][0-9]+)?|\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?
        |[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*|[-+]?\.(?:inf|Inf|INF)|\.(?:nan|NaN|NAN)
        |[0-9]{4}-[0-9]{2}-[0-9]{2}|[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}(?:[Tt]|[\x20\t]+)[0-9]{1,2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]*)?(?:[\x20\t]*(?:Z|[-+][0-9]{1,2}(?::[0-9]{2})?))?)$").unwrap()
    });
    if let Ok(n) = serde_json::from_str::<serde_json::Number>(value) {
        return Value::Number(n);
    }
    match value {
        "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" => json!(true),
        "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" => json!(false),
        "" | "null" | "Null" | "NULL" | "~" | "=" | "<<" => Value::Null,
        _ if NON_STRING.is_match(value) => Value::Null,
        _ => json!(value),
    }
}
fn fields(v: &Value, fields: &[&str], path: &str) -> Result<()> {
    ensure!(
        v.as_object()
            .is_some_and(|m| m.len() == fields.len() && fields.iter().all(|k| m.contains_key(*k))),
        "{path}: expected {}",
        fields.join(", ")
    );
    Ok(())
}
fn token(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 127
        && t.as_bytes()[0].is_ascii_alphanumeric()
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
}
fn name(v: &Value, path: &str) -> Result<()> {
    let s = v.as_str().unwrap_or("");
    ensure!(
        !s.is_empty()
            && s.len() <= 64
            && s.as_bytes()[0].is_ascii_lowercase()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
        "{path}: invalid name"
    );
    Ok(())
}
fn description(v: &Value, path: &str) -> Result<()> {
    ensure!(
        v.as_str().is_some_and(|s| !s.trim().is_empty()),
        "{path}: expected nonempty text"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_source_types_and_port_constraints() {
        let yaml="schema: an.spool/v1\nname: editor\nsummary: 工具\ninputs:\n  - name: file\n    required: yes\n    description: Content\n    media_types: [application/ld+json]\noutputs: []\n";
        assert_eq!(
            parse(yaml.as_bytes()).unwrap()["inputs"][0]["required"],
            true
        );
        for invalid in [
            yaml.replace("yes", "'true'"),
            yaml.replace("application/ld+json", "text/*"),
            yaml.replace("application/ld+json", "text/plain, TEXT/plain"),
            yaml.replace("Content", "2026-11-18"),
        ] {
            assert!(parse(invalid.as_bytes()).is_err());
        }
    }
    #[test]
    fn rejects_excessive_nesting_before_constructing_nodes() {
        let nested = format!("{}x{}", "[".repeat(1024), "]".repeat(1024));
        assert!(parse(nested.as_bytes()).is_err());
        assert!(parse("- x\n".repeat(4097).as_bytes()).is_err());
    }
}
