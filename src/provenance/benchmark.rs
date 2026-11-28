//! Fixed workload for the actual SQLite-backed application projections.
use super::{access, history, query, schema};
use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{hint::black_box, time::Instant};

fn documents(db: &Connection, task: &str, prefix: &str, count: usize) -> Result<()> {
    for i in 0..count {
        let id = format!("{prefix}{i}");
        db.execute(
            "INSERT INTO pv_artifact VALUES(?,?,'document',1)",
            params![id, task],
        )?;
        db.execute(
            "INSERT INTO pv_version VALUES(?,1,?,1,'benchmark',NULL,NULL,'[]')",
            params![id, task],
        )?;
        db.execute(
            "INSERT INTO pv_block VALUES(?,1,'text',?)",
            params![id, r#"{"text":"recorded"}"#],
        )?;
        if i > 0 {
            db.execute(
                "INSERT INTO pv_edge VALUES(?,1,'text',?,1,'text','quotes')",
                params![id, format!("{prefix}{}", i - 1)],
            )?;
        }
    }
    Ok(())
}

fn measure(name: &str, iterations: usize, mut read: impl FnMut() -> Result<Value>) -> Result<()> {
    black_box(read()?);
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        black_box(read()?);
        samples.push(start.elapsed().as_nanos());
    }
    samples.sort_unstable();
    println!(
        "{name}: median {:.1} us, p95 {:.1} us",
        samples[iterations / 2] as f64 / 1000.0,
        samples[iterations * 95 / 100] as f64 / 1000.0
    );
    Ok(())
}

#[test]
#[ignore = "release query benchmark"]
fn projections() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
    schema(&db)?;
    let tx = db.transaction()?;
    documents(&tx, "t", "a", 8)?;
    tx.execute("INSERT INTO pv_point VALUES('t',256,1,NULL,NULL)", [])?;
    for node in 0..32 {
        tx.execute("INSERT INTO pv_node VALUES('t',?,1)", [node.to_string()])?;
        for cursor in (1..=256).step_by(8) {
            tx.execute(
                "INSERT INTO pv_node_history VALUES('t',?,?,?)",
                params![
                    node.to_string(),
                    cursor,
                    json!({"node":node,"activity":{"attempt":cursor}}).to_string()
                ],
            )?;
        }
    }
    tx.commit()?;
    for unrelated in [0, 50_000] {
        if unrelated > 0 {
            let tx = db.transaction()?;
            documents(&tx, "other", "b", unrelated + 1)?;
            tx.commit()?;
        }
        println!("{unrelated} unrelated edges; 200 reads per projection");
        let actor = access::test_controller("t");
        for (name, id, direction) in [
            ("sources", "a7", query::Direction::Sources),
            ("impact", "a0", query::Direction::Impact),
        ] {
            let start = json!({"artifact":id,"version":1,"block":"text"});
            measure(name, 200, || query::trace(&db, &actor, &start, direction))?;
        }
        measure("history", 200, || history::view(&db, &actor, 256))?;
    }
    for (name, sql) in [
        ("sources", include_str!("sources.sql")),
        ("impact", include_str!("impact.sql")),
        ("history", include_str!("history.sql")),
    ] {
        let mut explain = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        let mut rows = explain.raw_query();
        let mut steps = vec![];
        while let Some(row) = rows.next()? {
            steps.push(row.get::<_, String>(3)?);
        }
        if name == "impact" {
            assert!(steps.iter().any(|s| s.contains("pv_consumers")));
        }
        println!("{name} plan: {}", steps.join(" / "));
    }
    Ok(())
}
