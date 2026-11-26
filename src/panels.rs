//! Versioned, admitted presentation contracts.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_alphabetic()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn names(list: &Value) -> Result<BTreeSet<&str>> {
    let mut names = BTreeSet::new();
    for value in list.as_array().context("Expected name list")? {
        let s = value.as_str().context("Expected name")?;
        ensure!(
            identifier(s) && names.insert(s),
            "Invalid or duplicate name {s}"
        );
    }
    Ok(names)
}

pub fn validate(panels: &Value) -> Result<()> {
    for (id, p) in panels.as_object().context("panels: expected map")? {
        validate_panel(id, p).with_context(|| format!("panels.{id}"))?;
    }
    Ok(())
}

fn validate_panel(id: &str, p: &Value) -> Result<()> {
    ensure!(identifier(id), "Invalid panel ID");
    ensure!(
        p.as_object()
            .context("Expected panel")?
            .keys()
            .all(|k| matches!(
                k.as_str(),
                "kind" | "title" | "bindings" | "optional" | "html" | "decision" | "adapter"
            )),
        "Unknown panel field"
    );
    ensure!(
        p["title"].as_str().is_some_and(|s| !s.trim().is_empty()),
        "Invalid title"
    );
    let required = names(&p["bindings"]).context("bindings")?;
    if let Some(optional) = p.get("optional") {
        ensure!(
            names(optional)?.is_disjoint(&required),
            "Overlapping bindings"
        );
    }
    if let Some(html) = p.get("html") {
        let path = html.as_str().context("Invalid html path")?;
        ensure!(
            path.starts_with("spool/code/panels/")
                && path.ends_with(".html")
                && !path.contains('\\')
                && crate::artifacts::safe(path).is_ok(),
            "Invalid html path"
        );
    }
    if p["kind"] != "native" {
        ensure!(
            p.get("adapter").is_none(),
            "Adapter requires a native panel"
        );
    }
    match p["kind"].as_str() {
        Some("native") => ensure!(
            crate::adapters::named(p["adapter"].as_str()).is_some()
                && p["bindings"] == json!(["document"])
                && ["html", "optional", "decision"]
                    .iter()
                    .all(|key| p.get(key).is_none()),
            "Expected an installed native adapter with one document binding"
        ),
        Some("surface") => {
            schema(p)?;
        }
        Some("delivery") => ensure!(p.get("decision").is_none(), "Unexpected delivery decision"),
        _ => anyhow::bail!("Invalid panel kind"),
    }
    Ok(())
}

/// JSON clients may encode the same integral option with or without a fraction.
pub(crate) fn option_equal(left: &Value, right: &Value) -> bool {
    let mixed = if left.is_f64() && right.is_number() && !right.is_f64() {
        Some((left, right))
    } else if right.is_f64() && left.is_number() && !left.is_f64() {
        Some((right, left))
    } else {
        None
    };
    if let Some((float, integer)) = mixed {
        let n = float.as_f64().unwrap();
        let exact = integer
            .as_i64()
            .map(i128::from)
            .or_else(|| integer.as_u64().map(i128::from));
        return n.fract() == 0.0 && Some(n as i128) == exact;
    }
    left == right
}

pub fn schema(panel: &Value) -> Result<Value> {
    let d = &panel["decision"];
    let mut props = json!({});
    let coordinate = json!({"type":"number","minimum":0,"maximum":1});
    let mut keys = vec!["type", "extra"];
    match d["type"].as_str().context("Missing decision.type")? {
        kind @ ("choose" | "compare") => {
            let bindings = names(&panel["bindings"])?;
            ensure!(
                bindings.len() >= 2,
                "{kind} requires two or more artifact bindings"
            );
            props["choice"] = json!({"type":"string","enum":bindings});
        }
        "point" | "region" => {
            ensure!(
                !names(&panel["bindings"])?.is_empty(),
                "Location needs an artifact"
            );
            for k in if d["type"] == "point" {
                &["x", "y"][..]
            } else {
                &["x", "y", "width", "height"][..]
            } {
                props[k] = coordinate.clone();
            }
        }
        "confirm" => props["confirmed"] = json!({"type":"boolean"}),
        "sketch" => {
            // A pressure drawing, optionally over one reference artifact.
            // Points are normalized [x, y, pressure] so any screen size maps
            // onto the same canvas, and downstream spools need no viewport.
            ensure!(
                names(&panel["bindings"])?.len() <= 1,
                "sketch takes at most one reference artifact"
            );
            let stroke = json!({"type":"object","additionalProperties":false,
                "required":["color","size","points"],
                "properties":{
                    "color":{"type":"string"},
                    "size":{"type":"number","minimum":0.5,"maximum":200},
                    "points":{"type":"array","items":{"type":"array","items":coordinate}}}});
            props["strokes"] = json!({"type":"array","items":stroke});
        }
        "adjust" => {
            keys.extend(["field", "value_type", "options", "default"]);
            let field = d["field"]
                .as_str()
                .filter(|s| identifier(s))
                .context("Invalid decision.field")?;
            let ty = d["value_type"].as_str().context("Missing value_type")?;
            ensure!(
                matches!(ty, "integer" | "number" | "string"),
                "Invalid value_type"
            );
            let mut rule = json!({"type":ty});
            if let Some(options) = d.get("options") {
                let options = options.as_array().context("Invalid options")?;
                ensure!(!options.is_empty(), "Empty options");
                for (i, v) in options.iter().enumerate() {
                    crate::graph::response_valid(&rule, v)?;
                    ensure!(
                        !options[..i].iter().any(|prior| option_equal(prior, v)),
                        "Duplicate option"
                    );
                }
                rule["enum"] = json!(options);
            }
            if let Some(default) = d.get("default") {
                crate::graph::response_valid(&rule, default)?;
            }
            props[field] = rule;
        }
        _ => anyhow::bail!("Unknown decision type"),
    }
    ensure!(
        d.as_object()
            .unwrap()
            .keys()
            .all(|k| keys.contains(&k.as_str())),
        "Unknown decision field"
    );
    if let Some(extra) = d.get("extra") {
        for (k, ty) in extra.as_object().context("Invalid decision.extra")? {
            ensure!(
                identifier(k) && props.get(k).is_none(),
                "Invalid extra field {k}"
            );
            props[k] = match ty.as_str() {
                Some("text") => json!({"type":"string"}),
                Some("coordinate") => coordinate.clone(),
                _ => anyhow::bail!("Invalid extra field type"),
            };
        }
    }
    Ok(
        json!({"type":"object","properties":props,"required":props.as_object().unwrap().keys().collect::<Vec<_>>() }),
    )
}

