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
        let differs = match fs::read(&p) {
            Ok(old) => old != data,
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
pub fn snapshot(root: &Path) -> Result<Value> {
    fn visit(root: &Path, p: &Path, files: &mut Value) -> Result<()> {
        for e in fs::read_dir(p)? {
            let e = e?;
            let path = e.path();
            let ty = e.file_type()?;
            if ty.is_dir() {
                visit(root, &path, files)?;
            } else {
                ensure!(ty.is_file(), "Only regular generator files supported");
                let rel = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("Invalid generator filename")?;
                files[rel] = json!({"data":STANDARD.encode(fs::read(&path)?),"mode":fs::metadata(&path)?.permissions().mode()&0o777});
            }
        }
        Ok(())
    }
    ensure!(
        fs::symlink_metadata(root)?.is_dir(),
        "Expected generator directory"
    );
    let mut files = json!({});
    visit(root, root, &mut files)?;
    Ok(files)
}
