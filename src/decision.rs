use super::{display, graph, Task};
use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};
use std::io::{self, Write};

#[derive(Subcommand)]
pub enum Action {
    List {
        #[command(flatten)]
        target: Task,
    },
    Answer {
        #[command(flatten)]
        target: Task,
        #[arg(long)]
        node: String,
        /// Require the attempt previously displayed by list.
        #[arg(long)]
        request: Option<String>,
        /// Read one JSON response object from standard input.
        #[arg(long,value_parser=["-"])]
        json: Option<String>,
    },
}

fn prompt(schema: &Value, decision: &Value, draft: &Value) -> Result<Value> {
    let mut result = json!({});
    for (key, rule) in schema["properties"]
        .as_object()
        .context("Missing response fields")?
    {
        let default = draft.get(key).or_else(|| {
            if decision["field"] == key.as_str() {
                decision.get("default")
            } else {
                None
            }
        });
        loop {
            eprint!("{key} ({})", rule["type"]);
            if let Some(options) = rule.get("enum") {
                eprint!(" {options}");
            }
            if let Some(min) = rule.get("minimum") {
                eprint!(" [{min}, {}]", rule["maximum"]);
            }
            if let Some(v) = default {
                eprint!(" default {v}");
            }
            eprint!(" > ");
            io::stderr().flush()?;
            let mut line = String::new();
            ensure!(
                io::stdin().read_line(&mut line)? > 0,
                "Input ended before answer"
            );
            let text = line.trim_end_matches(['\r', '\n']);
            let value = if text.is_empty() && default.is_some() {
                default.cloned().unwrap()
            } else if rule["type"] == "string" {
                json!(text)
            } else {
                match serde_json::from_str(text) {
                    Ok(v) => v,
                    Err(_) => {
                        eprintln!("Expected {}", rule["type"]);
                        continue;
                    }
                }
            };
            match graph::response_valid(rule, &value) {
                Ok(()) => {
                    result[key] = value;
                    break;
                }
                Err(e) => eprintln!("{e}"),
            }
        }
    }
    Ok(result)
}

pub fn execute(action: Action) -> Result<()> {
    let target = match &action {
        Action::List { target } | Action::Answer { target, .. } => &target.task,
    };
    let state = graph::host(target, "state", &json!({}))?;
    let mut waiting = Vec::new();
    for (id, node) in state["nodes"].as_object().context("Invalid graph")? {
        let activity = &state["activity"][id];
        if activity["status"] == "waiting" && node["package"]["declaration"].is_object() {
            let d = &node["package"]["declaration"];
            if node["kind"] == "native" {
                waiting.push(json!({"node":id,"request":activity["id"],"title":d["title"],"adapter":d["adapter"],"phase":activity["native_phase"],"error":activity["native_error"]}));
                continue;
            }
            waiting.push(json!({"node":id,"request":activity["id"],"title":d["title"],"decision":d["decision"],"response":node["response"],"bindings":activity["bindings"]}));
        }
    }
    if let Action::Answer {
        node,
        request,
        json: source,
        ..
    } = &action
    {
        let row = waiting
            .iter()
            .find(|r| r["node"] == node.as_str())
            .context("No waiting declared decision")?;
        ensure!(
            request
                .as_ref()
                .is_none_or(|r| row["request"] == r.as_str()),
            "Stale decision"
        );
        ensure!(
            row.get("adapter").is_none(),
            "Complete and deliver the document in Krita"
        );
        let mut args = json!({"node":node,"request":row["request"]});
        let decision = graph::host(target, "decision", &args)?;
        eprintln!("{}", serde_json::to_string_pretty(row)?);
        graph::host(target, "viewed", &args)?;
        let value = if source.is_some() {
            serde_json::from_reader(io::stdin().lock())?
        } else {
            prompt(&row["response"], &row["decision"], &decision["draft"])?
        };
        args["value"] = value;
        display(graph::host(target, "respond", &args)?);
    } else {
        display(json!(waiting));
    }
    Ok(())
}
