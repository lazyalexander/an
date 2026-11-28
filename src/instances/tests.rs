use super::{lifecycle::save_observation, worker::reap};
use super::{request_task_unload, submit};
use super::{save, string};
use crate::engine::capacity;
use crate::permission;
use crate::store::{connect, get, put, revision, root, uid};
use anyhow::{ensure, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::process::Command;

#[test]
fn cancellation_waits_for_a_competing_capacity_writer() -> Result<()> {
    let file = std::env::temp_dir().join(uid());
    let mut db = Connection::open(&file)?;
    db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
    let tx = db.transaction()?;
    save(
        &tx,
        "run",
        &json!({"id":"a", "status":"cancel_pending", "capacity_managed":true}),
    )?;
    capacity::enqueue(&tx, "a")?;
    tx.execute("UPDATE execution_queue SET admitted=1 WHERE run='a'", [])?;
    tx.commit()?;
    let (reserved, wait) = std::sync::mpsc::channel();
    let writer = Connection::open(&file)?;
    let other = std::thread::spawn(move || -> Result<()> {
        let mut writer = writer;
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        capacity::enqueue(&tx, "b")?;
        reserved.send(())?;
        std::thread::sleep(std::time::Duration::from_millis(300));
        tx.commit()?;
        Ok(())
    });
    wait.recv()?;
    let mut instance =
        json!({"id":"i","active":"a","intent":"stop","status":"stopped","observed":"stopped"});
    let result = save_observation(
        &mut db,
        &mut instance,
        crate::engine::Termination::Confirmed,
    );
    other.join().unwrap()?;
    result?;
    assert_eq!(get(&db, "run", "a")?["status"], "cancelled");
    assert!(get(&db, "instance", "i")?["active"].is_null());
    let remaining: Vec<String> = db
        .prepare("SELECT run FROM execution_queue")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    assert_eq!(remaining, ["b"]);
    drop(db);
    fs::remove_file(file)?;
    Ok(())
}
#[test]
fn closing_marks_only_this_tasks_settled_instances() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
    for (id, task, active) in [
        ("warm", "one", Value::Null),
        ("busy", "one", json!("attempt")),
        ("other", "two", Value::Null),
    ] {
        put(
            &db,
            "instance",
            id,
            &json!({"task":task,"id":id,"status":"running","intent":"load","active":active}),
        )?;
    }
    {
        let tx = db.transaction()?;
        assert!(request_task_unload(&tx, "one").is_err());
    }
    assert_eq!(get(&db, "instance", "warm")?["intent"], "load");
    let mut busy = get(&db, "instance", "busy")?;
    busy["active"] = Value::Null;
    save(&db, "instance", &busy)?;
    request_task_unload(&db, "one")?;
    request_task_unload(&db, "one")?;
    assert_eq!(get(&db, "instance", "warm")?["intent"], "unload");
    assert_eq!(get(&db, "instance", "busy")?["intent"], "unload");
    assert_eq!(get(&db, "instance", "other")?["intent"], "load");
    Ok(())
}
#[test]
fn closed_task_retries_owner_checked_cleanup_after_restart() -> Result<()> {
    // Exercise graph finish in separate host processes against one durable registry.
    // Only the container CLI boundary is replaced in the test executable.
    if std::env::var_os("AN_CLEANUP_CHILD").is_some() {
        let report = crate::graph::host("t", "finish", &json!({}))?;
        fs::write(root()?.join("finish.json"), report.to_string())?;
        return Ok(());
    }

    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(uid());
    fs::create_dir_all(dir.join("instances/i"))?;
    let executable = dir.join("container");
    fs::write(
        &executable,
        r#"#!/bin/sh
set -eu
cd "$AN_HOME"
echo "$*" >> calls
case "$1" in
    list)
        if test ! -f inventory; then echo unavailable >&2; exit 1; fi
        cat inventory ;;
    delete)
        if test -f busy; then echo busy >&2; exit 1; fi
        echo '[]' > inventory ;;
    *) exit 2 ;;
