use anyhow::{anyhow, ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{env, fs, os::unix::fs::DirBuilderExt, path::PathBuf};

pub fn root() -> Result<PathBuf> {
    let path = env::var_os("AN_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|v| PathBuf::from(v).join(".an")))
        .ok_or_else(|| anyhow!("Expected AN_HOME or HOME"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&path)?;
    Ok(path.canonicalize()?)
}
pub fn connect() -> Result<Connection> {
    let db = Connection::open(root()?.join("registry.sqlite3"))?;
    db.busy_timeout(crate::config::load()?.seconds("sqlite_busy_s"))?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS objects (kind, id, body, PRIMARY KEY(kind,id))")?;
    Ok(db)
}
pub fn uid() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
pub fn find(db: &Connection, kind: &str, id: &str) -> Result<Option<Value>> {
    let data: Option<String> = db
        .query_row(
            "SELECT body FROM objects WHERE kind=? AND id=?",
            params![kind, id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(data.map(|s| serde_json::from_str(&s)).transpose()?)
}
pub fn get(db: &Connection, kind: &str, id: &str) -> Result<Value> {
    find(db, kind, id)?.ok_or_else(|| anyhow!("Unknown {kind}: {id}"))
}
pub fn put(db: &Connection, kind: &str, id: &str, data: &Value) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO objects VALUES(?,?,?)",
        params![kind, id, data.to_string()],
    )?;
    Ok(())
}
// Existing revision IDs and binding keys use Python's sorted, ASCII JSON encoding.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::String(s) => {
            let escaped = serde_json::to_string(s).unwrap();
            let mut out = String::new();
            for c in escaped.chars() {
                if c as u32 >= 127 {
                    for unit in c.encode_utf16(&mut [0; 2]) {
                        out.push_str(&format!("\\u{unit:04x}"));
                    }
                } else {
                    out.push(c)
                }
            }
            out
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(o) => {
            let mut keys = o.keys().collect::<Vec<_>>();
            keys.sort();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|k| format!("{}: {}", canonical(&json!(k)), canonical(&o[k])))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::Number(n) if n.is_f64() => {
            let formatted = format!("{:?}", n.as_f64().unwrap());
            if let Some((mantissa, exponent)) = formatted.split_once('e') {
                let exponent = exponent.parse::<i32>().unwrap();
                format!("{mantissa}e{exponent:+03}")
            } else {
                formatted
            }
        }
        _ => v.to_string(),
    }
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn revision(db: &Connection, files: &Value) -> Result<String> {
    let id = hash(canonical(files).as_bytes());
    put(db, "revision", &id, files)?;
    Ok(id)
}
pub fn inputs(db: &Connection, paths: &[PathBuf]) -> Result<Value> {
    let mut result = json!({});
    for path in paths {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("Invalid input name"))?;
        ensure!(result.get(name).is_none(), "Duplicate input name");
        result[name] = json!(revision(
            db,
            &json!({(name):STANDARD.encode(fs::read(path)?)})
        )?);
    }
    Ok(result)
}
pub fn deliveries(db: &Connection, task: &str, after: i64) -> Result<Value> {
    let t = get(db, "task", task)?;
    let wid = t["workspace"]
        .as_str()
        .ok_or_else(|| anyhow!("Task has no workspace"))?;
    let mut stmt=db.prepare("SELECT r.rowid,r.id,r.body FROM objects r JOIN objects t ON t.kind='task' AND t.id=json_extract(r.body,'$.task') WHERE json_extract(t.body,'$.workspace')=? AND ((r.kind='run' AND json_extract(r.body,'$.status')='delivered' AND json_array_length(r.body,'$.outputs')>0) OR r.kind='graph_delivery') AND r.rowid>? ORDER BY r.rowid LIMIT 51")?;
    let mut rows = stmt.query(params![wid, after])?;
    let mut items = vec![];
    let mut last = 0;
    let mut next = None;
    while let Some(row) = rows.next()? {
        if items.len() == 50 {
            next = Some(last);
            break;
        }
        last = row.get::<_, i64>(0)?;
        let id: String = row.get(1)?;
        let body: String = row.get(2)?;
        let r: Value = serde_json::from_str(&body)?;
        if r["node"].is_string() && r["bindings"].is_object() {
            items.push(json!({"id":id,"node":r["node"],"files":r["outputs"],"revision":r["revision"],"package":r["package"]}));
        } else {
            items.push(
                json!({"id":id,"spool":r["spool"],"files":r["outputs"],"revision":r["output"]}),
            );
        }
    }
    Ok(json!({"items":items,"next":next}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_previous_binding_and_grant_key_encoding() {
        let value = json!([{"中文":"😀", "n":0.00001},1.0,1e20]);
        assert_eq!(
            canonical(&value),
            r#"[{"n": 1e-05, "\u4e2d\u6587": "\ud83d\ude00"}, 1.0, 1e+20]"#
        );
    }
}
