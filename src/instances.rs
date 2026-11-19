use crate::{artifacts, permission, runtime, store::*};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{ErrorKind, Read, Seek, SeekFrom},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().with_context(|| format!("Expected {k}"))
}
fn lock(db: &Connection, task: &str) -> Result<File> {
    get(db, "task", task)?;
    ensure!(
        !task.contains('/') && task != "." && task != "..",
        "Invalid task ID"
    );
    let dir = root()?.join("locks");
    fs::create_dir_all(&dir)?;
    let f = File::options()
        .create(true)
        .append(true)
        .open(dir.join(task))?;
    f.lock()?;
    Ok(f)
}
fn path(i: &Value) -> Result<PathBuf> {
    Ok(root()?.join("instances").join(string(i, "id")?))
}
fn save(db: &Connection, kind: &str, r: &Value) -> Result<()> {
    put(db, kind, string(r, "id")?, r)
}
fn items(db: &Connection, kind: &str, task: &str) -> Result<Vec<Value>> {
    let mut q = db.prepare(
        "SELECT body FROM objects WHERE kind=? AND json_extract(body,'$.task')=? ORDER BY rowid",
    )?;
    let mut rows = q.query([kind, task])?;
    let mut out = vec![];
    while let Some(row) = rows.next()? {
        out.push(serde_json::from_str(&row.get::<_, String>(0)?)?);
    }
    Ok(out)
}
fn observe(i: &mut Value, f: &File) -> String {
    match runtime::list(f) {
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
fn reconcile(db: &mut Connection, i: &mut Value, f: &File) -> Result<()> {
    if i["status"] == "unloaded" {
        return Ok(());
    }
    let mut actual = observe(i, f);
    let active = i["active"].as_str().map(str::to_owned);
    i["checked_at"] = json!(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64());
    if let Some(id) = &active {
        let lease = File::options()
            .create(true)
            .append(true)
            .open(root()?.join("runs").join(id).join("lease"))?;
        let mut run = get(db, "run", id)?;
        if lease.try_lock().is_ok() && run["status"] == "running" {
            run["status"] = json!("unknown");
            save(db, "run", &run)?;
        }
    }
    let unloading = i["intent"] == "unload";
    let stopping = i["intent"] == "stop";
    if unloading || stopping {
        if actual == "running" {
            if let Err(e) = runtime::apple(&["stop", string(i, "name")?], Some(f)) {
                i["error"] = json!(e.to_string());
            }
            actual = observe(i, f);
        }
        if unloading && matches!(actual.as_str(), "stopped" | "created" | "exited") {
            if let Err(e) = runtime::apple(&["delete", string(i, "name")?], Some(f)) {
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
    let tx = db.transaction()?;
    if (stopping || unloading) && i["status"] != "cleanup_pending" {
        if let Some(id) = active {
            let mut r = get(&tx, "run", &id)?;
            r["status"] = json!("cancelled");
            save(&tx, "run", &r)?;
            i["active"] = Value::Null;
        }
    }
    if i["status"] == "unloaded" {
        if let Err(e) = fs::remove_dir_all(path(i)?) {
            if e.kind() != ErrorKind::NotFound {
                i["status"] = json!("cleanup_pending");
                i["intent"] = json!("unload");
                i["error"] = json!(e.to_string());
            }
        }
    }
    i["observed"] = json!(actual);
    save(&tx, "instance", i)?;
    tx.commit()?;
    Ok(())
}
fn load(db: &mut Connection, task: &str, t: &Value, image: &str, f: &File) -> Result<Value> {
    let work = t["work"][image].as_str().context("Spool outside task")?;
    let platform = runtime::require(db, image)?["platform"].clone();
    for mut i in items(db, "instance", task)? {
        if i["spool"] != image || i["status"] == "unloaded" {
            continue;
        }
        reconcile(db, &mut i, f)?;
        if i["status"] == "running" {
            ensure!(
                i["platform"] == platform,
                "Unload instance to reload admitted platform"
            );
            return Ok(i);
        }
        if i["status"] == "stopped" && i["active"].is_null() {
            i["intent"] = json!("unload");
            save(db, "instance", &i)?;
            reconcile(db, &mut i, f)?;
        }
        ensure!(
            i["status"] == "unloaded",
            "Resolve previous instance before loading"
        );
    }
    let id = uid();
    let mut i = json!({"id":id,"name":format!("an-{id}"),"task":task,"spool":image,"platform":platform,"status":"loading","intent":"load","active":null});
    save(db, "instance", &i)?;
    let setup = (|| -> Result<()> {
        let p = path(&i)?;
        fs::create_dir_all(&p)?;
        artifacts::stage(&p.join("work"), &get(db, "revision", work)?, false)?;
        fs::create_dir(p.join("input"))?;
        fs::create_dir(p.join("output"))?;
        runtime::start(
            image,
            string(&i, "name")?,
            &format!("an.instance={id}"),
            &p,
            t["editable"]
                .as_array()
                .is_some_and(|v| v.contains(&json!(image))),
            &platform,
            f,
        )
    })();
    if let Err(e) = setup {
        i["intent"] = json!("unload");
        i["error"] = json!(e.to_string());
        save(db, "instance", &i)?;
        reconcile(db, &mut i, f)?;
        bail!("Failed to load {id}: {e}")
    }
    reconcile(db, &mut i, f)?;
    ensure!(i["status"] == "running", "Instance failed to start: {i}");
    Ok(i)
}
fn owned(db: &Connection, task: &str, id: &str) -> Result<Value> {
    let r = get(db, "run", id)?;
    ensure!(r["task"] == task, "Run outside task");
    Ok(r)
}
fn submit(
    db: &mut Connection,
    task: &str,
    t: &Value,
    i: &mut Value,
    args: &Value,
) -> Result<Value> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let spool = string(i, "spool")?;
    ensure!(
        i["status"] == "running" && i["intent"] == "load" && i["active"].is_null(),
        "Instance is not ready"
    );
    let editable = t["editable"]
        .as_array()
        .is_some_and(|v| v.contains(&i["spool"]));
    if editable {
        let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE kind='run' AND json_extract(body,'$.task')=? AND json_extract(body,'$.spool')=? AND json_extract(body,'$.instance') IS NULL AND json_extract(body,'$.status') IN ('running','unknown','cancel_pending'))",[task,spool],|r|r.get(0))?;
        ensure!(!busy, "Unresolved legacy run owns editable spool");
    }
    let argv: Vec<String> = serde_json::from_value(args["argv"].clone())?;
    ensure!(
        argv.first()
            .is_some_and(|s| !s.is_empty() && !s.starts_with('-')),
        "Expected a guest executable"
    );
    let outputs: Vec<String> =
        serde_json::from_value(args.get("outputs").cloned().unwrap_or(json!([])))?;
    for p in &outputs {
        artifacts::safe(p)?;
    }
    let mut ticket = None;
    let mut input_stamp = Value::Null;
    let inp = if let Some(source) = args["input"].as_str() {
        input_stamp = permission::stamp(&tx, task)?;
        if source.starts_with("handoff-") && t["inputs"].get(source).is_none() {
            let r = get(&tx, "handoff", source)?;
            ensure!(
                r["task"] == task
                    && r["recipient"] == spool
                    && r["used"].is_null()
                    && r["stamp"] == input_stamp,
                "Invalid or revoked handoff"
            );
            let rev = r["revision"].clone();
            ticket = Some((source, r));
            rev
        } else {
            let (rev, flow) = if let Some(rev) = t["inputs"].get(source) {
                (rev.clone(), json!(["input", source, spool]))
            } else {
                let up = get(&tx, "run", source)?;
                if up["task"] != task {
                    ensure!(
                        t["workspace"].is_string()
                            && up["outputs"].as_array().is_some_and(|a| !a.is_empty())
                            && get(&tx, "task", string(&up, "task")?)?["workspace"]
                                == t["workspace"],
                        "Delivery outside workspace"
                    );
                }
                ensure!(up["status"] == "delivered", "Input has not been delivered");
                (up["output"].clone(), json!(["handoff", up["spool"], spool]))
            };
            if let Some(p) = permission::gate(
                &tx,
                task,
                "input",
                json!([source, rev, spool]),
                json!([i["id"], argv, outputs]),
                args["request_id"].as_str(),
                Some(flow),
            )? {
                tx.commit()?;
                return Ok(p);
            }
            rev
        }
    } else {
        json!(revision(&tx, &json!({}))?)
    };
    let id = uid();
    let dir = root()?.join("runs").join(&id);
    fs::create_dir_all(&dir)?;
    let lease = File::create_new(dir.join("lease"))?;
    lease.lock()?;
    let mut r = json!({"id":id,"instance":i["id"],"task":task,"spool":spool,"work":t["work"][spool],"argv":argv,"input":inp,"source":args["input"],"outputs":outputs,"editable":editable,"status":"running","input_stamp":input_stamp});
    if let Some((key, record)) = &ticket {
        let mut used = record.clone();
        used["used"] = json!(id);
        put(&tx, "handoff", key, &used)?;
    }
    i["active"] = json!(id);
    save(&tx, "run", &r)?;
    save(&tx, "instance", i)?;
    tx.commit()?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["_worker", &id, &lease.as_raw_fd().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    runtime::inherited(&mut cmd, &lease);
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    match cmd.spawn() {
        Ok(child) => reap(child),
        Err(e) => {
            let tx = db.transaction()?;
            if let Some((key, record)) = ticket {
                put(&tx, "handoff", key, &record)?;
            }
            r["status"] = json!("failed");
            r["error"] = json!(format!("Worker launch failed: {e}"));
            i["active"] = Value::Null;
            save(&tx, "run", &r)?;
            save(&tx, "instance", i)?;
            tx.commit()?;
        }
    }
    Ok(json!(id))
}
fn reap(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

pub fn dispatch(task: &str, op: &str, args: &Value) -> Result<Value> {
    let mut db = connect()?;
    let lock = lock(&db, task)?;
    let t = get(&db, "task", task)?;
    match op {
        "load" => return load(&mut db, task, &t, string(args, "spool")?, &lock),
        "skill" => {
            let s = string(args, "spool")?;
            ensure!(t["work"].get(s).is_some(), "Spool outside task");
            return runtime::require(&db, s);
        }
        "state" => {
            let mut result = t.clone();
            let mut instances = items(&db, "instance", task)?;
            for i in &mut instances {
                reconcile(&mut db, i, &lock)?;
            }
            let mut spools = json!({});
            for s in t["work"].as_object().context("Invalid task")?.keys() {
                spools[s] = runtime::summary(&db, s)?;
            }
            let mut runs = json!({});
            let mut stmt = db.prepare("SELECT id, json_extract(body, '$.status') FROM objects WHERE kind='run' AND json_extract(body, '$.task')=?")?;
            let mut rows = stmt.query([task])?;
            while let Some(row) = rows.next()? {
                runs[row.get::<_, String>(0)?] = json!(row.get::<_, String>(1)?);
            }
            result["task"] = json!(task);
            result["spools"] = spools;
            result["runs"] = runs;
            result["instances"] = json!(instances);
            return Ok(result);
        }
        "publish" | "handoff" | "status" | "cancel" | "log" => {
            let id = string(args, "run")?;
            let mut r = owned(&db, task, id)?;
            if matches!(op, "publish" | "handoff") {
                ensure!(
                    r["status"] == "awaiting_publication",
                    "Expected private candidate"
                );
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let files = get(&tx, "revision", string(&r, "candidate")?)?;
                let files = files.as_object().context("Invalid revision")?;
                let mut manifest = vec![];
                let mut names = files.keys().collect::<Vec<_>>();
                names.sort();
                for name in &names {
                    let data = artifacts::bytes(&files[*name])?;
                    manifest.push(json!([name, data.len(), hash(&data)]));
                }
                let dest = if op == "handoff" {
                    let s = string(args, "recipient_spool")?;
                    ensure!(t["work"].get(s).is_some(), "Recipient outside task");
                    s
                } else {
                    t["workspace"].as_str().unwrap_or(task)
                };
                let result = if let Some(p) = permission::gate(
                    &tx,
                    task,
                    op,
                    json!([r["spool"], names, dest]),
                    json!([id, manifest]),
                    args["request_id"].as_str(),
                    Some(json!([op, r["spool"], dest])),
                )? {
                    p
                } else if op == "handoff" {
                    let key = format!("handoff-{}", uid());
                    put(
                        &tx,
                        "handoff",
                        &key,
                        &json!({"task":task,"recipient":dest,"source":id,"revision":r["candidate"],"stamp":permission::stamp(&tx,task)?}),
                    )?;
                    json!({"handoff":key})
                } else {
                    r["output"] = r.as_object_mut().unwrap().remove("candidate").unwrap();
                    r["status"] = json!("delivered");
                    save(&tx, "run", &r)?;
                    json!({"run":id,"status":"delivered"})
                };
                tx.commit()?;
                return Ok(result);
            }
            if op == "log" {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                if let Some(p) = permission::gate(
                    &tx,
                    task,
                    "log",
                    json!([id, "agent"]),
                    json!({}),
                    args["request_id"].as_str(),
                    None,
                )? {
                    tx.commit()?;
                    return Ok(p);
                }
                let mut bytes = vec![];
                match File::open(root()?.join("runs").join(id).join("log")) {
                    Ok(mut f) => {
                        f.seek(SeekFrom::Start(f.metadata()?.len().saturating_sub(65536)))?;
                        f.take(65536).read_to_end(&mut bytes)?;
                    }
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                let mut result = String::from_utf8_lossy(&bytes).into_owned();
                if let Some(err) = r["error"].as_str() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&format!("An: {err}"));
                }
                tx.commit()?;
                return Ok(json!(result));
            }
            if r.get("instance").is_none() {
                if op == "cancel" && r["status"] == "running" {
                    r["status"] = json!("cancel_pending");
                }
                if matches!(r["status"].as_str(), Some("running" | "cancel_pending")) {
                    if let Some(pid) = r["worker_pid"].as_i64() {
                        if unsafe { libc::kill(pid as i32, 0) } < 0
                            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                        {
                            r["status"] = json!("unknown");
                        }
                    }
                }
                put(&db, "run", id, &r)?;
                return Ok(r);
            }
            let mut i = get(&db, "instance", string(&r, "instance")?)?;
            if op == "cancel" && i["active"] == id && i["intent"] == "load" {
                let tx = db.transaction()?;
                i["intent"] = json!("stop");
                r["status"] = json!("cancel_pending");
                save(&tx, "instance", &i)?;
                save(&tx, "run", &r)?;
                tx.commit()?;
            }
            reconcile(&mut db, &mut i, &lock)?;
            let r = owned(&db, task, id)?;
            return Ok(json!({"id":id,"status":r["status"],"spool":r["spool"]}));
        }
        "run" | "unload" => {}
        _ => bail!("Unknown operation {op}"),
    }
    let mut i = if let Some(id) = args["instance"].as_str() {
        let i = get(&db, "instance", id)?;
        ensure!(
            i["task"] == task && (args["spool"].is_null() || i["spool"] == args["spool"]),
            "Instance outside requested task or spool"
        );
        i
    } else {
        items(&db, "instance", task)?
            .into_iter()
            .rev()
            .find(|i| i["spool"] == args["spool"] && i["status"] != "unloaded")
            .context("Load the spool first")?
    };
    if op == "unload" && i["status"] != "unloaded" {
        i["intent"] = json!("unload");
        let tx = db.transaction()?;
        if let Some(id) = i["active"].as_str() {
            let mut r = owned(&tx, task, id)?;
            r["status"] = json!("cancel_pending");
            save(&tx, "run", &r)?;
        }
        save(&tx, "instance", &i)?;
        tx.commit()?;
    }
    reconcile(&mut db, &mut i, &lock)?;
    if op == "unload" {
        return Ok(i);
    }
    ensure!(
        i["platform"] == runtime::require(&db, string(&i, "spool")?)?["platform"],
        "Reload instance on admitted platform"
    );
    submit(&mut db, task, &t, &mut i, args)
}
pub fn worker(id: &str, fd: i32) -> Result<()> {
    // The descriptor is the inherited run lease. Do not leak it into container exec.
    ensure!(
        fd >= 3 && unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
        "Missing worker lease"
    );
    let mut db = connect()?;
    let mut r = get(&db, "run", id)?;
    let task = string(&r, "task")?.to_owned();
    let instance = string(&r, "instance")?.to_owned();
    let mut quiet = true;
    let run = (|| -> Result<()> {
        let mut child = {
            let f = lock(&db, &task)?;
            let mut i = get(&db, "instance", &instance)?;
            if i["active"] != id || i["intent"] != "load" {
                return Ok(());
            }
            ensure!(
                observe(&mut i, &f) == "running",
                "Cannot verify running instance"
            );
            let p = path(&i)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !r["source"].is_null() {
                ensure!(
                    r["input_stamp"] == permission::stamp(&tx, &task)?,
                    "Input authority changed"
                );
            }
            artifacts::stage(
                &p.join("input"),
                &get(&tx, "revision", string(&r, "input")?)?,
                true,
            )?;
            tx.commit()?;
            artifacts::stage(&p.join("output"), &json!({}), true)?;
            artifacts::stage(
                &p.join("work"),
                &get(&db, "revision", string(&r, "work")?)?,
                true,
            )?;
            let log = File::create(root()?.join("runs").join(id).join("log"))?;
            quiet = false;
            Command::new(runtime::CONTAINER)
                .args(["exec", string(&i, "name")?])
                .args(serde_json::from_value::<Vec<String>>(r["argv"].clone())?)
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()?
        };
        let code = child.wait()?;
        let f = lock(&db, &task)?;
        let mut i = get(&db, "instance", &instance)?;
        if i["active"] != id || i["intent"] != "load" {
            return Ok(());
        }
        ensure!(
            observe(&mut i, &f) == "running",
            "Cannot verify instance at settlement"
        );
        runtime::apple(&["stop", string(&i, "name")?], Some(&f))?;
        ensure!(
            observe(&mut i, &f) == "stopped",
            "Cannot confirm guest termination"
        );
        quiet = true;
        runtime::apple(&["start", string(&i, "name")?], Some(&f))?;
        ensure!(
            observe(&mut i, &f) == "running",
            "Instance restart not confirmed"
        );
        ensure!(code.success(), "Guest command exited {code}");
        let p = path(&i)?;
        let mut files = json!({});
        for name in serde_json::from_value::<Vec<String>>(r["outputs"].clone())? {
            let file = p.join("output").join(artifacts::safe(&name)?);
            ensure!(
                file.is_file() && file.canonicalize()? == file,
                "Missing or invalid output: {name}"
            );
            files[name] = json!(STANDARD.encode(fs::read(file)?));
        }
        let tx = db.transaction()?;
        if files == json!({}) {
            r["status"] = json!("completed");
        } else {
            r["status"] = json!("awaiting_publication");
            r["candidate"] = json!(revision(&tx, &files)?);
        }
        if r["editable"] == true {
            let mut t = get(&tx, "task", &task)?;
            r["result_work"] = json!(revision(&tx, &artifacts::snapshot(&p.join("work"))?)?);
            t["work"][string(&i, "spool")?] = r["result_work"].clone();
            put(&tx, "task", &task, &t)?;
        }
        i["active"] = Value::Null;
        save(&tx, "run", &r)?;
        save(&tx, "instance", &i)?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = run {
        let _guard = lock(&db, &task)?;
        let mut i = get(&db, "instance", &instance)?;
        if i["active"] == id && i["intent"] == "load" {
            let mut r = get(&db, "run", id)?;
            r["status"] = json!("failed");
            r["error"] = json!(e.to_string());
            if quiet && r["editable"] == true {
                if let Err(e) = artifacts::stage(
                    &path(&i)?.join("work"),
                    &get(&db, "revision", string(&r, "work")?)?,
                    true,
                ) {
                    quiet = false;
                    i["error"] = json!(e.to_string());
                }
            }
            if !quiet {
                i["intent"] = json!("stop");
                i["status"] = json!("cleanup_pending");
            }
            i["active"] = Value::Null;
            let tx = db.transaction()?;
            save(&tx, "run", &r)?;
            save(&tx, "instance", &i)?;
            tx.commit()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
