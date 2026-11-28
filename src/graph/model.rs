//! Host-only local inference. Commit the send intent before issuing HTTP. An
//! abandoned sending attempt is indeterminate and consumes its reserved slot.
use super::*;
use std::{
    fs,
    io::Write,
    os::fd::{AsRawFd, FromRawFd},
    process::{Command, Stdio},
};

fn messages(db: &Connection, revision_id: &str, limit: u64) -> Result<Value> {
    let files = get(db, "revision", revision_id)?;
    ensure!(
        files.as_object().is_some_and(|m| m.len() == 1),
        "Expected one request.json"
    );
    let data = artifacts::bytes(&files["request.json"])?;
    ensure!(data.len() as u64 <= limit, "Model input exceeds byte limit");
    let request: Value = serde_json::from_slice(&data)?;
    ensure!(
        request.as_object().is_some_and(|m| m.len() == 1),
        "Unexpected request fields"
    );
    let messages = request["messages"]
        .as_array()
        .context("Expected messages")?;
    ensure!(
        !messages.is_empty()
            && messages
                .iter()
                .all(|m| m.as_object().is_some_and(|o| o.len() == 2)
                    && matches!(m["role"].as_str(), Some("system" | "user" | "assistant"))
                    && m["content"].is_string()),
        "Invalid text messages"
    );
    Ok(json!(messages))
}

// A reservation that has provably never reached send intent can be released.
// Callers hold the task lock, which also serializes the worker's send transition.
pub(super) fn release_unsent(db: &Connection, id: &str, reason: &str) -> Result<()> {
    if let Some(mut call) = find(db, "model_call", id)? {
        if call["state"] == "reserved" {
            call["state"] = json!("withdrawn");
            call["error"] = json!(reason);
            call["settled_at"] = json!(now_ms());
            put(db, "model_call", id, &call)?;
        }
    }
    Ok(())
}

