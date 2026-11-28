//! The transaction boundary shared by the CLI and the task MCP gateway.
mod access;
mod domain;
mod history;
pub(crate) use history::require_accepted;
mod material;
mod versions;
use crate::{
    instances,
    store::{canonical, connect, hash},
};
pub(crate) use access::scoped;
use access::Actor;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};

pub fn authenticate(db: &Connection, task: &str) -> Result<()> {
    Actor::authenticate(db, task).map(|_| ())
}

pub fn schema(db: &Connection) -> Result<()> {
    Ok(db.execute_batch(include_str!("schema.sql"))?)
}

pub fn execute(task: &str, action: &str, args: &Value) -> Result<Value> {
    let mut db = connect()?;
    let _lock = instances::lock(&db, task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actor = Actor::authenticate(&tx, task)?;
    let result = match action {
        "grant" => access::grant(&tx, &actor, args)?,
        "revoke" => access::revoke(&tx, &actor, args)?,
        "admit-material" => material::admit(&tx, &actor, args)?,
        "history" => history::view(
            &tx,
            &actor,
            args["cursor"].as_u64().context("Expected cursor")?,
        )?,
        "decide" => {
            let decision: domain::Decision = serde_json::from_value(args.clone())?;
            let key = decision.idempotency_key.clone();
            replay(&tx, &actor, &key, action, args, || {
                history::decide(&tx, &actor, decision)
            })?
        }
        "commit" => {
            let batch: domain::Batch = serde_json::from_value(args.clone())?;
            let key = batch.idempotency_key.clone();
            replay(&tx, &actor, &key, action, args, || {
                let receipt = versions::commit(&tx, &actor, batch)?;
                history::proposed(&tx, task, &receipt)?;
                Ok(receipt)
            })?
        }
        _ => anyhow::bail!("Unknown provenance operation"),
    };
    tx.commit()?;
    Ok(result)
}

fn replay(
    tx: &Transaction,
    actor: &Actor,
    key: &str,
    action: &str,
    args: &Value,
    f: impl FnOnce() -> Result<Value>,
) -> Result<Value> {
    ensure!(!key.is_empty(), "Expected idempotency key");
    let digest = hash(canonical(&json!([action, args])).as_bytes());
    let prior: Option<(String, String)> = tx
        .query_row(
            "SELECT digest,receipt FROM pv_request WHERE task=? AND actor=? AND key=?",
            params![actor.task, actor.id, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((stored, receipt)) = prior {
        ensure!(
            stored == digest,
            "Idempotency key reused with different content"
        );
        return Ok(serde_json::from_str(&receipt)?);
    }
    let receipt = f()?;
    tx.execute(
        "INSERT INTO pv_request VALUES(?,?,?,?,?)",
        params![actor.task, actor.id, key, digest, receipt.to_string()],
    )?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Result<Connection> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
        schema(&db)?;
        Ok(db)
    }

    #[test]
    fn invalid_last_edge_rolls_back_the_batch_and_head() -> Result<()> {
        let mut db = db()?;
        let actor = access::test_controller("t");
        let batch = json!({"idempotency_key":"k","versions":[
            {"artifact":"a","expected":0,"kind":"document","blocks":[{"id":"b","text":"x","original":true}]},
            {"artifact":"d","expected":0,"kind":"decision","blocks":[{"id":"b","text":"x","cites":[{"relation":"quotes","source":{"artifact":"a","version":1,"block":"missing"}}]}]}]});
        {
            let tx = db.transaction()?;
            assert!(versions::commit(&tx, &actor, serde_json::from_value(batch)?).is_err());
        }
        assert_eq!(
            db.query_row("SELECT count(*) FROM pv_artifact", [], |r| r
                .get::<_, usize>(0))?,
            0
        );
        Ok(())
    }

    #[test]
    fn receipt_replay_and_frozen_graph_position() -> Result<()> {
        let mut db = db()?;
        let actor = access::test_controller("t");
        let tx = db.transaction()?;
        let batch = json!({"idempotency_key":"k","versions":[{"artifact":"a","expected":0,"kind":"decision","blocks":[{"id":"b","text":"x","original":true}]}]});
        let receipt = versions::commit(&tx, &actor, serde_json::from_value(batch)?)?;
        history::proposed(&tx, "t", &receipt)?;
        let args = json!({"idempotency_key":"pick","artifact":"a","version":1,"choice":"accept","seen_cursor":1});
        let result = replay(&tx, &actor, "pick", "decide", &args, || {
            history::decide(&tx, &actor, serde_json::from_value(args.clone())?)
        })?;
        assert_eq!(
            result,
            replay(&tx, &actor, "pick", "decide", &args, || unreachable!())?
        );
        assert!(replay(&tx, &actor, "pick", "decide", &json!({}), || unreachable!()).is_err());
        assert_eq!(
            history::view(&tx, &actor, 1)?["event"]["kind"],
            "versions_registered"
        );
        assert_eq!(
            history::view(&tx, &actor, 2)?["event"]["kind"],
            "decision_recorded"
        );
        tx.commit()?;
        Ok(())
    }
}
