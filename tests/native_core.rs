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

impl Fixture {
    fn graph(&self, action: &str, args: Value) -> Result<Value, String> {
        use std::{io::Write, process::Stdio};
        let mut child = Command::new(env!("CARGO_BIN_EXE_an"))
            .args(["graph", "--task", "task", action])
            .env("AN_HOME", &self.root)
            .env("AN_TURN", self.turn.as_deref().unwrap_or(""))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(args.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        if out.status.success() {
            Ok(serde_json::from_slice(&out.stdout).unwrap())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    fn graph_task() -> Self {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let mut f = Fixture::new();
        let schema = json!({"type":"object","properties":{"choice":{"type":"string","enum":["A","B"]}},"required":["choice"]});
        let manifest = json!({"version":1,"entry":"index.html","files":{"index.html":"<p>Compare</p>"},"bindings":[],"response":schema});
        f.insert(
            "revision",
            "panel",
            json!({"panel.json":STANDARD.encode(manifest.to_string())}),
        );
        f.insert(
            "task",
            "task",
            json!({"work":{},"inputs":{"package":"panel"}}),
        );
        f.insert("turn", "task", json!("active"));
        f.turn = Some("active".into());
        f
    }
    fn surface(id: &str) -> Value {
        json!({"id":id,"kind":"surface","package":"ui","inputs":{"ui":{"input":"package"}},
        "outputs":["response"],"response":{"type":"object","properties":{"choice":{"type":"string","enum":["A","B"]}},"required":["choice"]}})
    }
    fn decide_graph(&self, decision: &str) {
        for request in self
            .graph("requests", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
        {
            self.call(&[
                "permission",
                "decide",
                "--task",
                "task",
                "--request-id",
                request["id"].as_str().unwrap(),
                "--decision",
                decision,
            ]);
        }
    }
}

#[test]
fn graph_validates_a_whole_revision_and_never_replays_an_unknown_attempt() {
    let f = Fixture::graph_task();
    let mut a = Fixture::surface("A");
    let mut b = Fixture::surface("B");
    a["after"] = json!(["B"]);
    b["after"] = json!(["A"]);
    let mut request = json!({"idempotency_key":"test","base_revision":0,"nodes":[a,b]});
    assert!(f
        .graph("submit", request.clone())
        .unwrap_err()
        .contains("cycle"));
    assert_eq!(f.graph("view", json!({})).unwrap()["revision"], 0);
    request["nodes"][1]["after"] = json!([]);
    request["nodes"][1]["inputs"]["ui"] = json!({"node":"A","port":"undeclared"});
    assert!(f
        .graph("submit", request)
        .unwrap_err()
        .contains("Unknown output"));

    // Simulate restart after run reservation, with an external outcome already
    // known to be indeterminate. Reconciliation may observe, never relaunch.
    f.insert(
        "run",
        "reserved",
        json!({"id":"reserved","task":"task","status":"unknown","spool":"demo"}),
    );
    f.insert(
        "graph",
        "task",
        json!({"revision":1,"nodes":{"job":{"id":"job","kind":"run","spool":"demo","outputs":[]}},
        "activity":{"job":{"attempt":1,"id":"reserved","status":"queued"}},"events":[]}),
    );
    let state = f.graph("state", json!({})).unwrap();
    assert_eq!(state["activity"]["job"]["status"], "unknown");
    assert_eq!(state["activity"]["job"]["evidence"]["run"], "reserved");
    assert!(f
        .graph(
            "submit",
            json!({"base_revision":1,"idempotency_key":"retry","nodes":[],"retry":["job"]})
        )
        .is_err());
    assert_eq!(
        f.graph("state", json!({"after":1})).unwrap()["events"],
        json!([])
    );
}

#[test]
fn graph_surfaces_recover_drafts_require_current_authority_and_settle_once() {
    let mut f = Fixture::graph_task();
    let submit =
        json!({"idempotency_key":"one","base_revision":0,"nodes":[Fixture::surface("choice")]});
    let receipt = f.graph("submit", submit.clone()).unwrap();
    assert_eq!(receipt, f.graph("submit", submit).unwrap());
    f.decide_graph("deny");
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["choice"]["status"],
        "failed"
    );
    f.graph(
        "submit",
        json!({"idempotency_key":"two","base_revision":1,"nodes":[],"retry":["choice"]}),
    )
    .unwrap();
    f.decide_graph("once");
    let state = f.graph("view", json!({})).unwrap()["activity"]["choice"].clone();
    assert_eq!(state["attempt"], 2);
    let mut req = json!({"node":"choice","request":state["id"]});
    f.graph("panel", req.clone()).unwrap();
    f.graph("viewed", req.clone()).unwrap();
    req["value"] = json!({"choice":"B"});
    f.graph("draft", req.clone()).unwrap();
    let original: String =
        f.db.query_row(
            "SELECT body FROM objects WHERE kind='revision' AND id='panel'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    f.insert("revision", "panel", json!({"panel.json":"e30="}));
    assert!(f.graph("panel", req.clone()).is_err());
    f.insert(
        "revision",
        "panel",
        serde_json::from_str(&original).unwrap(),
    );
    assert_eq!(
        f.graph("panel", req.clone()).unwrap()["draft"],
        req["value"]
    );

    f.insert("turn", "task", json!("new-turn"));
    assert!(f.graph("respond", req.clone()).is_err());
    f.turn = Some("new-turn".into());
    f.graph("state", json!({})).unwrap();
    f.decide_graph("once");
    assert!(f.graph("respond", req.clone()).is_err());
    let state = f.graph("view", json!({})).unwrap();
    req["request"] = state["activity"]["choice"]["id"].clone();
    f.graph("panel", req.clone()).unwrap();
    f.graph("viewed", req.clone()).unwrap();
    req["value"]["choice"] = json!("C");
    assert!(f.graph("respond", req.clone()).is_err());
    req["value"]["choice"] = json!("B");
    for _ in 0..2 {
        assert_eq!(f.graph("respond", req.clone()).unwrap()["status"], "done");
    }
    let state = f.graph("state", json!({})).unwrap();
    assert_eq!(
        state["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "responded")
            .count(),
        1
    );
}

#[test]
fn inactive_turn_only_sees_public_status() {
    let mut f = Fixture::graph_task();
    f.insert(
        "graph",
        "task",
        json!({"revision":1,"events":[{"node":"n","data":"private"}],
        "nodes":{"n":{"id":"n","kind":"surface","title":"private"}},
        "activity":{"n":{"status":"done","response":"private","bindings":{"secret":"private"}}}}),
    );
    for turn in [None, Some("expired".to_owned())] {
        f.turn = turn;
        let view = f.graph("view", json!({})).unwrap();
        assert_eq!(view["authorized"], false);
        assert_eq!(view["activity"]["n"], json!({"status":"done"}));
        assert!(!view.to_string().contains("private"));
        let trace = f.graph("sources", json!({"node":"n"})).unwrap();
        assert_eq!(trace["n"]["execution"], json!({"status":"done"}));
        assert!(!trace.to_string().contains("private"));
    }
}

#[test]
fn superseded_save_settles_waiting_document() {
    let f = Fixture::graph_task();
    f.insert(
        "graph",
        "task",
        json!({"revision":1,"events":[],"nodes":{
        "s":{"id":"s","kind":"surface","save":{"node":"writer","port":"file"}},
        "writer":{"id":"writer","kind":"run"}},
        "activity":{"s":{"status":"saving"},"writer":{"status":"superseded"}}}),
    );
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["s"]["status"],
        "superseded"
    );
    assert_eq!(f.graph("finish", json!({})).unwrap()["closed"], true);
}

#[test]
fn delivery_rejects_ambiguous_flattened_paths_before_publication() {
    for (left, right) in [("b/c", "c"), ("b", "c")] {
        let f = Fixture::graph_task();
        f.insert("task", "task", json!({"inputs":{"one":"v1","two":"v2"}}));
        f.insert("revision", "v1", json!({left:"YQ=="}));
        f.insert("revision", "v2", json!({right:"Yg=="}));
        f.graph("submit",json!({"base_revision":0,"idempotency_key":"publish","nodes":[
            {"kind":"delivery","id":"delivery","outputs":[],"inputs":{"a":{"input":"one"},"a/b":{"input":"two"}}}
        ]})).unwrap();
        for _ in 0..2 {
            f.decide_graph("once");
        }
        let state = f.graph("state", json!({})).unwrap();
        assert_eq!(state["activity"]["delivery"]["status"], "failed");
        assert_eq!(f.graph("deliveries", json!({})).unwrap(), json!([]));
    }
}

#[test]
fn graph_export_requires_all_origins_and_exact_revision_approval() {
    let f = Fixture::graph_task();
    f.insert("task", "task", json!({"workspace":"w"}));
    f.insert("workspace", "w", json!({"images":["spool"]}));
    f.insert(
        "graph_delivery",
        "group",
        json!({"task":"closed","workspace":"w","revision":"version","bindings":{
        "one":{"spool":"spool","revision":"a"},"two":{"revision":"b"}}}),
    );
    f.insert(
        "revision",
        "version",
        json!({"one/a":"YQ==","two/b":"Yg=="}),
    );
    let path = f.root.canonicalize().unwrap().join("export");
    let dst = path.to_str().unwrap();
    let policy = f.root.join("policy");
    let policyfile = policy.to_str().unwrap();
    let set_policy = |flows: Value| {
        fs::write(&policy, flows.to_string()).unwrap();
        f.call(&["workspace", "policy", "set", "w", "--file", policyfile]);
    };
    set_policy(json!([["export", "spool", dst]]));
    let args = ["export", "group", dst, "--task", "task"];
    assert!(!f.run(&args).status.success());
    assert!(!path.exists());
    set_policy(json!([
        ["export", "spool", dst],
        ["export", "response", dst]
    ]));
    let pending = f.call(&args);
    assert!(!path.exists());
    let id = pending["request_id"].as_str().unwrap();
    f.call(&[
        "permission",
        "decide",
        "--task",
        "task",
        "--request-id",
        id,
        "--decision",
        "once",
    ]);
    let mut approved = args.to_vec();
    approved.extend(["--request-id", id]);
    assert!(f.run(&approved).status.success());
    assert_eq!(fs::read(path.join("one/a")).unwrap(), b"a");
    assert_eq!(fs::read(path.join("two/b")).unwrap(), b"b");
}

#[test]
fn captured_driver_proposal_is_reconciled_atomically_under_its_authority() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use sha2::{Digest, Sha256};
    let stamp = format!("{:x}", Sha256::digest(br#"["active", "legacy", "legacy"]"#));
    for case in ["valid", "invalid", "expired", "denied", "budget"] {
        let f = Fixture::graph_task();
        let mut children = vec![Fixture::surface("child")];
        if case == "invalid" {
            children.push(Fixture::surface("driver"));
        }
        if case == "budget" {
            children = (0..2)
                .map(|i| Fixture::surface(&format!("child{i}")))
                .collect();
        }
        f.insert(
            "revision",
            "captured",
            json!({"subgraph.json":STANDARD.encode(json!({"nodes":children}).to_string())}),
        );
        f.insert(
            "instance",
            "instance",
            json!({"id":"instance","task":"task","status":"unloaded"}),
        );
        f.insert("run","attempt",json!({"id":"attempt","task":"task","status":"completed","candidate":"captured","outputs":["subgraph.json"]}));
        let drivers: serde_json::Map<_, _> = if case == "budget" {
            (0..255)
                .map(|i| (format!("accepted{i}"), json!("previous")))
                .collect()
        } else {
            Default::default()
        };
        f.insert("graph","task",json!({"revision":1,"drivers":drivers,"nodes":{"driver":{"id":"driver","kind":"expand","outputs":["subgraph.json"]}},
            "events":[],"activity":{"driver":{"id":"attempt","status":"running","instance":"instance","authority":if case == "expired" {"old"} else {&stamp}}}}));
        let pending = f.graph("state", json!({})).unwrap();
        assert!(!pending.to_string().contains("child"));
        if case != "expired" {
            assert_eq!(
                pending["activity"]["driver"]["status"],
                "permission_required"
            );
            f.decide_graph(if case == "denied" { "deny" } else { "once" });
        }
        for _ in 0..2 {
            let graph = f.graph("state", json!({})).unwrap();
            assert_eq!(graph["revision"], if case == "valid" { 2 } else { 1 });
            assert_eq!(graph["nodes"].get("child").is_some(), case == "valid");
            assert_eq!(
                graph["activity"]["driver"]["status"],
                if case == "valid" { "done" } else { "failed" }
            );
        }
    }
}

#[test]
fn public_run_selection_excludes_driver_reservations() {
    let f = Fixture::graph_task();
    f.insert("instance", "driver-x", json!({"id":"driver-x","task":"task","spool":"sample","owner":"x","dedicated":true,"status":"running"}));
    for (selector, value, error) in [
        ("--spool", "sample", "Load the spool first"),
        ("--instance", "driver-x", "Instance reserved for driver"),
    ] {
        let out = f.run(&["run", "--task", "task", selector, value, "true"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains(error));
    }
}

#[test]
fn saver_reservation_rejects_same_batch_and_later_submissions() {
    let f = Fixture::graph_task();
    let mut surface = Fixture::surface("review");
    surface["save"] = json!({"node":"save","port":"file"});
    surface["continuation"] = json!("driver");
    let proposal = |nodes: Value| json!({"nodes":nodes,"idempotency_key":"test","base_revision":0});
    let claimed = Fixture::surface("save");
    assert!(f
        .graph("submit", proposal(json!([surface, claimed])))
        .unwrap_err()
        .contains("unclaimed saver"));
    f.insert(
        "graph",
        "task",
        json!({"nodes":{"review":surface},"revision":0,"activity":{},"events":[]}),
    );
    assert!(f
        .graph("submit", proposal(json!([claimed])))
        .unwrap_err()
        .contains("reserved for continuation"));
}

#[test]
fn driver_cleanup_is_reconciled_before_metadata_authorization() {
    use sha2::{Digest, Sha256};
    let f = Fixture::graph_task();
    let instance = uuid::Uuid::new_v4().simple().to_string();
    let record = json!({"id":instance,"name":format!("an-{instance}"),"task":"task","status":"cleanup_pending","intent":"unload","dedicated":true,"owner":"attempt"});
    f.insert("instance", &instance, record.clone());
    // Even if the VM is absent, an unremovable staging path keeps unload pending.
    fs::create_dir(f.root.join("instances")).unwrap();
    fs::write(f.root.join("instances").join(&instance), b"pending").unwrap();
    let stamp = format!("{:x}", Sha256::digest(br#"["active", "legacy", "legacy"]"#));
    f.insert("graph","task",json!({"revision":1,"nodes":{"driver":{"id":"driver","kind":"expand","spool":"demo"}},
        "events":[],"activity":{"driver":{"id":"attempt","status":"cleanup_pending","after_cleanup":"done","instance":instance,
        "authority":stamp,"outputs":{"subgraph.json":{"revision":"candidate"}}}}}));
    let state = f.graph("state", json!({})).unwrap();
    assert_eq!(state["activity"]["driver"]["status"], "cleanup_pending");
    assert_eq!(f.graph("requests", json!({})).unwrap(), json!([]));
    let mut confirmed = record;
    confirmed["status"] = json!("unloaded");
    f.insert("instance", &instance, confirmed);
    let state = f.graph("state", json!({})).unwrap();
    assert_eq!(state["activity"]["driver"]["status"], "permission_required");
    assert_eq!(state["revision"], 1);
}

#[test]
fn driver_reserves_instance_even_when_startup_fails() {
    let f = Fixture::graph_task();
    f.insert("graph","task",json!({"revision":1,"nodes":{"driver":{"id":"driver","kind":"expand","spool":"unavailable"}},
        "events":[],"activity":{"driver":{"id":"attempt","status":"queued"}}}));
    for _ in 0..2 {
        let state = f.graph("state", json!({})).unwrap();
        assert_eq!(state["activity"]["driver"]["status"], "failed");
        assert_eq!(state["activity"]["driver"]["instance"], "driver-attempt");
    }
}
