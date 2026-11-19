//! Exercise the shipped binary against the historical objects table.
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
struct Fixture {
    root: PathBuf,
    db: Connection,
    turn: Option<String>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        let db = Connection::open(root.join("registry.sqlite3")).unwrap();
        db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")
            .unwrap();
        Self {
            root,
            db,
            turn: None,
        }
    }
    fn insert(&self, kind: &str, id: &str, body: Value) {
        self.db
            .execute(
                "INSERT OR REPLACE INTO objects VALUES(?,?,?)",
                params![kind, id, body.to_string()],
            )
            .unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_an"));
        cmd.env("AN_HOME", &self.root)
            .env("AN_PYTHON", "/missing")
            .env_remove("AN_TURN");
        if let Some(v) = &self.turn {
            cmd.env("AN_TURN", v);
        }
        cmd.args(args).output().unwrap()
    }
    fn call(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn bound_tasks_and_deliveries_keep_workspace_isolation() {
    let f = Fixture::new();
    f.insert("image", "demo", json!("demo"));
    let w = f.call(&["workspace", "create", "--spool", "demo"]);
    let w = w["workspace"].as_str().unwrap();
    f.call(&[
        "workspace",
        "bind",
        w,
        "--harness",
        "test",
        "--session",
        "会话",
    ]);
    let t = f.call(&["task", "new", "--harness", "test", "--session", "会话"]);
    let t = t["task"].as_str().unwrap();
    let s = f.call(&["state", "--task", t]);
    assert_eq!(s["workspace"], w);
    assert_eq!(s["spools"]["demo"]["status"], "needs_admission");
    f.insert("run","delivered",json!({"id":"delivered","status":"delivered","task":t,"spool":"demo","outputs":["object"],"output":"rev"}));
    f.insert("run","private",json!({"id":"private","status":"awaiting_publication","task":t,"spool":"demo","outputs":["object"],"candidate":"private"}));
    let page = f.call(&["deliveries", "--task", t]);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], "delivered");
    assert!(!f
        .run(&["load", "--task", t, "--spool", "demo"])
        .status
        .success());
}
#[test]
fn private_handoff_is_revoked_on_host_change() {
    let mut f = Fixture::new();
    f.insert(
        "task",
        "t",
        json!({"work":{"a":"rev","b":"rev"},"inputs":{},"editable":[],"workspace":"w"}),
    );
    f.insert("workspace", "w", json!({"images":["a","b"]}));
    f.insert("revision", "rev", json!({"result":"eA=="}));
    f.insert(
        "run",
        "r",
        json!({"id":"r","task":"t","candidate":"rev","spool":"a","status":"awaiting_publication"}),
    );
    let file = f.root.join("flows");
    fs::write(&file, r#"[["handoff","a","b"]]"#).unwrap();
    f.call(&[
        "workspace",
        "policy",
        "set",
        "w",
        "--file",
        file.to_str().unwrap(),
    ]);
    let turn = f.call(&["permission", "begin", "--task", "t"]);
    f.turn = Some(turn["turn"].as_str().unwrap().into());
    let args = ["handoff", "--task", "t", "--recipient-spool", "b", "r"];
    let req = f.call(&args);
    let req = req["request_id"].as_str().unwrap();
    f.call(&[
        "permission",
        "decide",
        "--task",
        "t",
        "--request-id",
        req,
        "--decision",
        "once",
    ]);
    let mut retry = args.to_vec();
    retry.extend(["--request-id", req]);
    let ticket = f.call(&retry);
    assert!(ticket["handoff"].as_str().unwrap().starts_with("handoff-"));
    assert!(!f.run(&retry).status.success());
    assert!(!f.run(&["publish", "--task", "t", "r"]).status.success());
    let req = f.call(&args);
    let id = req["request_id"].as_str().unwrap();
    fs::write(&file, r#"{"default":"deny","effects":[]}"#).unwrap();
    f.call(&[
        "permission",
        "policy",
        "--task",
        "t",
        "--file",
        file.to_str().unwrap(),
    ]);
    assert!(!f
        .run(&[
            "permission",
            "decide",
            "--task",
            "t",
            "--request-id",
            id,
            "--decision",
            "once"
        ])
        .status
        .success());
    assert!(!f.run(&args).status.success());
}
#[test]
fn yaml_admission_does_not_use_python() {
    let f = Fixture::new();
    let file = f.root.join("manifest");
    let run = || f.run(&["check", file.to_str().unwrap()]);
    let manifest = "schema: an.spool/v1\nname: native\nsummary: Demo\ninputs: []\noutputs: []\n";
    fs::write(&file, manifest).unwrap();
    assert_eq!(run().stdout, b"native\n");
    for suffix in ["name: again", "extra: true", "---\n{}", "bad: &alias []"] {
        fs::write(&file, format!("{manifest}{suffix}")).unwrap();
        assert!(!run().status.success());
    }
}

#[test]
fn legacy_run_without_body_id_uses_registry_key() {
    let f = Fixture::new();
    f.insert("task", "task", json!({"work":{}}));
    f.insert(
        "run",
        "legacy",
        json!({"task":"task","spool":"image","status":"running"}),
    );
    assert_eq!(
        f.call(&["state", "--task", "task"])["runs"]["legacy"],
        "running"
    );
    assert_eq!(
        f.call(&["cancel", "--task", "task", "legacy"])["status"],
        "cancel_pending"
    );
    assert_eq!(
        f.call(&["status", "--task", "task", "legacy"])["status"],
        "cancel_pending"
    );
}
