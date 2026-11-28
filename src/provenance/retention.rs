//! Provenance roots retained when a closed task is collected.
use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::Value;

pub(crate) fn visit_materials(db: &Connection, mut visit: impl FnMut(&Value)) -> Result<bool> {
    let managed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='pv_version')",
        [],
        |r| r.get(0),
    )?;
    if managed {
        let mut versions =
            db.prepare("SELECT material FROM pv_version WHERE material IS NOT NULL")?;
        for material in versions.query_map([], |r| r.get::<_, String>(0))? {
            visit(&serde_json::from_str(&material?)?);
        }
    }
    Ok(managed)
}

pub(crate) fn keeps_evidence(db: &Connection, id: &str) -> Result<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM pv_version,json_each(evidence) e WHERE e.value=?)
                    OR EXISTS(SELECT 1 FROM objects g JOIN json_each(g.body,'$.rights') r
                    JOIN objects t ON t.kind='turn' AND t.id=json_extract(g.body,'$.target') AND t.body=json_quote(json_extract(g.body,'$.epoch'))
                    WHERE g.kind='provenance_grant' AND json_extract(g.body,'$.active') AND json_extract(r.value,'$.snapshot')=?)",
                    params![id,id],|r|r.get(0))?)
}