esac
"#,
    )?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    let db = Connection::open(dir.join("registry.sqlite3"))?;
    db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
    put(&db, "task", "t", &json!({"work": {}}))?;
    put(&db, "turn", "t", &json!("owner"))?;
    let instance = json!({"id":"i", "task":"t", "name":"an-i", "spool":"demo@sha256:abc",
                             "intent":"load", "status":"running", "active":null});
    save(&db, "instance", &instance)?;

    let finish = || -> Result<Value> {
        fs::write(dir.join("calls"), "")?;
        let result = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "instances::tests::closed_task_retries_owner_checked_cleanup_after_restart",
            ])
            .env("AN_CLEANUP_CHILD", "1")
            .env("AN_HOME", &dir)
            .env("AN_TURN", "owner")
            .env("AN_TEST_CONTAINER", &executable)
            .output()?;
        ensure!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        Ok(serde_json::from_slice(&fs::read(dir.join("finish.json"))?)?)
    };
    let pending = json!({"closed": true, "cleanup_pending": ["i"]});
    let done = json!({"closed": true, "cleanup_pending": []});
    let list = "list --all --format json\n";
    assert_eq!(finish()?, pending);
    assert_eq!(get(&db, "task", "t")?["closed"], true);
    assert!(get(&db, "turn", "t")?.is_null());
    let saved = get(&db, "instance", "i")?;
    assert_eq!(saved["intent"], "unload");
    assert_eq!(saved["observed"], "unknown");
    assert_eq!(saved["status"], "cleanup_pending");
    assert_eq!(fs::read_to_string(dir.join("calls"))?, list);

    let inventory = |label: &str, digest: &str| -> Result<()> {
        let item = json!({"id":"an-i", "status":{"state":"stopped"},
                        "configuration":{"labels":{"an.instance":label},
                                         "image":{"descriptor":{"digest":digest}}}});
        Ok(fs::write(dir.join("inventory"), json!([item]).to_string())?)
    };
    for (label, digest) in [("foreign", "sha256:abc"), ("i", "sha256:other")] {
        inventory(label, digest)?;
        assert_eq!(finish()?, pending);
        assert_eq!(get(&db, "instance", "i")?["observed"], "identity_mismatch");
        assert_eq!(fs::read_to_string(dir.join("calls"))?, list);
    }

    inventory("i", "sha256:abc")?;
    fs::write(dir.join("busy"), "")?;
    assert_eq!(finish()?, pending);
    let delete = format!("{list}delete an-i\n{list}");
    assert_eq!(fs::read_to_string(dir.join("calls"))?, delete);
    assert!(dir.join("instances/i").is_dir());

    fs::remove_file(dir.join("busy"))?;
    assert_eq!(finish()?, done);
    assert_eq!(fs::read_to_string(dir.join("calls"))?, delete);
    assert_eq!(get(&db, "instance", "i")?["status"], "unloaded");
    assert!(!dir.join("instances/i").exists());
    assert_eq!(finish()?, done);
    assert_eq!(fs::read_to_string(dir.join("calls"))?, "");
    assert_eq!(
        get(&db, "graph", "t")?["events"].as_array().unwrap().len(),
        1
    );
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn named_inputs_preserve_approvals_and_tickets_until_all_ports_commit() -> Result<()> {
    // One subprocess owns its environment and throwaway registry.
    if std::env::var_os("AN_RESERVATION_CHILD").is_none() {
        let dir = std::env::temp_dir().join(uid());
        let status = Command::new(std::env::current_exe()?)
                .args(["--exact", "instances::tests::named_inputs_preserve_approvals_and_tickets_until_all_ports_commit"])
                .env("AN_HOME", &dir).env("AN_TURN", "turn").env("AN_RESERVATION_CHILD", "1")
                .status()?;
        if dir.is_dir() {
            fs::remove_dir_all(dir)?;
        }
        assert!(status.success());
        return Ok(());
    }
    let mut db = connect()?;
    let tx = db.transaction()?;
    let rev = revision(&tx, &json!({"data": "YQ=="}))?;
    let t = json!({"work":{"dest":rev},"inputs":{"second":rev,"third":rev}});
    let mut i = json!({"id":"i","task":"t","spool":"dest","status":"running","intent":"load"});
    let manifest = json!({"inputs":[{"name":"a","required":true},{"name":"b","required":true},{"name":"c","required":true}]});
    let mut args = json!({"argv":["run"],"inputs":{"a":"handoff-one","b":"second","c":"third"}});
    put(&tx, "task", "t", &t)?;
    tx.commit()?;
    put(&db, "turn", "t", &json!("turn"))?;
    let ticket =
        json!({"task":"t","recipient":"dest","revision":rev,"stamp":permission::stamp(&db,"t")?});
    put(&db, "handoff", "handoff-one", &ticket)?;
    let fail = |_: &str, _: &File| Err(std::io::Error::other("injected launch failure"));
    let mut missing = args.clone();
    missing["inputs"].as_object_mut().unwrap().remove("c");
    assert!(submit(&mut db, "t", &t, &mut i, &missing, &manifest, fail).is_err());
    let mut invalid = args.clone();
    invalid["inputs"]["extra"] = json!("second");
    assert!(submit(&mut db, "t", &t, &mut i, &invalid, &manifest, fail).is_err());
    for name in ["b", "c"] {
        let pending = submit(&mut db, "t", &t, &mut i, &args, &manifest, fail)?;
        assert_eq!(pending["port"], name);
        let id = string(&pending, "request_id")?;
        let mut request = get(&db, "request", id)?;
        request["state"] = json!("once");
        put(&db, "request", id, &request)?;
        args["input_requests"][name] = json!(id);
        assert_eq!(get(&db, "handoff", "handoff-one")?, ticket);
        let first = string(&args["input_requests"], "b")?;
        assert_eq!(get(&db, "request", first)?["state"], "once");
    }
    let failed = submit(&mut db, "t", &t, &mut i, &args, &manifest, fail)?;
    let run = get(&db, "run", failed.as_str().unwrap())?;
    assert_eq!(run["status"], "failed");
    assert!(get(&db, "instance", "i")?["active"].is_null());
    assert_eq!(get(&db, "handoff", "handoff-one")?, ticket);
    for name in ["b", "c"] {
        assert_eq!(
            get(&db, "request", string(&args["input_requests"], name)?)?["state"],
            "once"
        );
    }
    let launch = |_: &str, _: &File| Command::new("/usr/bin/true").spawn();
    let accepted = submit(&mut db, "t", &t, &mut i, &args, &manifest, launch)?;
    let run = get(&db, "run", accepted.as_str().unwrap())?;
    assert_eq!(
        get(&db, "revision", string(&run, "input")?)?,
        json!({"a/data":"YQ==","b/data":"YQ==","c/data":"YQ=="})
    );
    assert!(run["worker_pid"].as_u64().is_some());
    assert_eq!(get(&db, "handoff", "handoff-one")?["used"], accepted);
    Ok(())
}
#[test]
fn completed_child_is_reaped_while_parent_remains_alive() {
    let child = Command::new("/usr/bin/true").spawn().unwrap();
    let pid = child.id() as libc::pid_t;
    reap(child);
    for _ in 0..200 {
        if unsafe { libc::kill(pid, 0) } < 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("worker was not reaped");
}