pub fn check_bindings(panel: &Value, inputs: &Value) -> Result<()> {
    let required = names(&panel["bindings"])?;
    let mut allowed = required.clone();
    if let Some(optional) = panel.get("optional") {
        allowed.extend(names(optional)?);
    }
    let actual: BTreeSet<_> = inputs
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k.as_str())
        .collect();
    ensure!(
        required.is_subset(&actual) && actual.is_subset(&allowed),
        "Bindings differ from panel declaration"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sketch_declares_normalized_pressure_strokes() {
        let panel = json!({"kind":"surface","title":"Draw","bindings":["reference"],
            "decision":{"type":"sketch","extra":{"note":"text"}}});
        let rule = schema(&panel).unwrap();
        let good = json!({"note":"roof", "strokes":[{"color":"#1d3557","size":12,
            "points":[[0.1,0.2,0.3],[0.15,0.25,0.9]]}]});
        crate::graph::response_valid(&rule, &good).unwrap();
        for bad in [
            json!({"note":"x","strokes":[{"color":"#000","size":12,"points":[[1.5,0.2,0.3]]}]}),
            json!({"note":"x","strokes":[{"color":"#000","size":12,"points":[[0.1,0.2,0.3]],"t":1}]}),
            json!({"note":"x","strokes":[{"color":"#000","points":[]}]}),
            json!({"note":"x"}),
        ] {
            assert!(crate::graph::response_valid(&rule, &bad).is_err(), "{bad}");
        }
        let mut two = panel.clone();
        two["bindings"] = json!(["a", "b"]);
        assert!(schema(&two).is_err());
    }
    #[test]
    fn numeric_options_round_trip_through_browser_json() {
        let mut panel = json!({"kind":"surface","title":"Adjust","bindings":[],"decision":{"type":"adjust","value_type":"number","field":"append","options":[1.0],"default":1}});
        let rule = schema(&panel).unwrap();
        assert!(crate::graph::response_valid(&rule, &json!({"append":1})).is_ok());
        assert!(crate::graph::response_valid(&rule, &json!({"append":"1"})).is_err());
        panel["decision"]["options"] = json!([1, 1.0]);
        assert!(schema(&panel).is_err());
        assert!(!option_equal(
            &json!(9007199254740993_u64),
            &json!(9007199254740992.0)
        ));
    }

    #[test]
    fn invalid_declarations_and_responses_are_rejected() {
        let base = json!({"title":"Place","kind":"surface","bindings":["image"],"decision":{"type":"point"}});
        for (key, value) in [
            ("decision", Value::Null),
            ("decision", json!({"type":"unknown"})),
            ("bindings", json!(["image", "image"])),
            ("optional", json!(["image"])),
            ("html", json!("spool/code/panels/../bad.html")),
            (
                "decision",
                json!({"type":"adjust","field":"s","value_type":"integer","options":[1,2],"default":3}),
            ),
            ("decision", json!({"type":"point","extra":{"x":"text"}})),
        ] {
            let mut bad = base.clone();
            bad["html"] = json!("spool/code/panels/valid.html");
            bad[key] = value;
            assert!(validate(&json!({"place":bad})).is_err());
        }
        let rule = schema(&base).unwrap();
        assert!(crate::graph::response_valid(&rule, &json!({"x":-0.1,"y":1})).is_err());
        assert!(crate::graph::response_valid(&rule, &json!({"x":0.5,"y":1})).is_ok());
    }
}
