use super::*;
use std::{
    sync::{Arc, Barrier},
    time::Duration,
};

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

#[test]
fn competing_revisions_keep_the_first_commit_and_its_predecessor() -> Result<()> {
    let path = std::env::temp_dir().join(crate::store::uid());
    let mut db = Connection::open(&path)?;
    db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
    schema(&db)?;
    fn batch(text: &str, previous: u32) -> Result<domain::Batch> {
        Ok(serde_json::from_value(json!({"idempotency_key":text,
            "versions":[{"artifact":"proposal","expected":previous,"kind":"decision",
            "blocks":[{"id":"text","text":text,"original":true}]}]}))?)
    }
    let tx = db.transaction()?;
    versions::commit(&tx, &access::test_controller("t"), batch("original", 0)?)?;
    tx.commit()?;
    let barrier = Arc::new(Barrier::new(2));
    let mut clients = vec![];
    for text in ["first", "second"] {
        let barrier = barrier.clone();
        let path = path.clone();
        clients.push(std::thread::spawn(move || -> Result<()> {
            let mut db = Connection::open(path)?;
            db.busy_timeout(Duration::from_secs(3))?;
            barrier.wait();
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            versions::commit(&tx, &access::test_controller("t"), batch(text, 1)?)?;
            Ok(tx.commit()?)
        }));
    }
    let mut conflicts = 0;
    for client in clients {
        if let Err(e) = client.join().unwrap() {
            assert_eq!(e.to_string(), "Version conflict");
            conflicts += 1;
        }
    }
    assert_eq!(conflicts, 1);
    let old = query::trace(
        &db,
        &access::test_controller("t"),
        &json!({"artifact":"proposal","version":1}),
        query::Direction::Sources,
    )?;
    assert_eq!(old["blocks"][0]["content"]["text"], "original");
    drop(db);
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn cycle_and_different_quote_cannot_be_committed() -> Result<()> {
    let mut db = db()?;
    for (text, expected) in [
        ("one", "Circular block reference"),
        ("two", "Quoted bytes differ"),
    ] {
        let tx = db.transaction()?;
        let mut blocks = vec![];
        for (id, source, value) in [("a", "b", "one"), ("b", "a", text)] {
            blocks.push(json!({"id":id,"text":value,"cites":[{"relation":"quotes","source":{"artifact":"a","version":1,"block":source}}]}));
        }
        let batch = serde_json::from_value(
            json!({"idempotency_key":"c","versions":[{"artifact":"a","expected":0,"kind":"document","blocks":blocks}]}),
        )?;
        let err = versions::commit(&tx, &access::test_controller("t"), batch).unwrap_err();
        assert!(err.to_string().contains(expected));
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM pv_edge", [], |r| r.get::<_, u32>(0))?,
        0
    );
    Ok(())
}

#[test]
fn replay_returns_the_assigned_artifact_id() -> Result<()> {
    let mut db = db()?;
    let tx = db.transaction()?;
    let actor = access::test_controller("t");
    let args = json!({"idempotency_key":"create","versions":[{
        "expected":0,"kind":"document","blocks":[{"id":"b","text":"first","original":true}]
    }]});
    let submit = || {
        replay(&tx, &actor, "create", "commit", &args, || {
            versions::commit(&tx, &actor, versions::parse(args.clone())?)
        })
    };
    let receipt = submit()?;
    assert_eq!(receipt, submit()?);
    assert!(uuid::Uuid::parse_str(receipt["versions"][0]["artifact"].as_str().unwrap()).is_ok());
    Ok(())
}

#[test]
fn moving_a_block_preserves_identity_and_removal_retires_it() -> Result<()> {
    let mut db = db()?;
    let actor = access::test_controller("t");
    let mut revise = |previous, names: &[&str]| -> Result<()> {
        let blocks: Vec<_> = names
            .iter()
            .map(|id| json!({"id":id,"text":id,"original":true}))
            .collect();
        let batch = versions::parse(json!({"idempotency_key":"edit","versions":[
            {"artifact":"doc","expected":previous,"kind":"document","blocks":blocks}
        ]}))?;
        let tx = db.transaction()?;
        versions::commit(&tx, &actor, batch)?;
        Ok(tx.commit()?)
    };
    revise(0, &["a", "b"])?;
    revise(1, &["b", "a"])?;
    revise(2, &["b"])?;
    assert!(revise(3, &["new", "a"])
        .unwrap_err()
        .to_string()
        .contains("Retired block ID"));
    revise(3, &["new", "b"])?;
    let view = query::trace(
        &db,
        &actor,
        &json!({"artifact":"doc","version":2}),
        query::Direction::Sources,
    )?;
    assert_eq!(view["blocks"][0]["ref"]["block"], "b");
    assert_eq!(view["blocks"][1]["ref"]["block"], "a");
    Ok(())
}

#[test]
fn split_and_merge_keep_both_paths_to_the_original_block() -> Result<()> {
    let mut db = db()?;
    let actor = access::test_controller("t");
    let mut edit = |expected, blocks: Vec<Value>| -> Result<()> {
        let tx = db.transaction()?;
        let batch = versions::parse(json!({"idempotency_key":"edit","versions":[{
            "artifact":"doc","expected":expected,"kind":"document","blocks":blocks
        }]}))?;
        versions::commit(&tx, &actor, batch)?;
        Ok(tx.commit()?)
    };
    let derived = |id, relation, version, parents: &[&str]| {
        let cites: Vec<_> = parents
            .iter()
            .map(|p| {
                json!({"relation":relation,
            "source":{"artifact":"doc","version":version,"block":p}})
            })
            .collect();
        json!({"id":id,"text":id,"cites":cites})
    };
    edit(
        0,
        vec![json!({"id":"original","text":"original","original":true})],
    )?;
    let left = derived("left", "split_from", 1, &["original"]);
    assert!(edit(1, vec![left.clone()]).is_err());
    edit(
        1,
        vec![left, derived("right", "split_from", 1, &["original"])],
    )?;
    assert!(edit(
        2,
        vec![derived("left", "merged_from", 2, &["left", "right"])]
    )
    .is_err());
    edit(
        2,
        vec![derived("combined", "merged_from", 2, &["left", "right"])],
    )?;
    for (version, block, direction) in [
        (1, "original", query::Direction::Impact),
        (3, "combined", query::Direction::Sources),
    ] {
        let trace = query::trace(
            &db,
            &actor,
            &json!({"artifact":"doc","version":version,"block":block}),
            direction,
        )?;
        assert_eq!(trace["edges"].as_array().unwrap().len(), 4);
        assert_eq!(trace["blocks"].as_array().unwrap().len(), 4);
    }
    Ok(())
}
