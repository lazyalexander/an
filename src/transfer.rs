//! Per-call transfer area for operator-registered local MCP bridges.
use crate::{artifacts, store::*};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{ErrorKind, Read},
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
};

pub struct Transfer {
    path: PathBuf,
    output: File,
    _lease: File,
}
impl Transfer {
    fn directory() -> Result<(PathBuf, File)> {
        let home = root()?;
        let guard = File::options()
            .create(true)
            .append(true)
            .open(home.join("transfers.lock"))?;
        guard.lock()?;
        let dir = home.join("transfers");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        Ok((dir, guard))
    }
    pub fn recover() -> Result<()> {
        let (dir, _guard) = Self::directory()?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir()
                || uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_err()
            {
                continue;
            }
            match File::open(entry.path().join("lease")) {
                Ok(lease) => match lease.try_lock() {
                    Ok(()) => fs::remove_dir_all(entry.path())?,
                    Err(std::fs::TryLockError::WouldBlock) => continue,
                    Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
                },
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    // A previous process may have exited between mkdir and opening the lease.
                    if entry.path().exists() {
                        fs::remove_dir_all(entry.path())?;
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    pub fn new(id: &str) -> Result<Self> {
        let (dir, _guard) = Self::directory()?;
        let path = dir.join(id);
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        let lease = File::create_new(path.join("lease"))?;
        lease.lock()?;
        let output = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path.join("result"))?;
        Ok(Self {
            path,
            output,
            _lease: lease,
        })
    }
    pub fn prepare(&self, db: &Connection, revision: Option<&str>) -> Result<Value> {
        let files = match revision {
            Some(r) => get(db, "revision", r)?,
            None => json!({}),
        };
        artifacts::stage(&self.path.join("input"), &files, false)?;
        Ok(json!({"input":self.path.join("input"),"output":self.path.join("result")}))
    }
    pub fn finish(&self, db: &Connection, mut provenance: Value) -> Result<Value> {
        // Read only the inode created by An. Never resolve a bridge-supplied path.
        let limit = crate::config::load()?.limit("transfer_limit_bytes");
        let mut bytes = Vec::new();
        (&self.output).take(limit + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= limit,
            "Bridge output exceeds {limit} bytes"
        );
        if bytes.is_empty() {
            return Ok(json!({"status":"completed"}));
        }
        let rev = revision(db, &json!({"data":STANDARD.encode(&bytes)}))?;
        let id = format!("artifact-{}", uid());
        provenance["revision"] = json!(rev);
        provenance["files"] = json!([{"name":"data","bytes":bytes.len(),"sha256":hash(&bytes)}]);
        put(db, "artifact", &id, &provenance)?;
        Ok(
            json!({"artifact":id,"revision":rev,"file":"data","bytes":bytes.len(),"sha256":hash(&bytes)}),
        )
    }
}
impl Drop for Transfer {
    fn drop(&mut self) {
        if let Ok((_dir, _guard)) = Self::directory() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
pub fn source(db: &Connection, task: &str, args: &Value) -> Result<Option<(String, String)>> {
    let Some(value) = args.get("an_source") else {
        return Ok(None);
    };
    let r = get(
        db,
        "run",
        value.as_str().context("Expected candidate run ID")?,
    )?;
    ensure!(
        r["task"] == task && r["status"] == "awaiting_publication",
        "Expected private candidate in this task"
    );
    Ok(Some((
        r["candidate"].as_str().context("Missing revision")?.into(),
        r["spool"].as_str().context("Missing sender")?.into(),
    )))
}
