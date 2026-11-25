//! Task-bound local channel for an admitted native document editor.
use super::*;
use anyhow::bail;
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Cursor, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(clap::Subcommand)]
pub enum Action {
    /// Serve one active task to the local Krita plugin. Prints its connection file.
    Serve {
        #[arg(long)]
        task: String,
    },
}

pub(super) fn document(files: &Value) -> Result<(&str, &Value)> {
    let map = files
        .as_object()
        .context("Invalid native document revision")?;
    ensure!(
        map.len() == 1,
        "Native surface requires exactly one document"
    );
    let (name, entry) = map.iter().next().unwrap();
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    ensure!(
        matches!(ext, "kra" | "ora"),
        "Krita input must be ORA or KRA"
    );
    Ok((ext, entry))
}

fn private_dir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
        "Expected private adapter directory"
    );
    Ok(())
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let temp = path.with_extension(uid());
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    f.write_all(data)?;
    f.sync_all()?;
    fs::rename(temp, path)?;
    Ok(())
}

pub fn execute(action: Action) -> Result<()> {
    let Action::Serve { task } = action;
    permission::current(&connect()?, &task)?;
    wake(&task)?;
    let identity = hash(canonical(&json!([root()?, task])).as_bytes());
    let dir = PathBuf::from("/tmp").join(format!(
        "an-krita-{}-{}",
        unsafe { libc::geteuid() },
        &identity[..20]
    ));
    private_dir(&dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("server.lock"))?;
    lock.try_lock().context("Native channel already running")?;
    let path = dir.join("adapter.sock");
    if path.exists() {
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let descriptor = dir.join("connect.json");
    atomic_write(
        &descriptor,
        &serde_json::to_vec(&json!({"adapter":"krita","task":task,"socket":path}))?,
    )?;
    println!("{}", descriptor.display());
    std::io::stdout().flush()?;
    for connection in listener.incoming() {
        let mut stream = connection?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut line = String::new();
        let response = (|| -> Result<Value> {
            BufReader::new((&stream).take(8193)).read_line(&mut line)?;
            ensure!(
                line.len() <= 8192 && line.ends_with('\n'),
                "Invalid adapter request"
            );
            handle(&task, &serde_json::from_str(&line)?)
        })();
        let body = match response {
            Ok(v) => v,
            Err(e) => json!({"error":format!("{e:#}")}),
        };
        let _ = writeln!(stream, "{body}");
        drop(stream);
        if body["status"] == "done" {
            if let Err(error) = wake(&task) {
                eprintln!(
                    "Native receipt committed for task {task}; scheduler wake failed: {error:#}"
                );
            }
        }
    }
    Ok(())
}

fn verify_document_xml(data: &[u8]) -> Result<()> {
    use quick_xml::{events::Event, Reader};
    let mut reader = Reader::from_reader(data);
    let mut depth = 0usize;
    let mut root = false;
    let mut image = false;
    loop {
        match reader.read_event()? {
            Event::Start(tag) => {
                if depth == 0 {
                    ensure!(
                        !root && tag.name().as_ref() == b"DOC",
                        "Invalid KRA document root"
                    );
                    root = true;
                }
                if depth == 1 && tag.name().as_ref() == b"IMAGE" {
                    image = true;
                }
                depth += 1;
                ensure!(depth <= 256, "KRA metadata nesting limit exceeded");
            }
            Event::End(_) => depth = depth.checked_sub(1).context("Invalid KRA document")?,
            Event::Eof => break,
            _ => {}
        }
    }
    ensure!(root && image && depth == 0, "Missing Krita image");
    Ok(())
}

fn verify_snapshot(bytes: &[u8]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("Invalid KRA archive")?;
    ensure!(archive.len() <= 32768, "Too many KRA members");
    let mut mime = false;
    let mut document = false;
    let mut total = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let name = file.name()?.into_owned();
        let size = file.size();
        total = total.checked_add(size).context("KRA size overflow")?;
        ensure!(
            total <= 512 * 1024 * 1024,
            "KRA expands beyond the document limit"
        );
        let mut reader = file.take(size + 1);
        if name == "mimetype" || name == "maindoc.xml" {
            ensure!(size <= 8 * 1024 * 1024, "Invalid KRA header");
            let mut data = Vec::new();
            reader.read_to_end(&mut data)?;
            ensure!(data.len() as u64 == size, "Truncated KRA header");
            if name == "mimetype" {
                ensure!(
                    !mime && data == b"application/x-krita",
                    "Invalid KRA MIME type"
                );
                mime = true;
            } else {
                ensure!(!document, "Duplicate KRA document header");
                verify_document_xml(&data)?;
                document = true;
            }
        } else {
            ensure!(
                std::io::copy(&mut reader, &mut std::io::sink())? == size,
                "Truncated KRA member"
            );
        }
    }
    ensure!(mime && document, "Missing KRA document headers");
    Ok(())
}

