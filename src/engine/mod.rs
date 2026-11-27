pub(crate) mod apple;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs::File;
pub(crate) fn observe(i: &mut Value, f: &File) -> String {
    match apple::list(f) {
        Err(e) => {
            i["error"] = json!(e.to_string());
            "unknown".into()
        }
        Ok(list) => match list.iter().find(|v| v["id"] == i["name"]) {
            None => "absent".into(),
            Some(hit) => {
                let cfg = &hit["configuration"];
                if cfg["labels"]["an.instance"] != i["id"]
                    || cfg["image"]["descriptor"]["digest"].as_str()
                        != i["spool"]
                            .as_str()
                            .and_then(|s| s.rsplit_once('@').map(|v| v.1))
                {
                    "identity_mismatch".into()
                } else {
                    hit["status"]["state"].as_str().unwrap_or("unknown").into()
                }
            }
        },
    }
}
pub(crate) fn settle(i: &mut Value, f: &File, mut actual: String) -> Result<String> {
    let unloading = i["intent"] == "unload";
    let stopping = i["intent"] == "stop";
    if unloading || stopping {
        if actual == "running" {
            if let Err(e) = apple::stop(i["name"].as_str().context("Missing instance name")?, f) {
                i["error"] = json!(e.to_string());
            }
            actual = observe(i, f);
        }
        if unloading && matches!(actual.as_str(), "stopped" | "created" | "exited") {
            if let Err(e) = apple::delete(i["name"].as_str().context("Missing instance name")?, f) {
                i["error"] = json!(e.to_string());
            }
            actual = observe(i, f);
        }
        i["status"] = json!(if actual == "absent" {
            "unloaded"
        } else if stopping && matches!(actual.as_str(), "stopped" | "created" | "exited") {
            "stopped"
        } else {
            "cleanup_pending"
        });
    } else {
        i["status"] = json!(match actual.as_str() {
            "running" => "running",
            "absent" | "stopped" | "created" | "exited" => "stopped",
            _ => "unknown",
        });
    }
    Ok(actual)
}
