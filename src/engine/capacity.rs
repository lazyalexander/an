//! Queue tickets live in the same SQLite transaction as run and instance records.
use crate::store::root;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::fs::File;

pub fn enqueue(tx: &Transaction, run: &str) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS execution_queue (
        ticket INTEGER PRIMARY KEY, run TEXT NOT NULL UNIQUE, admitted INTEGER NOT NULL DEFAULT 0
    )",
    )?;
    tx.execute("INSERT INTO execution_queue(run) VALUES (?)", [run])?;
    Ok(())
}

pub fn release(db: &Connection, run: &str) -> Result<()> {
    db.execute("DELETE FROM execution_queue WHERE run=?", [run])?;
    Ok(())
}

pub fn position(db: &Connection, run: &str) -> Result<Option<u64>> {
    Ok(db
        .query_row(
            "SELECT 1+(SELECT count(*) FROM execution_queue earlier
        WHERE earlier.admitted=0 AND earlier.ticket<q.ticket)
        FROM execution_queue q WHERE run=? AND admitted=0",
            [run],
            |row| row.get(0),
        )
        .optional()?)
}

fn worker_alive(run: &str) -> Result<bool> {
    let lease = File::open(root()?.join("runs").join(run).join("lease"))?;
    match lease.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

// Caller holds an IMMEDIATE transaction and its own task lock. Only unadmitted
// tickets can be discarded on lease loss: their guest command never started.
fn claim(
    tx: &Transaction,
    run: &str,
    limit: u64,
    alive: impl Fn(&str) -> Result<bool>,
) -> Result<bool> {
    let used: u64 = tx.query_row(
        "SELECT (SELECT count(*) FROM execution_queue WHERE admitted=1)
            + (SELECT count(*) FROM objects i JOIN objects r
                ON r.kind='run' AND r.id=json_extract(i.body,'$.active')
                WHERE i.kind='instance' AND json_extract(r.body,'$.capacity_managed') IS NULL)",
        [],
        |r| r.get(0),
    )?;
    if used >= limit {
        return Ok(false);
    }
    let mut stmt =
        tx.prepare("SELECT run FROM execution_queue WHERE admitted=0 ORDER BY ticket")?;
    let mut rows = stmt.query([])?;
    let mut abandoned = Vec::new();
    let mut ahead = 0;
    while let Some(row) = rows.next()? {
        let other: String = row.get(0)?;
        if other == run {
            break;
        }
        if alive(&other)? {
            ahead += 1;
        } else {
            abandoned.push(other);
        }
        if ahead >= limit - used {
            return Ok(false);
        }
    }
    drop(rows);
    drop(stmt);
    for id in abandoned {
        release(tx, &id)?;
    }
    anyhow::ensure!(
        tx.execute(
            "UPDATE execution_queue SET admitted=1 WHERE run=? AND admitted=0",
            [run]
        )? == 1,
        "Missing execution ticket"
    );
    Ok(true)
}

pub fn wait(
    db: &mut Connection,
    task: &str,
    instance: &str,
    run: &str,
    mut lock: File,
) -> Result<Option<File>> {
    loop {
        let i = crate::store::get(db, "instance", instance)?;
        if i["active"] != run || i["intent"] != "load" {
            return Ok(None);
        }
        let limit = crate::config::load()?.limit("max_concurrent_runs");
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let ready = claim(&tx, run, limit, worker_alive)
            .context("Checking container execution capacity")?;
        tx.commit()?;
        if ready {
            return Ok(Some(lock));
        }
        drop(lock);
        std::thread::sleep(std::time::Duration::from_millis(200));
        lock = crate::instances::lock(db, task)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_workers_can_be_skipped_only_before_admission() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
        let tx = db.transaction()?;
        for id in ["first", "second", "third"] {
            enqueue(&tx, id)?;
        }
        assert!(!claim(&tx, "second", 1, |_| Ok(true))?);
        assert!(claim(&tx, "second", 1, |_| Ok(false))?);
        assert_eq!(position(&tx, "third")?, Some(1));
        assert!(!claim(&tx, "third", 1, |_| Ok(false))?);
        release(&tx, "second")?;
        assert!(claim(&tx, "third", 1, |_| Ok(true))?);
        Ok(())
    }
}
