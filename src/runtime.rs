use crate::{manifest, store::*};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
pub const CONTAINER: &str = "/usr/local/bin/container";
pub fn inherited(command: &mut Command, file: &File) {
    let fd = file.as_raw_fd();
    // Only the child needs the lock across exec; keep CLOEXEC in the parent.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}
pub fn apple(args: &[&str], lock: Option<&File>) -> Result<Vec<u8>> {
    let mut cmd = Command::new(CONTAINER);
    cmd.args(args);
    if let Some(f) = lock {
        inherited(&mut cmd, f);
    }
    let out = cmd.output()?;
    ensure!(
        out.status.success(),
        "container {}: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}
pub fn inspect(args: &[&str], lock: Option<&File>) -> Result<Value> {
    let rows: Vec<Value> = serde_json::from_slice(&apple(args, lock)?)?;
    rows.into_iter()
        .next()
        .context("Missing container inspection")
}
pub fn list(lock: &File) -> Result<Vec<Value>> {
    Ok(serde_json::from_slice(&apple(
        &["list", "--all", "--format", "json"],
        Some(lock),
    )?)?)
}
pub fn start(
    image: &str,
    id: &str,
    label: &str,
    path: &Path,
    editable: bool,
    platform: &Value,
    lock: &File,
) -> Result<()> {
    let p = path.to_str().context("Invalid mount path")?;
    ensure!(!p.contains(','), "Unsupported comma in mount path");
    let plat = ["os", "architecture", "variant"]
        .iter()
        .filter_map(|k| platform[*k].as_str())
        .collect::<Vec<_>>()
        .join("/");
    let mut args = vec![
        "run",
        "--detach",
        "--platform",
        &plat,
        "--name",
        id,
        "--label",
        label,
        "--network",
        "none",
        "--read-only",
        "-w",
        "/work",
        "--entrypoint",
        "/bin/sleep",
    ];
    let mounts = ["input", "output", "work"].map(|d| {
        format!(
            "type=bind,source={p}/{d},target=/{d}{}",
            if d == "input" || (d == "work" && !editable) {
                ",readonly"
            } else {
                ""
            }
        )
    });
    for m in &mounts {
        args.extend(["--mount", m]);
    }
    args.extend([image, "infinity"]);
    apple(&args, Some(lock))?;
    Ok(())
}
fn digest(image: &str) -> Result<&str> {
    let (_, d) = image.rsplit_once('@').context("Expected pinned image")?;
    ensure!(
        d.len() == 71
            && d.starts_with("sha256:")
            && d[7..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid image digest"
    );
    Ok(d)
}
fn variant(image: &str) -> Result<Value> {
    let i = inspect(&["image", "inspect", image], None)?;
    ensure!(
        i["configuration"]["descriptor"]["digest"] == digest(image)?,
        "Local image digest mismatch"
    );
    let mut v = i["variants"]
        .as_array()
        .context("Missing variants")?
        .iter()
        .filter(|v| v["platform"]["os"] == "linux" && v["platform"]["architecture"] == "arm64");
    let hit = v.next().context("Missing linux/arm64 variant")?;
    ensure!(v.next().is_none(), "Ambiguous linux/arm64 variant");
    Ok(json!({"variant":hit["digest"],"platform":hit["platform"]}))
}
fn record(db: &Connection, image: &str) -> Result<Option<Value>> {
    let Some(d) = image.rsplit('@').next() else {
        return Ok(None);
    };
    let Some(mut v) = find(db, "admission", d)? else {
        return Ok(None);
    };
    if v["spec"] != "spool/v2" || !v["manifest"].is_object() || !v["manifest_digest"].is_string() {
        return Ok(None);
    }
    if let Some(panels) = v["manifest"].get("panels") {
        crate::panels::validate(panels)?;
    }
    v["image"] = json!(image);
    Ok(Some(v))
}
pub fn require(db: &Connection, image: &str) -> Result<Value> {
    let msg = format!("needs_admission: run an register {image}");
    let v = record(db, image)?.context(msg.clone())?;
    let info = variant(image)?;
    ensure!(
        v["variant"] == info["variant"] && v["platform"] == info["platform"],
        "{msg}"
    );
    Ok(v)
}
pub fn summary(db: &Connection, image: &str) -> Result<Value> {
    let Some(mut v) = record(db, image)? else {
        return Ok(json!({"status":"needs_admission"}));
    };
    v["name"] = v["manifest"]["name"].clone();
    v["summary"] = v["manifest"]["summary"].clone();
    let m = v.as_object_mut().unwrap();
    m.remove("manifest");
    m.remove("skill");
    v["status"] = json!("admitted");
    Ok(v)
}
fn layout(reader: impl Read) -> Result<Value> {
    let mut archive = tar::Archive::new(reader);
    let mut paths = HashMap::new();
    let mut total = 0usize;
    let mut result = json!({});
    let mut code = json!({});
    let mut omissions = Vec::new();
    let mut code_bytes = 0;
    for entry in archive.entries()? {
        let mut e = entry?;
        let bytes = e.path_bytes();
        ensure!(bytes.len() <= 4096, "Export path exceeds limit");
        let path = PathBuf::from_iter(
            e.path()?
                .components()
                .filter(|c| *c != std::path::Component::CurDir),
        );
        if !path.starts_with("spool") {
            continue;
        }
        total += bytes.len();
        ensure!(
            paths.len() < 16384 && total <= 4 * 1024 * 1024,
            "Spool layout exceeds budget"
        );
        ensure!(
            !path
                .components()
                .any(|p| p == std::path::Component::ParentDir),
            "Ambiguous spool path"
        );
        let ty = e.header().entry_type();
        let size = e.size();
        ensure!(
            paths.insert(path.clone(), (ty, size)).is_none(),
            "Duplicate spool path"
        );
        if ty.is_file() && path.starts_with("spool/code") {
            let Some(name) = path.to_str() else {
                omissions.push(path.to_string_lossy().into_owned());
                continue;
            };
            // Admission still accepts large binaries. Their immutable image
            // digest remains available when a source preview is omitted.
            if size <= 1024 * 1024 && code_bytes + size <= 8 * 1024 * 1024 {
                let mut bytes = Vec::new();
                e.read_to_end(&mut bytes)?;
                code[name] = json!(STANDARD.encode(bytes));
                code_bytes += size;
            } else {
                omissions.push(name.to_owned());
            }
        }
        if path == Path::new("spool/SKILL.md") {
            ensure!(
                ty.is_file() && size > 0 && size <= 262144,
                "Invalid SKILL.md"
            );
            let mut s = String::new();
            e.read_to_string(&mut s)?;
            ensure!(!s.trim().is_empty(), "Empty SKILL.md");
            result["skill_digest"] = json!(hash(s.as_bytes()));
            result["skill"] = json!(s);
        }
        if path == Path::new("spool/spool.yaml") {
            ensure!(
                ty.is_file() && size <= manifest::LIMIT,
                "Invalid spool.yaml"
            );
            let mut buf = vec![];
            e.read_to_end(&mut buf)?;
            result["manifest"] = manifest::parse(&buf)?;
            result["manifest_digest"] = json!(hash(&buf));
        }
    }
    ensure!(
        result["manifest"].is_object(),
        "Missing /spool/spool.yaml: add the manifest and rebuild the image"
    );
    ensure!(
        result["skill"].is_string()
            && paths
                .iter()
                .any(|(p, (ty, size))| p.starts_with("spool/code")
                    && p.components().count() > 2
                    && ty.is_file()
                    && *size > 0
                    && p.ancestors()
                        .skip(1)
                        .filter(|p| !p.as_os_str().is_empty())
                        .all(|p| paths.get(p).is_some_and(|(t, _)| t.is_dir()))),
        "Invalid spool layout"
    );
    for (id, panel) in result["manifest"]["panels"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if let Some(path) = panel["html"].as_str() {
            let file = Path::new(path);
            ensure!(
                file.ancestors()
                    .skip(1)
                    .filter(|p| !p.as_os_str().is_empty())
                    .all(|p| paths.get(p).is_some_and(|(ty, _)| ty.is_dir())),
                "Invalid panel directory: {id}"
            );
            let bytes = crate::artifacts::bytes(&code[path])
                .with_context(|| format!("Missing or oversized panel HTML: {id}"))?;
            ensure!(
                !std::str::from_utf8(&bytes)?.trim().is_empty(),
                "Empty panel HTML: {id}"
            );
        }
    }
    result["code_files"] = code;
    result["code_omissions"] = json!(omissions);
    Ok(result)
}
pub fn register(db: &Connection, reference: &str) -> Result<String> {
    ensure!(
        !reference.is_empty() && !reference.starts_with('-'),
        "Invalid image reference"
    );
    let cfg = inspect(&["image", "inspect", reference], None)?["configuration"].clone();
    let name = cfg["name"]
        .as_str()
        .context("Missing name")?
        .split('@')
        .next()
        .unwrap();
    let (repo, last) = name
        .rsplit_once('/')
        .map(|(r, l)| (format!("{r}/"), l))
        .unwrap_or((String::new(), name));
    let image = format!(
        "{repo}{}@{}",
        last.split(':').next().unwrap(),
        cfg["descriptor"]["digest"]
            .as_str()
            .context("Missing digest")?
    );
    let d = digest(&image)?;
    if reference.contains('@') {
        ensure!(digest(reference)? == d, "Requested digest mismatch");
    }
    if cfg["name"] != image {
        apple(&["image", "tag", reference, &image], None)?;
    }
    let info = variant(&image)?;
    let key = &d[7..];
    let dir = root()?.join("probes");
    fs::create_dir_all(&dir)?;
    let lock = File::options()
        .create(true)
        .append(true)
        .open(dir.join(format!("{key}.lock")))?;
    lock.lock()?;
    let cleanup = |id: &str| -> Result<()> {
        if let Some(hit) = list(&lock)?.into_iter().find(|v| v["id"] == id) {
            ensure!(
                hit["configuration"]["labels"]["an.probe"] == id,
                "Probe identity mismatch"
            );
            apple(&["delete", "--force", id], Some(&lock))?;
            ensure!(
                !list(&lock)?.iter().any(|v| v["id"] == id),
                "Probe still present"
            );
        }
        let path = dir.join(id);
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
        db.execute("DELETE FROM objects WHERE kind='probe' AND id=?", [key])?;
        Ok(())
    };
    if let Some(old) = find(db, "probe", key)? {
        cleanup(old["name"].as_str().context("Missing probe name")?)?;
    }
    if record(db, &image)?.is_some_and(|v| {
        v["platform"] == info["platform"]
            && v["variant"] == info["variant"]
            && v["code_revision"].is_string()
    }) {
        put(db, "image", &image, &json!(image))?;
        return Ok(image);
    }
    let id = format!("an-probe-{}", uid());
    let path = dir.join(&id);
    let phase = |stage: &str| put(db, "probe", key, &json!({"stage":stage,"name":id}));
    phase("startup")?;
    let checked = (|| -> Result<Value> {
        for d in ["input", "output", "work"] {
            fs::create_dir_all(path.join(d))?;
        }
        start(
            &image,
            &id,
            &format!("an.probe={id}"),
            &path,
            true,
            &info["platform"],
            &lock,
        )?;
        let i = inspect(&["inspect", &id], Some(&lock))?;
        let cfg = &i["configuration"];
        ensure!(
            i["status"]["state"] == "running"
                && cfg["platform"] == info["platform"]
                && cfg["image"]["descriptor"]["digest"] == d
                && cfg["readOnly"] == true
                && cfg["networks"].as_array().is_some_and(Vec::is_empty),
            "Probe platform or readiness mismatch"
        );
        let mounts = cfg["mounts"].as_array().context("Missing mounts")?;
        ensure!(mounts.len() == 3, "Unexpected probe mounts");
        for d in ["input", "output", "work"] {
            let m = mounts
                .iter()
                .find(|m| m["destination"] == format!("/{d}"))
                .context("Missing mount")?;
            ensure!(
                m["source"] == path.join(d).to_string_lossy().as_ref()
                    && (m["options"]
                        .as_array()
                        .context("Invalid mount")?
                        .contains(&json!("ro"))
                        == (d == "input")),
                "Probe mount mismatch"
            );
        }
        phase("layout")?;
        apple(&["stop", &id], Some(&lock))?;
        let mut cmd = Command::new(CONTAINER);
        cmd.args(["export", &id]).stdout(Stdio::piped());
        inherited(&mut cmd, &lock);
        let mut child = cmd.spawn()?;
        let result = layout(child.stdout.take().unwrap());
        let exited = child.wait()?;
        let value = result?;
        ensure!(exited.success(), "Image export failed");
        Ok(value)
    })();
    if let Err(e) = &checked {
        put(
            db,
            "admission_failure",
            &image,
            &json!({"probe":id,"stage":get(db,"probe",key)?["stage"],"detail":e.to_string()}),
        )?;
    }
    phase("cleanup")?;
    cleanup(&id)?;
    let mut record = checked?;
    let code = record
        .as_object_mut()
        .unwrap()
        .remove("code_files")
        .context("Missing code snapshot")?;
    record["code_revision"] = json!(revision(db, &code)?);
    record["spec"] = json!("spool/v2");
    record["image"] = json!(image);
    record["platform"] = info["platform"].clone();
    record["variant"] = info["variant"].clone();
    put(db, "admission", d, &record)?;
    put(db, "image", &image, &json!(image))?;
    Ok(image)
}

#[cfg(test)]
mod cached_admission_tests {
    use super::*;

    #[test]
    fn cached_panel_requires_adapter_in_current_build() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE objects(kind TEXT,id TEXT,body TEXT,PRIMARY KEY(kind,id))")
            .unwrap();
        let admission = json!({"spec":"spool/v2","manifest_digest":"digest","manifest":{
            "panels":{"paint":{"kind":"native","title":"Paint","bindings":["document"],"adapter":"krita"}}
        }});
        put(&db, "admission", "digest", &admission).unwrap();
        assert_eq!(
            record(&db, "spool@digest").is_ok(),
            cfg!(feature = "krita-experiment")
        );
    }
}
