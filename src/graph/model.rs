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

pub(crate) fn is_image(model: &Value) -> bool {
    model["kind"] == "image"
}

const IMAGE_SIZES: [&str; 3] = ["1024x1024", "1536x1024", "1024x1536"];

/// An image request: `request.json` holds `prompt` and an optional `size`;
/// the optional `image` binding holds one PNG (a sketch or control image).
fn image_request(
    db: &Connection,
    request: &str,
    image: Option<&str>,
    limit: u64,
) -> Result<(Value, Option<Vec<u8>>)> {
    let files = get(db, "revision", request)?;
    ensure!(
        files.as_object().is_some_and(|m| m.len() == 1),
        "Expected one request.json"
    );
    let data = artifacts::bytes(&files["request.json"])?;
    ensure!(data.len() <= 16384, "Image prompt exceeds byte limit");
    let request: Value = serde_json::from_slice(&data)?;
    ensure!(
        request
            .as_object()
            .is_some_and(|m| m.keys().all(|k| k == "prompt" || k == "size"))
            && request["prompt"]
                .as_str()
                .is_some_and(|p| !p.trim().is_empty())
            && request
                .get("size")
                .is_none_or(|s| s.as_str().is_some_and(|s| IMAGE_SIZES.contains(&s))),
        "Expected prompt and optional size {IMAGE_SIZES:?}"
    );
    let image = match image {
        None => None,
        Some(revision_id) => {
            let files = get(db, "revision", revision_id)?;
            let files = files.as_object().context("Invalid image revision")?;
            ensure!(files.len() == 1, "Expected one PNG image");
            let (name, entry) = files.iter().next().unwrap();
            let bytes = artifacts::bytes(entry)?;
            ensure!(
                name.ends_with(".png") && bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                "Image input must be a PNG"
            );
            ensure!(
                bytes.len() as u64 <= limit,
                "Image input exceeds byte limit"
            );
            Some(bytes)
        }
    };
    Ok((request, image))
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
        let image = is_image(&grant["model"]);
        // Consume approvals for every bound port together: if a later port
        // still waits, roll back the earlier consumption (as run inputs do).
        tx.execute_batch("SAVEPOINT model_access")?;
        for (port, key) in [("request", "model"), ("image", "image")] {
            let Some(binding) = bound.get(port).filter(|b| !b.is_null()) else {
                continue;
            };
            let from = binding_origin(binding);
            let revision_id = field(binding, "revision")?;
            if let Some(p) = permission::graph_gate(
                &tx,
                task,
                "model",
                json!([from, dest, revision_id, grant["model"], id]),
                json!([id, revision_id, grant["model"]]),
                state["requests"][key].as_str(),
                json!(["model", from, dest]),
                crate::intent::graph_route(&tx, "model", &node["inputs"][port], &dest, binding)?,
            )? {
                let pending = field(&p, "request_id")?.to_owned();
                let row = get(&tx, "request", &pending)?;
                tx.execute_batch("ROLLBACK TO model_access")?;
                put(&tx, "request", &pending, &row)?;
                state["requests"][key] = json!(pending);
                state["status"] = json!("permission_required");
                tx.commit()?;
                return Ok(());
            }
        }
        tx.execute_batch("RELEASE model_access")?;
        let image_input = bound
            .get("image")
            .filter(|b| !b.is_null())
            .map(|b| field(b, "revision").map(str::to_owned))
            .transpose()?;
        if image {
            image_request(
                &tx,
                input,
                image_input.as_deref(),
                cfg.limit("image_output_bytes"),
            )?;
        } else {
            messages(&tx, input, cfg.limit("model_input_bytes"))?;
        }
        let call = json!({
            "task": task, "node": node["id"], "attempt": id, "alias": node["model"],
            "model": grant["model"], "input": input, "input_origin": origin, "output": null,
            "image_input": image_input, "kind": if image { "image" } else { "text" },
            "output_name": if image { "result.png" } else { "result.json" },
            "authority": stamp, "state": "reserved", "reserved_at": now_ms(), "usage": null,
            "port": cfg.limit(if image { "image_port" } else { "ollama_port" }),
            "timeout": cfg.limit(if image { "image_timeout_s" } else { "model_timeout_s" }),
            "input_limit": cfg.limit(if image { "image_output_bytes" } else { "model_input_bytes" }),
            "output_limit": cfg.limit(if image { "image_output_bytes" } else { "model_output_tokens" })
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
            state["status"] = json!("done");
        }
        "unknown" | "failed" => {
            state["status"] = call["state"].clone();
            state["error"] = call["error"].clone();
        }
        "withdrawn" => {
            state["status"] = json!("failed");
            state["error"] = call["error"].clone();
        }
        "sending" => state["status"] = json!("running"),
        "reserved" => {
            state["status"] = json!("running");
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
                crate::runtime::inherited(&mut cmd, &lease);
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
    if call["kind"] == "image" {
        return image_worker(id, lease, db, call);
    }
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

/// Run curl with an empty environment, no proxy, redirects or retries.
/// `args` must not carry secrets: they are visible to other local processes.
fn curl(timeout: u64, max_bytes: u64, url: &str, args: &[String]) -> Result<(u16, Vec<u8>)> {
    let out = Command::new("/usr/bin/curl")
        .env_clear()
        .args([
            "-q",
            "--silent",
            "--write-out",
            "\n%{http_code}",
            "--noproxy",
            "*",
        ])
        .args([
            "--max-time",
            &timeout.to_string(),
            "--max-filesize",
            &max_bytes.to_string(),
        ])
        .args(args)
        .arg(url)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    // curl exit 7: the connection was refused, so nothing was sent.
    if out.status.code() == Some(7) {
        return Err(NotSent(format!("Image backend not reachable at {url}")).into());
    }
    ensure!(
        out.status.success(),
        "Image HTTP call failed ({})",
        out.status
    );
    let split = out
        .stdout
        .iter()
        .rposition(|&b| b == b'\n')
        .context("Missing HTTP status")?;
    let status = std::str::from_utf8(&out.stdout[split + 1..])?.parse::<u16>()?;
    Ok((status, out.stdout[..split].to_vec()))
}

#[derive(Debug)]
struct NotSent(String);
impl std::fmt::Display for NotSent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for NotSent {}

/// A private directory for one call's request files; removed on drop.
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new(id: &str) -> Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let path = root()?.join("model-scratch").join(id);
        let _ = fs::remove_dir_all(&path);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&path)?;
        Ok(Self(path))
    }
    fn write(&self, name: &str, data: &[u8]) -> Result<String> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.0.join(name);
        File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
            .write_all(data)?;
        Ok(path.to_str().context("Invalid scratch path")?.to_owned())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn keychain(service: &str) -> Result<String> {
    let out = Command::new("/usr/bin/security")
        .env_clear()
        .args(["find-generic-password", "-a", "an", "-s", service, "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    ensure!(
        out.status.success(),
        "No Keychain entry {service} for account an"
    );
    let key = String::from_utf8(out.stdout)?.trim().to_owned();
    ensure!(!key.is_empty(), "Empty Keychain entry {service}");
    Ok(key)
}

/// Generate one image. Returns the PNG and reported usage.
fn generate(
    call: &Value,
    prompt: &Value,
    image: Option<&[u8]>,
    scratch: &Scratch,
) -> Result<(Vec<u8>, Value)> {
    let model = &call["model"];
    let timeout = call["timeout"].as_u64().unwrap();
    let limit = call["output_limit"].as_u64().unwrap();
    let size = prompt["size"].as_str().unwrap_or("1024x1024");
    let (status, body) = if model["backend"] == "local" {
        let request = json!({"model":model["model"],"prompt":prompt["prompt"],"size":size,
            "image":image.map(|b| STANDARD.encode(b))});
        let file = scratch.write("request.json", &serde_json::to_vec(&request)?)?;
        let port = call["port"].as_u64().unwrap();
        curl(
            timeout,
            limit,
            &format!("http://127.0.0.1:{port}/generate"),
            &[
                "-H".into(),
                "Content-Type: application/json".into(),
                "--data-binary".into(),
                format!("@{file}"),
            ],
        )?
    } else {
        // The key goes through a 0600 header file, never argv or the registry.
        let key = keychain(field(model, "keychain")?)?;
        let header = scratch.write("auth", format!("Authorization: Bearer {key}").as_bytes())?;
        let prompt_file = scratch.write("prompt.txt", field(prompt, "prompt")?.as_bytes())?;
        let mut args = vec![
            "-H".into(),
            format!("@{header}"),
            "-F".into(),
            format!("model={}", field(model, "model")?),
            "-F".into(),
            format!("prompt=<{prompt_file}"),
            "-F".into(),
            format!("size={size}"),
        ];
        let endpoint = if let Some(bytes) = image {
            let png = scratch.write("image.png", bytes)?;
            args.extend(["-F".into(), format!("image=@{png};type=image/png")]);
            "edits"
        } else {
            "generations"
        };
        curl(
            timeout,
            limit,
            &format!("https://api.openai.com/v1/images/{endpoint}"),
            &args,
        )?
    };
    if !(200..300).contains(&status) {
        return Err(HttpStatus(status).into());
    }
    let reply: Value = serde_json::from_slice(&body)?;
    let encoded = reply["image"]
        .as_str()
        .or_else(|| reply["data"][0]["b64_json"].as_str())
        .context("Image backend returned no image")?;
    let png = STANDARD.decode(encoded)?;
    ensure!(
        png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "Image backend returned a non-PNG image"
    );
    Ok((png, reply.get("usage").cloned().unwrap_or(Value::Null)))
}

fn image_worker(id: &str, lease: File, mut db: Connection, mut call: Value) -> Result<()> {
    let task = field(&call, "task")?.to_owned();
    let (prompt, image) = {
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
        let request = image_request(
            &tx,
            field(&call, "input")?,
            call["image_input"].as_str(),
            call["input_limit"].as_u64().unwrap(),
        )?;
        call["state"] = json!("sending");
        call["sent_at"] = json!(now_ms());
        put(&tx, "model_call", id, &call)?;
        tx.commit()?;
        request
    };
    let scratch = Scratch::new(id)?;
    let answer = generate(&call, &prompt, image.as_deref(), &scratch);
    drop(scratch);
    let lock = instances::lock(&db, &task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    call["settled_at"] = json!(now_ms());
    match answer {
        Ok((png, usage)) => {
            call["output"] = json!(revision(&tx, &json!({"result.png":STANDARD.encode(&png)}))?);
            call["usage"] = usage;
            call["state"] = json!("completed");
        }
        Err(e)
            if e.downcast_ref::<HttpStatus>()
                .is_some_and(|s| (400..500).contains(&s.0)) =>
        {
            call["state"] = json!("failed");
            call["error"] = json!(e.to_string());
        }
        Err(e)
            if e.downcast_ref::<NotSent>().is_some()
                || e.to_string().starts_with("No Keychain") =>
        {
            call["state"] = json!("failed");
            call["error"] = json!(e.to_string());
        }
        _ => {
            call["state"] = json!("unknown");
            call["error"] = json!("No confirmed image response; explicit retry required");
        }
    }
    put(&tx, "model_call", id, &call)?;
    tx.commit()?;
    drop(lease);
    drop(lock);
    wake(&task)
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn image_requests_need_a_prompt_a_known_size_and_a_png() {
        let dir = std::env::temp_dir().join(uid());
        std::env::set_var("AN_HOME", &dir);
        let db = connect().unwrap();
        let file =
            |name: &str, data: &[u8]| revision(&db, &json!({name: STANDARD.encode(data)})).unwrap();
        let request = |v: Value| file("request.json", v.to_string().as_bytes());
        let png = file("control.png", b"\x89PNG\r\n\x1a\nrest");
        let ok = request(json!({"prompt":"a cottage","size":"1024x1536"}));
        let (parsed, image) = image_request(&db, &ok, Some(&png), 1 << 20).unwrap();
        assert_eq!(parsed["prompt"], "a cottage");
        assert!(image.unwrap().starts_with(b"\x89PNG"));
        for bad in [
            json!({"prompt":"  "}),
            json!({"prompt":"x","size":"7x7"}),
            json!({"prompt":"x","seed":1}),
        ] {
            assert!(image_request(&db, &request(bad), None, 1 << 20).is_err());
        }
        let jpeg = file("control.png", b"\xff\xd8\xff not a png");
        assert!(image_request(&db, &ok, Some(&jpeg), 1 << 20).is_err());
        assert!(
            image_request(&db, &ok, Some(&png), 4).is_err(),
            "byte limit"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn http_errors_name_rejected_credentials() {
        assert!(HttpStatus(401).to_string().contains("credentials"));
        assert!(HttpStatus(500).to_string().contains("500"));
    }
}
