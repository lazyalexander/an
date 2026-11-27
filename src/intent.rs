//! Controller-owned, turn-scoped graph flow authorizations.
mod followup;

use crate::{permission, store::*};
use anyhow::{ensure, Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

fn route(op: &str, origin: &str, source: &Value, destination: &str, revision: &Value) -> Value {
    let mut row = json!({"operation":op,"origin":origin,"source":source,"destination":destination});
    if !revision.is_null() {
        row["revision"] = revision.clone();
    }
    row
}

/// Only the host's run record can attest to the command that produced a binding.
pub fn graph_route(
    db: &Connection,
    op: &str,
    source: &Value,
    destination: &str,
    binding: &Value,
) -> Result<Value> {
    let origin = crate::graph::binding_origin(binding);
    let mut actual = route(op, &origin, source, destination, &binding["revision"]);
    if let Some(id) = binding["run"].as_str() {
        let run = get(db, "run", id)?;
        if run["editable"] == false && run["spool"] == origin {
            actual["producer_argv"] = run["argv"].clone();
        }
    }
    Ok(actual)
}

pub fn show(db: &Connection, task: &str) -> Result<Value> {
    let stamp = permission::stamp(db, task)?;
    Ok(find(db, "intent", task)?
        .filter(|c| c["stamp"] == stamp)
        .unwrap_or(Value::Null))
}

pub fn propose(db: &Connection, task: &str, spec: Value) -> Result<Value> {
    let existing = show(db, task)?;
    let stamp = permission::stamp(db, task)?;
    let task_info = get(db, "task", task)?;
    let model = task_info["model_grant"]["alias"]
        .as_str()
        .map(|s| format!("model:{s}"));
    let image = |s: &str| task_info["work"].get(s).is_some();
    let workspace = task_info["workspace"]
        .as_str()
        .map(|id| get(db, "workspace", id))
        .transpose()?;
    let permitted = workspace
        .as_ref()
        .and_then(|w| w["policy"]["flows"].as_array());
    // An object opts into a closed scope. Legacy lists retain their fallback policy.
    let scoped = spec.is_object();
    let rows = if scoped {
        ensure!(
            spec.as_object()
                .unwrap()
                .keys()
                .all(|k| matches!(k.as_str(), "turn_scope" | "instruction_cursor")),
            "Unknown scope field"
        );
        spec["turn_scope"]
            .as_array()
            .context("Expected turn_scope list")?
    } else {
        spec.as_array().context("Expected intent route list")?
    };
    ensure!(
        !rows.is_empty() && rows.len() <= 128,
        "Expected 1 to 128 routes"
    );
    let mut routes = Vec::new();
    for row in rows {
        ensure!(
            row.as_object().is_some_and(|m| m.keys().all(|k| matches!(
                k.as_str(),
                "operation" | "origin" | "source" | "destination" | "producer_argv"
            ))),
            "Unknown intent route field"
        );
        let op = row["operation"].as_str().context("Missing operation")?;
        let origin = row["origin"].as_str().context("Missing origin")?;
        let dest = row["destination"].as_str().context("Missing destination")?;
        let source = &row["source"];
        let fields = source.as_object().context("Expected a graph source")?;
        let dynamic = scoped && source.get("producer_port").is_some();
        let version = if let Some(name) = source["input"].as_str() {
            ensure!(
                fields.len() == 1 && origin == format!("task-input:{name}"),
                "Wrong input origin"
            );
            task_info["inputs"]
                .get(name)
                .context("Input outside task")?
        } else if dynamic {
            ensure!(
                fields.len() == 1
                    && source["producer_port"]
                        .as_str()
                        .is_some_and(|p| !p.is_empty())
                    && image(origin)
                    && row["producer_argv"].is_array(),
                "Expected immutable producer command and output port"
            );
            &Value::Null
        } else {
            ensure!(
                fields.len() == 2
                    && ["node", "port"]
                        .iter()
                        .all(|k| source[k].as_str().is_some_and(|s| !s.is_empty())),
                "Expected node and output port"
            );
            ensure!(
                image(origin)
                    || Some(origin) == model.as_deref()
                    || (origin == "response" && source["port"] == "response"),
                "Origin outside task"
            );
            &Value::Null
        };
        ensure!(
            match op {
                "input" => image(dest) && (source["input"].is_string() || origin == "response"),
                "handoff" => image(dest) && (image(origin) || Some(origin) == model.as_deref()),
                "preview" => dest == "studio",
                "model" => Some(dest) == model.as_deref(),
                "publish" =>
                    dest == task_info["workspace"].as_str().unwrap_or(task)
                        || (dest == "graph-metadata"
                            && image(origin)
                            && (source["port"] == "subgraph.json"
                                || source["producer_port"] == "subgraph.json")),
                _ => false,
            },
            "Destination or operation outside task"
        );
        if let Some(flows) = permitted {
            let flow_origin = if op == "input" {
                source["input"].as_str().unwrap_or(origin)
            } else {
                origin
            };
            ensure!(
                flows.contains(&json!([op, flow_origin, dest])),
                "Intent outside workspace flow policy"
            );
        }
        let mut r = route(op, origin, source, dest, version);
        if let Some(argv) = row.get("producer_argv") {
            ensure!(
                (dynamic || source["node"].is_string())
                    && image(origin)
                    && argv.as_array().is_some_and(|a| a
                        .first()
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty())
                        && a.iter().all(Value::is_string)),
                "Expected an image output and a nonempty producer command"
            );
            r["producer_argv"] = argv.clone();
        }
        ensure!(
            !routes.iter().any(|earlier| followup::overlaps(earlier, &r)),
            "Duplicate or overlapping intent route"
        );
        routes.push(r);
    }
    let (cursor, previous) = followup::prepare(db, task, &stamp, &existing, &spec, &routes)?;
    let mut card = json!({"id":uid(),"task":task,"stamp":stamp,"model":task_info["model_grant"],"routes":routes,"status":"pending","turn_scope":scoped,"instruction_cursor":cursor});
    if !previous.is_empty() {
        card["previous"] = Value::Array(previous);
    }
    put(db, "intent", task, &card)?;
    Ok(card)
}

