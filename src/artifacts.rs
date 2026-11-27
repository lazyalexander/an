use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{fs, io::ErrorKind, os::unix::fs::PermissionsExt, path::Path};
pub fn safe(path: &str) -> Result<&str> {
    ensure!(
        !path.split('/').any(|s| matches!(s, "" | "." | "..")),
        "Expected relative file path"
    );
    Ok(path)
}
pub fn bytes(entry: &Value) -> Result<Vec<u8>> {
    Ok(STANDARD.decode(
        entry
            .as_str()
            .or_else(|| entry["data"].as_str())
            .context("Invalid file entry")?,
    )?)
}
fn prune(root: &Path, folder: &Path, files: &Value) -> Result<()> {
    for e in fs::read_dir(folder)? {
        let e = e?;
        let p = e.path();
        let ty = e.file_type()?;
        let rel = p.strip_prefix(root)?.to_str().context("Invalid filename")?;
        if ty.is_dir() {
            prune(root, &p, files)?;
            if !files
                .as_object()
                .context("Invalid revision")?
                .keys()
                .any(|k| k.starts_with(&format!("{rel}/")))
            {
                fs::remove_dir(p)?;
            }
        } else if !ty.is_file() || files.get(rel).is_none() {
            fs::remove_file(p)?;
        }
    }
    Ok(())
}
pub fn stage(root: &Path, files: &Value, reset: bool) -> Result<()> {
    if reset {
        ensure!(fs::symlink_metadata(root)?.is_dir(), "Invalid mount root");
        prune(root, root, files)?;
    } else {
        fs::create_dir(root)?;
    }
    for (rel, entry) in files.as_object().context("Invalid revision")? {
        let p = root.join(safe(rel)?);
        fs::create_dir_all(p.parent().unwrap())?;
        let data = bytes(entry)?;
        let differs = match fs::metadata(&p) {
            Ok(meta) if meta.len() == data.len() as u64 => fs::read(&p)? != data,
            Ok(_) => true,
            Err(e) if e.kind() == ErrorKind::NotFound => true,
            Err(e) => return Err(e.into()),
        };
        if differs {
            if p.exists() {
                fs::set_permissions(&p, fs::Permissions::from_mode(0o600))?;
            }
            fs::write(&p, data)?;
        }
        if let Some(mode) = entry.get("mode") {
            fs::set_permissions(
                p,
                fs::Permissions::from_mode(mode.as_u64().context("Invalid mode")? as u32 & 0o777),
            )?;
        }
    }
    Ok(())
}
/// Counts raw file bytes before reading or encoding any one file. The guest
/// has stopped before settlement, so its output and work trees are stable.
pub struct OutputBudget {
    remaining: u64,
}

impl OutputBudget {
    pub fn new(bytes: u64) -> Self {
        Self { remaining: bytes }
    }

    fn encode(&mut self, path: &Path) -> Result<String> {
        let size = fs::metadata(path)?.len();
        self.remaining = self
            .remaining
            .checked_sub(size)
            .context("Run output budget exceeded (declared files and editable work)")?;
        Ok(STANDARD.encode(fs::read(path)?))
    }

    pub fn declared(&mut self, root: &Path, names: &[String]) -> Result<Value> {
        let mut files = json!({});
        for name in names {
            let path = root.join(safe(name)?);
            ensure!(
                path.is_file() && path.canonicalize()? == path,
                "Missing or invalid output: {name}"
            );
            files[name] = json!(self.encode(&path)?);
        }
        Ok(files)
    }

    pub fn snapshot(&mut self, root: &Path) -> Result<Value> {
        ensure!(
            fs::symlink_metadata(root)?.is_dir(),
            "Expected generator directory"
        );
        let mut files = json!({});
        self.visit(root, root, &mut files)?;
        Ok(files)
    }

    fn visit(&mut self, root: &Path, folder: &Path, files: &mut Value) -> Result<()> {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_dir() {
                self.visit(root, &path, files)?;
            } else {
                ensure!(ty.is_file(), "Only regular generator files supported");
                let rel = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("Invalid generator filename")?;
                files[rel] = json!({"data": self.encode(&path)?, "mode": entry.metadata()?.permissions().mode() & 0o777});
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editable_work_shares_the_declared_output_budget() -> Result<()> {
        let directory = std::env::temp_dir()
            .canonicalize()?
            .join(crate::store::uid());
        let work = directory.join("work");
        fs::create_dir_all(&work)?;
        fs::write(directory.join("result"), b"12345")?;
        fs::write(work.join("edit"), b"6789")?;
        let mut budget = OutputBudget::new(8);
        budget.declared(&directory, &["result".into()])?;
        assert!(budget
            .snapshot(&work)
            .unwrap_err()
            .to_string()
            .contains("budget exceeded"));
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
