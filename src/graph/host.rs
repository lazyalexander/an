use super::{control, deliveries, dispatch, sources, surfaces, view, wake};
use crate::{
    instances,
    store::{connect, find, get},
};
use anyhow::Result;
use rusqlite::TransactionBehavior;
use serde_json::{json, Value};

pub fn host(task: &str, action: &str, args: &Value) -> Result<Value> {
    match action {
        "submit" => return dispatch(task, "graph_submit", args),
        "state" => return dispatch(task, "graph_state", args),
        "say" => return dispatch(task, "graph_say", args),
        _ => {}
    }
    let mut db = connect()?;
    let lock = instances::lock(&db, task)?;
    if matches!(action, "control" | "view" | "finish") {
        super::model::reconcile_sends(&db, task)?;
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let t = get(&tx, "task", task)?;
    if matches!(action, "open" | "resume") {
        return deliveries::open_or_resume(tx, task, &t, action, args);
    }
    let mut graph = find(&tx, "graph", task)?
        .unwrap_or(json!({"nodes":{},"revision":0,"events":[],"activity":{}}));
    match action {
        "view" => view::task(&tx, task, &t, graph),
        "deliveries" => deliveries::list(&tx, task, &t),
        "requests" => view::requests(&tx, task, &graph),
        "sources" | "evidence" => sources::read(tx, task, &t, graph, action, args),
        "instruct" => {
            let result = control::instruct(&tx, task, &t, &mut graph, args)?;
            tx.commit()?;
            Ok(result)
        }
        "control" => {
            let change = control::apply(&tx, task, &t, &mut graph, args)?;
            tx.commit()?;
            // Cancelling observes and stops containers; it never relaunches work.
            for run in change.stop.runs {
                instances::dispatch_locked(&mut db, task, "cancel", &json!({"run":run}), &lock)?;
            }
            for instance in change.stop.drivers {
                instances::dispatch_locked(
                    &mut db,
                    task,
                    "unload",
                    &json!({"instance":instance}),
                    &lock,
                )?;
            }
            drop(lock);
            wake(task)?;
            Ok(change.receipt)
        }
        "finish" => {
            control::finish(&tx, task, t, &mut graph)?;
            tx.commit()?;
            instances::finish_unloads(&mut db, task, &lock)
        }
        "panel" | "decision" | "viewed" | "respond" | "draft" => {
            surfaces::handle(tx, lock, task, graph, action, args)
        }
        _ => anyhow::bail!("Unknown graph host action"),
    }
}