fn try_lease(id: &str) -> Result<Option<File>> {
    let dir = root()?.join("model-leases");
    fs::create_dir_all(&dir)?;
    let lease = File::options()
        .create(true)
        .append(true)
        .open(dir.join(id))?;
    match lease.try_lock() {
        Ok(()) => Ok(Some(lease)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

// Caller holds the task lock. A stopped node can still have a live worker.
pub(super) fn reconcile_sends(db: &Connection, task: &str) -> Result<()> {
    let mut query = db.prepare("SELECT body FROM objects WHERE kind='model_call' AND json_extract(body,'$.task')=? AND json_extract(body,'$.state')='sending'")?;
    let rows = query
        .query_map([task], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for body in rows {
        let mut call: Value = serde_json::from_str(&body)?;
        let id = field(&call, "attempt")?.to_owned();
        if let Some(_lease) = try_lease(&id)? {
            call["state"] = json!("unknown");
            call["settled_at"] = json!(now_ms());
            call["error"] = json!("Model worker lost after send intent; explicit retry required");
            put(db, "model_call", &id, &call)?;
        }
    }
    Ok(())
}

pub(super) fn schedule(
    db: &mut Connection,
    task: &str,
    node: &Value,
    bound: &Value,
    state: &mut Value,
    stamp: &Value,
    paused: bool,
) -> Result<()> {
    let id = field(state, "id")?.to_owned();
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let grant = get(&tx, "task", task)?["model_grant"].clone();
    ensure!(
        grant["alias"] == node["model"] && !grant["model"].is_null(),
        "Model outside task grant"
    );
    let input = field(&bound["request"], "revision")?;
    let dest = format!("model:{}", field(node, "model")?);
    let origin = binding_origin(&bound["request"]);
    let call = if let Some(call) = find(&tx, "model_call", &id)? {
        call
    } else {
        ensure!(
            grant["model"].is_string(),
            "This model requires the visual experiment branch"
        );
        if state["authority"] != *stamp {
            state["requests"] = json!({});
        }
        state["authority"] = stamp.clone();
        let cfg = crate::config::load()?;
        let budget = grant["budget"].as_u64().unwrap_or(0);
        let used: u64 = tx.query_row(
            "SELECT count(*) FROM objects WHERE kind='model_call'
             AND json_extract(body,'$.task')=? AND json_extract(body,'$.state')!='withdrawn'",
            [task],
            |row| row.get(0),
        )?;
        if used >= budget {
            if let Some(request) = state["requests"]["model"].as_str() {
                let mut row = get(&tx, "request", request)?;
                if row["state"] == "pending" {
                    row["state"] = json!("withdrawn");
                    put(&tx, "request", request, &row)?;
                }
            }
            tx.commit()?;
            anyhow::bail!("Model call budget exhausted ({used}/{budget})");
        }
        if let Some(p) = permission::graph_gate(
            &tx,
            task,
            "model",
            json!([origin, dest, input, grant["model"], id]),
            json!([id, input, grant["model"]]),
            state["requests"]["model"].as_str(),
            json!(["model", origin, dest]),
            crate::intent::graph_route(
                &tx,
                "model",
                &node["inputs"]["request"],
                &dest,
                &bound["request"],
            )?,
        )? {
            state["requests"]["model"] = p["request_id"].clone();
            state["status"] = json!(AttemptStatus::PermissionRequired);
            tx.commit()?;
            return Ok(());
        }
        messages(&tx, input, cfg.limit("model_input_bytes"))?;
        let call = json!({
            "task": task, "node": node["id"], "attempt": id, "alias": node["model"],
            "model": grant["model"], "kind":"text", "output_name":"result.json", "input": input, "input_origin": origin, "output": null,
            "authority": stamp, "state": "reserved", "reserved_at": now_ms(), "usage": null,
            "port": cfg.limit("ollama_port"), "timeout": cfg.limit("model_timeout_s"),
            "input_limit": cfg.limit("model_input_bytes"), "output_limit": cfg.limit("model_output_tokens")
        });
        put(&tx, "model_call", &id, &call)?;
        call
    };
    tx.commit()?;
    state["bindings"] = bound.clone();
    state["evidence"] = call.clone();
    if call["authority"] != *stamp {
        release_unsent(db, &id, "Model call authority expired")?;
        anyhow::bail!("Model call authority expired");
    }
    match field(&call, "state")? {
        "completed" => {
            let name = call["output_name"].as_str().unwrap_or("result.json");
            state["outputs"][name] = json!({"spool":dest,"revision":call["output"]});
            state["status"] = json!(AttemptStatus::Done);
        }
        "unknown" | "failed" => {
            state["status"] = call["state"].clone();
            state["error"] = call["error"].clone();
        }
        "withdrawn" => {
            state["status"] = json!(AttemptStatus::Failed);
            state["error"] = call["error"].clone();
        }
        "sending" => state["status"] = json!(AttemptStatus::Running),
        "reserved" if !call["model"].is_string() => {
            release_unsent(db, &id, "This model requires the visual experiment branch")?;
            anyhow::bail!("This model requires the visual experiment branch");
        }
        "reserved" => {
            state["status"] = json!(AttemptStatus::Running);
            if !paused {
                let lease = match try_lease(&id) {
                    Ok(Some(lease)) => lease,
                    Ok(None) => return Ok(()),
                    Err(e) => {
                        release_unsent(db, &id, "Could not acquire model worker lease")?;
                        return Err(e);
                    }
                };
                let mut cmd = Command::new(std::env::current_exe()?);
                cmd.args(["_model-worker", &id, &lease.as_raw_fd().to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::engine::apple::inherited(&mut cmd, &lease);
                let mut child = match cmd.spawn() {
                    Ok(child) => child,
                    Err(e) => {
                        release_unsent(db, &id, "Could not start model worker")?;
                        return Err(e.into());
                    }
                };
                std::thread::spawn(move || child.wait());
            }
        }
        _ => anyhow::bail!("Invalid model call state"),
    }
    Ok(())
}

#[derive(Debug)]
struct HttpStatus(u16);
impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            401 | 403 => write!(
                f,
                "Model backend rejected the credentials (HTTP {})",
                self.0
            ),
            _ => write!(f, "Model backend returned HTTP {}", self.0),
        }
    }
}
impl std::error::Error for HttpStatus {}

// Use the system HTTP client with a fixed loopback destination, no proxy,
// redirect, retries, ambient configuration, or inherited credentials.
fn ollama(call: &Value, path: &str, body: Option<&Value>) -> Result<Value> {
    let port = call["port"].as_u64().context("Missing model port")?;
    let mut cmd = Command::new("/usr/bin/curl");
    cmd.env_clear()
        .args([
            "-q",
            "--silent",
            "--write-out",
            "\n%{http_code}",
            "--noproxy",
            "*",
            "--max-time",
            &call["timeout"].to_string(),
            "--max-filesize",
            "4194304",
            &format!("http://127.0.0.1:{port}/api/{path}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if body.is_some() {
        cmd.args([
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ])
        .stdin(Stdio::piped());
    }
    let mut child = cmd.spawn()?;
    if let Some(body) = body {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(body)?)?;
    }
    let out = child.wait_with_output()?;
    ensure!(
        out.status.success(),
        "Local model HTTP call failed ({})",
        out.status
    );
    let split = out
        .stdout
        .iter()
        .rposition(|&b| b == b'\n')
        .context("Missing HTTP status")?;
    let status = std::str::from_utf8(&out.stdout[split + 1..])?.parse::<u16>()?;
    if !(200..300).contains(&status) {
        return Err(HttpStatus(status).into());
    }
    Ok(serde_json::from_slice(&out.stdout[..split])?)
}

pub fn worker(id: &str, fd: i32) -> Result<()> {
    ensure!(
        fd >= 3 && unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
        "Missing model lease"
    );
    let lease = unsafe { File::from_raw_fd(fd) };
    let mut db = connect()?;
    let mut call = get(&db, "model_call", id)?;
    let task = field(&call, "task")?.to_owned();
    // Metadata requests contain only the configured model name. Check locality
    // before reading the prompt, then recheck authority under the task lock.
    let granted = field(&call, "model")?;
    let local_model = format!(
        "{}:local",
        granted.strip_suffix(":local").unwrap_or(granted)
    );
    let locality = (|| -> Result<()> {
        let name = field(&call, "model")?.to_ascii_lowercase();
        ensure!(
            !name.ends_with(":cloud") && !name.ends_with("-cloud"),
            "Cloud model is outside local grant"
        );
        let info = ollama(&call, "show", Some(&json!({"model":local_model})))?;
        ensure!(
            ["remote_host", "remote_model"]
                .iter()
                .all(|k| info.get(k).is_none_or(|v| v.as_str() == Some(""))),
            "Remote-backed model is outside local grant"
        );
        ensure!(
            info["details"]["format"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
                && info["model_info"]["general.architecture"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty()),
            "Could not verify local model weights"
        );
        Ok(())
    })();
    let body = {
        let _lock = instances::lock(&db, &task)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let graph = get(&tx, "graph", &task)?;
        call = get(&tx, "model_call", id)?;
        let activity = &graph["activity"][field(&call, "node")?];
        if scheduler::paused(&graph)
            || activity["id"] != id
            || activity["status"] == "cancelled"
            || call["state"] != "reserved"
        {
            return Ok(());
        }
        ensure!(
            call["authority"] == json!(hash(canonical(&permission::stamp(&tx, &task)?).as_bytes())),
            "Expired model authority"
        );
        if let Err(e) = locality {
            release_unsent(&tx, id, &e.to_string())?;
            tx.commit()?;
            drop(_lock);
            drop(lease);
            return wake(&task);
        }
        let msgs = messages(
            &tx,
            field(&call, "input")?,
            call["input_limit"].as_u64().unwrap(),
        )?;
        call["state"] = json!("sending");
        call["sent_at"] = json!(now_ms());
        put(&tx, "model_call", id, &call)?;
        tx.commit()?;
        json!({"model":local_model,"messages":msgs,"stream":false,"think":false,"options":{"temperature":0,"num_predict":call["output_limit"]}})
    };
    let answer = ollama(&call, "chat", Some(&body));
    let lock = instances::lock(&db, &task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    call["settled_at"] = json!(now_ms());
    match answer {
        Ok(r) if r["done"] == true && r["message"]["content"].is_string() => {
            let result = json!({"text":r["message"]["content"]});
            call["output"] = json!(revision(
                &tx,
                &json!({"result.json":STANDARD.encode(serde_json::to_vec(&result)?)})
            )?);
            call["actual_model"] = r["model"].clone();
            call["usage"] =
                json!({"input_tokens":r["prompt_eval_count"],"output_tokens":r["eval_count"]});
            call["state"] = json!("completed");
        }
        Err(e)
            if e.downcast_ref::<HttpStatus>()
                .is_some_and(|s| (400..500).contains(&s.0)) =>
        {
            call["state"] = json!("failed");
            call["error"] = json!(e.to_string());
        }
        _ => {
            call["state"] = json!("unknown");
            call["error"] = json!("No confirmed model response; explicit retry required");
        }
    }
    put(&tx, "model_call", id, &call)?;
    tx.commit()?;
    drop(lease);
    drop(lock);
    wake(&task)?;
    if call["state"] == "completed" {
        let granted_name = local_model.trim_end_matches(":local");
        let mut metadata = call.clone();
        metadata["timeout"] = json!(2);
        if let Ok(models) = ollama(&metadata, "ps", None) {
            if let Some(digest) = models["models"]
                .as_array()
                .and_then(|models| {
                    models
                        .iter()
                        .find(|m| m["name"] == call["actual_model"] || m["name"] == granted_name)
                })
                .and_then(|m| m["digest"].as_str())
            {
                call["observed_digest"] = json!(digest);
                put(&db, "model_call", id, &call)?;
            }
        }
    }
    Ok(())
}

pub(super) fn history(db: &Connection, task: &str) -> Result<Vec<Value>> {
    let mut query = db.prepare("SELECT body FROM objects WHERE kind='model_call' AND json_extract(body,'$.task')=? ORDER BY rowid")?;
    let mut calls = Vec::new();
    for row in query.query_map([task], |r| r.get::<_, String>(0))? {
        calls.push(serde_json::from_str(&row?)?);
    }
    Ok(calls)
}