pub fn decide(db: &Connection, task: &str, id: &str, decisions: Value) -> Result<Value> {
    let mut card = show(db, task)?;
    ensure!(card["id"] == id, "Stale intent");
    let values = decisions.as_array().context("Expected decisions list")?;
    ensure!(
        Some(values.len()) == card["routes"].as_array().map(Vec::len)
            && values
                .iter()
                .all(|v| matches!(v.as_str(), Some("allow" | "deny" | "ask"))),
        "Expected one allow, deny or ask per route"
    );
    if card["status"] == "decided" {
        ensure!(card["decisions"] == decisions, "Intent already decided");
        return Ok(card);
    }
    card["decisions"] = decisions;
    card["status"] = json!("decided");
    put(db, "intent", task, &card)?;
    let mut query=db.prepare("SELECT body FROM objects WHERE kind='request' AND json_extract(body,'$.task')=? AND json_extract(body,'$.state')='pending' ORDER BY rowid")?;
    let pending = query
        .query_map([task], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for body in pending {
        let mut request: Value = serde_json::from_str(&body)?;
        if request["revisions"] != card["stamp"] {
            continue;
        }
        let Some(index) = route_index(&card, &request["route"]) else {
            continue;
        };
        let state = match card["decisions"][index].as_str() {
            Some("deny") => "deny",
            Some("allow")
                if matches!(
                    approved_output(db, &card, index, &request["route"])?,
                    Match::Allow(_)
                ) =>
            {
                "intent_approved"
            }
            _ => continue,
        };
        request["state"] = json!(state);
        put(
            db,
            "request",
            request["id"].as_str().context("Invalid request ID")?,
            &request,
        )?;
    }
    let mut record = card.clone();
    record.as_object_mut().unwrap().remove("previous");
    put(db, "intent_history", id, &record)?;
    if card["turn_scope"] == true {
        crate::graph::intent_decided(db, task, &card)?;
    }
    Ok(card)
}

pub fn route_index(card: &Value, actual: &Value) -> Option<usize> {
    let source = &actual["source"];
    // Resolve references by the same fields as the graph scheduler, including
    // for graphs and permission requests persisted by older releases.
    let source = if let Some(input) = source["input"].as_str() {
        json!({"input":input})
    } else {
        json!({"node":source["node"].as_str()?,"port":source["port"].as_str()?})
    };
    card["routes"].as_array()?.iter().position(|r| {
        (r["source"] == source
            || (card["turn_scope"] == true
                && source["node"].is_string()
                && r["source"]["producer_port"].is_string()
                && r["source"]["producer_port"] == source["port"]
                && r["producer_argv"].is_array()
                && r["producer_argv"] == actual["producer_argv"]))
            && ["operation", "origin", "destination"]
                .iter()
                .all(|k| r[k] == actual[k])
            && (!source["input"].is_string() || actual["revision"] == r["revision"])
    })
}

#[derive(Debug, PartialEq)]
pub enum Match {
    Standard,
    Pending,
    Ask,
    Allow(Value),
}

/// Match the current card before consulting discretionary grants.
pub fn authorize(db: &Connection, task: &str, stamp: &Value, actual: &Value) -> Result<Match> {
    let Some(card) = find(db, "intent", task)?.filter(|c| c["stamp"] == *stamp) else {
        return Ok(Match::Standard);
    };
    let cards = std::iter::once(&card).chain(card["previous"].as_array().into_iter().flatten());
    for reviewed in cards {
        let Some(index) = route_index(reviewed, actual) else {
            continue;
        };
        if actual["operation"] == "model"
            && reviewed["model"] != get(db, "task", task)?["model_grant"]
        {
            return Ok(Match::Ask);
        }
        if reviewed["status"] == "pending" {
            return Ok(Match::Pending);
        }
        return match reviewed["decisions"][index].as_str() {
            Some("deny") => anyhow::bail!("Denied by turn intent"),
            Some("allow") => approved_output(db, reviewed, index, actual),
            Some("ask") => Ok(Match::Ask),
            _ => anyhow::bail!("Invalid intent decision"),
        };
    }
    if card["turn_scope"] == true
        || (actual["operation"] == "model"
            && card["model"] != get(db, "task", task)?["model_grant"])
    {
        Ok(Match::Ask)
    } else {
        Ok(Match::Standard)
    }
}

fn approved_output(db: &Connection, card: &Value, index: usize, actual: &Value) -> Result<Match> {
    let rule = &card["routes"][index];
    let origin = rule["origin"].as_str().context("Missing producer origin")?;
    if rule["source"]["node"].is_string()
        && origin != "response"
        && card["model"]["alias"]
            .as_str()
            .is_none_or(|alias| origin != format!("model:{alias}"))
        && (!rule["producer_argv"].is_array() || rule["producer_argv"] != actual["producer_argv"])
    {
        return Ok(Match::Ask);
    }
    let revision = actual["revision"]
        .as_str()
        .context("Missing graph artifact version")?;
    if card["turn_scope"] == true && rule["source"]["producer_port"].is_string() {
        return Ok(Match::Allow(card["id"].clone()));
    }
    let key = format!(
        "{}:{index}",
        card["id"].as_str().context("Invalid intent ID")?
    );
    match find(db, "intent_version", &key)? {
        Some(bound) if bound != revision => return Ok(Match::Ask),
        None => put(db, "intent_version", &key, &json!(revision))?,
        _ => (),
    }
    Ok(Match::Allow(card["id"].clone()))
}

/// A resumed agent needs the reviewed routes to propose just the missing flows.
pub fn scope_summary(db: &Connection, task: &str) -> Result<Vec<Value>> {
    let active = show(db, task)?;
    if active["turn_scope"] != true {
        return Ok(Vec::new());
    }
    Ok(active["previous"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(std::iter::once(&active))
        .map(|c| {
            json!({"id":c["id"],"instruction_cursor":c["instruction_cursor"],
            "status":c["status"],"routes":c["routes"],"decisions":c["decisions"]})
        })
        .collect())
}

pub fn public(mut card: Value) -> Value {
    if let Some(row) = card.as_object_mut() {
        row.remove("stamp");
        row.remove("previous");
    }
    card
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_decision_covers_expansion_and_replanned_model_inputs() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")
            .unwrap();
        let mut task = json!({"work":{"driver":"work"},"model_grant":{"model":"local","alias":"paint","budget":2}});
        put(&db, "task", "t", &task).unwrap();
        put(&db, "turn", "t", &json!("active")).unwrap();
        // Seed the reviewed card so this test does not mutate process-global AN_TURN.
        let stamp = json!(["turn", "workspace", "host"]);
        let routes = [
            ("publish", "subgraph.json", "graph-metadata"),
            ("model", "drawing", "model:paint"),
        ];
        let rules:Vec<_> = routes.iter().map(|(op,port,dest)| json!({"operation":op,"origin":"driver","destination":dest,"source":{"producer_port":port},"producer_argv":["render"]})).collect();
        put(&db,"intent","t",&json!({"id":"review","stamp":stamp,"model":task["model_grant"],"turn_scope":true,"routes":rules,"status":"decided","decisions":["allow","allow"]})).unwrap();
        let check = |index: usize, node: &str| {
            let (op, port, dest) = routes[index];
            authorize(&db,"t",&stamp,&json!({"operation":op,"origin":"driver","destination":dest,"source":{"node":node,"port":port},"producer_argv":["render"],"revision":node})).unwrap()
        };
        for (index, node) in [(0, "expand"), (1, "first"), (1, "replanned")] {
            assert_eq!(check(index, node), Match::Allow(json!("review")));
        }
        task["model_grant"]["model"] = json!({"backend":"openai"});
        put(&db, "task", "t", &task).unwrap();
        assert_eq!(check(1, "external"), Match::Ask);
    }

    #[test]
    fn turn_scope_follows_the_command_and_port_across_nodes_and_versions() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")
            .unwrap();
        let card = json!({"id":"scope","stamp":["turn"],"status":"decided","turn_scope":true,
            "decisions":["allow"], "routes":[{"operation":"handoff","origin":"converter",
            "source":{"producer_port":"output"},"destination":"viewer","producer_argv":["convert"]}]});
        put(&db, "intent", "task", &card).unwrap();
        let check = |r: &Value| authorize(&db, "task", &card["stamp"], r).unwrap();
        for (node, revision) in [("first", "v1"), ("replanned", "v2")] {
            let mut r = json!({"operation":"handoff","origin":"converter","destination":"viewer",
                "source":{"node":node,"port":"output"},"revision":revision,"producer_argv":["convert"]});
            assert_eq!(check(&r), Match::Allow(json!("scope")));
            r["destination"] = json!("external");
            assert_eq!(check(&r), Match::Ask);
            r["destination"] = json!("viewer");
            r["producer_argv"] = Value::Null;
            assert_eq!(check(&r), Match::Ask);
        }
        assert_eq!(check(&Value::Null), Match::Ask);
    }

    #[test]
    fn future_output_is_pinned_and_matches_only_its_declared_route() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")
            .unwrap();
        let source = json!({"node":"organize","port":"summary"});
        let make = |origin: &str, source: &Value, dest: &str, rev: Value| {
            let mut r = route("handoff", origin, source, dest, &rev);
            r["producer_argv"] = json!(["organize", "--public"]);
            r
        };
        let scope = make("organizer", &source, "formatter", Value::Null);
        let card = json!({"task":"t","id":"c","status":"decided","decisions":["allow"],"stamp":["turn","workspace","host"],"routes":[scope]});
        put(&db, "intent", "t", &card).unwrap();
        let check = |actual: Value| authorize(&db, "t", &card["stamp"], &actual).unwrap();
        let observed = |id: &str, argv: Value, editable: bool| {
            put(
                &db,
                "run",
                id,
                &json!({"argv":argv,"spool":"organizer","editable":editable}),
            )
            .unwrap();
            graph_route(
                &db,
                "handoff",
                &source,
                "formatter",
                &json!({"revision":"first","spool":"organizer","run":id}),
            )
            .unwrap()
        };
        assert_eq!(
            check(observed("different", json!(["other"]), false)),
            Match::Ask
        );
        assert_eq!(
            check(observed("editable", scope["producer_argv"].clone(), true)),
            Match::Ask
        );
        let approved = observed("expected", scope["producer_argv"].clone(), false);
        assert_eq!(check(approved.clone()), Match::Allow(json!("c")));
        assert_eq!(check(approved), Match::Allow(json!("c")));
        assert_eq!(
            check(make("organizer", &source, "formatter", json!("later"))),
            Match::Ask
        );
        for actual in [
            make("organizer", &source, "other", json!("first")),
            make("other", &source, "formatter", json!("first")),
            make(
                "organizer",
                &json!({"node":"organize","port":"internal"}),
                "formatter",
                json!("first"),
            ),
        ] {
            assert_eq!(check(actual), Match::Standard);
        }
        let mut legacy = card.clone();
        legacy["routes"][0]
            .as_object_mut()
            .unwrap()
            .remove("producer_argv");
        put(&db, "intent", "t", &legacy).unwrap();
        assert_eq!(
            check(make("organizer", &source, "formatter", json!("first"))),
            Match::Ask
        );
        legacy["decisions"][0] = json!("deny");
        put(&db, "intent", "t", &legacy).unwrap();
        let decorated = json!({"node":"organize","port":"summary","title":"display"});
        let actual = make("organizer", &decorated, "formatter", json!("later"));
        assert_eq!(
            authorize(&db, "t", &card["stamp"], &actual)
                .unwrap_err()
                .to_string(),
            "Denied by turn intent"
        );
    }
}