fn read_snapshot(dir: &Path, digest: &str) -> Result<Vec<u8>> {
    let limit = crate::config::load()?.limit("transfer_limit_bytes");
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("submitted.kra"))?;
    ensure!(file.metadata()?.is_file(), "Expected regular KRA snapshot");
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit && hash(&bytes) == digest,
        "Snapshot size or digest mismatch"
    );
    verify_snapshot(&bytes)?;
    Ok(bytes)
}

fn handle(task: &str, args: &Value) -> Result<Value> {
    let mut db = connect()?;
    let _lock = instances::lock(&db, task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let op = field(args, "op")?;
    permission::current(&tx, task)?;
    let Some(mut graph) = find(&tx, "graph", task)? else {
        ensure!(op == "poll", "No graph for native request");
        return Ok(json!({"status":"idle"}));
    };
    let nodes = graph["nodes"].as_object().context("Invalid graph")?;
    let id = if op == "poll" {
        let mut waiting = nodes.iter().filter(|(id, node)| {
            node["kind"] == "native" && graph["activity"][id]["status"] == "waiting"
        });
        let preferred = waiting
            .clone()
            .find(|(id, _)| graph["activity"][id]["id"] == args["request"]);
        let Some((id, _)) = preferred.or_else(|| waiting.next()) else {
            return Ok(json!({"status":"idle"}));
        };
        id.clone()
    } else {
        field(args, "node")?.to_owned()
    };
    ensure!(
        nodes.get(&id).is_some_and(|n| n["kind"] == "native"),
        "Unknown native surface"
    );
    let mut state = graph["activity"][&id].clone();
    let attempt = field(&state, "id")?.to_owned();
    let input = field(&state["bindings"]["document"], "revision")?.to_owned();
    if op != "poll" {
        ensure!(
            args["request"] == attempt && args["input"] == input,
            "Stale native request"
        );
    }
    let stamp = permission::stamp(&tx, task)?;
    ensure!(
        state["authority"] == hash(canonical(&stamp).as_bytes()),
        "Native authority expired"
    );
    if op == "receipt" && state["status"] == "done" && state["receipt"]["digest"] == args["digest"]
    {
        return Ok(json!({"status":"done","revision":state["outputs"]["document"]["revision"]}));
    }
    ensure!(
        state["status"] == "waiting",
        "Native request closed or expired"
    );
    let dir = root()?.join("native-documents").join(&attempt);
    let result = match op {
        "poll" => {
            private_dir(&dir)?;
            let cached = state["native_extension"]
                .as_str()
                .filter(|ext| matches!(*ext, "kra" | "ora"))
                .map(|ext| dir.join(format!("input.{ext}")))
                .filter(|source| source.is_file());
            let source = if let Some(source) = cached {
                source
            } else {
                let files = get(&tx, "revision", &input)?;
                let (ext, entry) = document(&files)?;
                let source = dir.join(format!("input.{ext}"));
                if !source.is_file() {
                    atomic_write(&source, &artifacts::bytes(entry)?)?;
                }
                if state["native_extension"] != ext {
                    state["native_extension"] = json!(ext);
                    graph["activity"][&id] = state.clone();
                    put(&tx, "graph", task, &graph)?;
                }
                source
            };
            json!({"status":"waiting","task":task,"node":id,"request":attempt,"input":input,"source":source,"directory":dir,"error_detail":state["native_error"]})
        }
        "opened" | "failed" => {
            if op == "opened" {
                state["native_opened"] = json!(true);
            }
            state["native_phase"] = json!(if op == "opened" {
                "editing"
            } else {
                "save_failed"
            });
            state["native_error"] = if op == "failed" {
                json!(field(args, "detail")?.chars().take(512).collect::<String>())
            } else {
                Value::Null
            };
            json!({"status":"waiting"})
        }
        "receipt" => {
            ensure!(
                state["native_opened"] == true,
                "Document has not been opened"
            );
            let digest = field(args, "digest")?;
            let bytes = match read_snapshot(&dir, digest) {
                Ok(bytes) => bytes,
                Err(error) => {
                    state["native_phase"] = json!("save_failed");
                    state["native_error"] = json!(error.to_string());
                    graph["activity"][&id] = state;
                    put(&tx, "graph", task, &graph)?;
                    tx.commit()?;
                    return Err(error);
                }
            };
            let revision = revision(&tx, &json!({"painting.kra":STANDARD.encode(bytes)}))?;
            state["outputs"]["document"] = json!({"revision":revision,"spool":"native:krita"});
            state["status"] = json!("done");
            state["native_phase"] = json!("saved");
            state.as_object_mut().unwrap().remove("native_error");
            let receipt = json!({"adapter":"krita","request":attempt,"input":input,"digest":digest,"revision":revision});
            state["receipt"] = receipt.clone();
            event(&mut graph, "user", "saved", &id, receipt);
            json!({"status":"done","revision":revision})
        }
        _ => bail!("Unsupported native adapter operation"),
    };
    if op != "poll" {
        graph["activity"][&id] = state;
        put(&tx, "graph", task, &graph)?;
    }
    tx.commit()?;
    Ok(result)
}
