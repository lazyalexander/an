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
        f.insert(
            "revision",
            "panel",
            json!({"spool/code/panels/test.html":STANDARD.encode("<p>Compare</p>")}),
        );
        f.insert("admission","demo",json!({"code_revision":"panel","manifest_digest":"example","manifest":{"schema":"an.spool/v2","panels":{"choice":{"kind":"surface","title":"Choose","bindings":["ui"],"html":"spool/code/panels/test.html","decision":{"type":"adjust","value_type":"string","field":"choice","options":["A","B"]}}}}}));
        f.insert(
            "task",
            "task",
            json!({"work":{"test@demo":"work"},"inputs":{"package":"panel"}}),
        );
        f.insert("turn", "task", json!("active"));
        f.turn = Some("active".into());
        f
    }
    fn surface(id: &str) -> Value {
        json!({"id":id,"kind":"surface","panel":"choice","spool":"test@demo","inputs":{"ui":{"input":"package"}},"outputs":["response"]})
    }
    fn propose_output(&self, routes: Value) -> Output {
        let path = self.root.join("routes.json");
        fs::write(&path, routes.to_string()).unwrap();
        self.run(&[
            "permission",
            "intent-propose",
            "--task",
            "task",
            "--file",
            path.to_str().unwrap(),
        ])
    }
    fn propose(&self, routes: Value) -> String {
        let output = self.propose_output(routes);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn decide_intent(&self, id: &str, decisions: Value) {
        self.call(&[
            "permission",
            "intent-decide",
            "--task",
            "task",
            "--request-id",
            id,
            "--decision",
            &decisions.to_string(),
        ]);
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
    for case in [
        "valid",
        "invalid",
        "legacy-panel",
        "expired",
        "denied",
        "budget",
    ] {
        let f = Fixture::graph_task();
        let mut children = vec![Fixture::surface("child")];
        if case == "invalid" {
            children.push(Fixture::surface("driver"));
        }
        if case == "legacy-panel" {
            children[0]["package"] = json!("ui");
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

#[test]
fn person_controls_pause_resume_and_stop_without_the_agent() {
    let mut f = Fixture::graph_task();
    let control = |f: &Fixture, action: &str| f.graph("control", json!({ "action": action }));
    assert_eq!(control(&f, "pause").unwrap()["control"], "paused");
    f.graph(
        "submit",
        json!({"idempotency_key":"held","base_revision":0,"nodes":[Fixture::surface("held")]}),
    )
    .unwrap();
    // Paused: the agent may plan, but nothing starts, not even a preview request.
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["held"]["status"],
        "queued"
    );
    assert_eq!(f.graph("requests", json!({})).unwrap(), json!([]));
    assert_eq!(f.graph("view", json!({})).unwrap()["control"], "paused");

    assert_eq!(control(&f, "resume").unwrap()["control"], "running");
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["held"]["status"],
        "permission_required"
    );

    // Stop cancels unsettled work and holds the task paused.
    let stopped = control(&f, "stop").unwrap();
    assert_eq!(stopped["control"], "paused");
    assert_eq!(stopped["cancelled"], json!(["held"]));
    let state = f.graph("state", json!({})).unwrap();
    assert_eq!(state["activity"]["held"]["status"], "cancelled");
    let kinds: Vec<_> = state["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["actor"] == "user")
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["paused", "resumed", "stopped"]);

    // After resume the agent can retry what the person stopped.
    control(&f, "resume").unwrap();
    f.graph(
        "submit",
        json!({"idempotency_key":"again","base_revision":1,"nodes":[],"retry":["held"]}),
    )
    .unwrap();
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["held"]["status"],
        "permission_required"
    );

    assert!(control(&f, "explode")
        .unwrap_err()
        .contains("Unknown control"));
    f.turn = None;
    assert!(
        control(&f, "stop").is_err(),
        "controls need the person's turn"
    );
}

#[test]
fn stop_requests_cancellation_of_running_work() {
    let f = Fixture::graph_task();
    f.insert(
        "run",
        "live",
        json!({"id":"live","task":"task","status":"running","spool":"demo"}),
    );
    f.insert(
        "graph",
        "task",
        json!({"revision":1,"events":[],"nodes":{"job":{"id":"job","kind":"run","spool":"demo","outputs":[]}},
        "activity":{"job":{"attempt":1,"id":"live","status":"running","run":"live"}}}),
    );
    let stopped = f.graph("control", json!({"action":"stop"})).unwrap();
    f.graph("control", json!({"action":"resume"})).unwrap();
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["job"]["status"],
        "cancel_pending"
    );
    assert!(f
        .graph(
            "submit",
            json!({"base_revision":1,"idempotency_key":"retry","nodes":[],"retry":["job"]})
        )
        .is_err());
    assert_eq!(stopped["cancelling"], json!(["live"]));
    assert_eq!(
        f.call(&["status", "--task", "task", "live"])["status"],
        "cancel_pending"
    );
}

#[test]
fn instructions_are_relayed_unchanged_with_the_revision_the_person_saw() {
    let mut f = Fixture::graph_task();
    let sent = f
        .graph(
            "instruct",
            json!({"text":"  用 B，转慢一点  ","revision":0}),
        )
        .unwrap();
    assert_eq!(sent["stale"], false);
    f.graph(
        "submit",
        json!({"idempotency_key":"one","base_revision":0,"nodes":[Fixture::surface("s")]}),
    )
    .unwrap();
    // The person typed while looking at revision 0; the graph has moved on.
    let late = f
        .graph("instruct", json!({"text":"换个角度","revision":0}))
        .unwrap();
    assert_eq!(late["stale"], true);
    let events = f.graph("state", json!({})).unwrap()["events"].clone();
    let relayed: Vec<_> = events
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "instruction")
        .collect();
    assert_eq!(relayed.len(), 2);
    assert_eq!(relayed[0]["actor"], "user");
    assert_eq!(relayed[0]["data"]["text"], "  用 B，转慢一点  ");
    assert_eq!(relayed[1]["data"]["seen_revision"], 0);
    assert_eq!(relayed[1]["data"]["revision"], 1);
    // An instruction is not a surface response.
    assert_ne!(
        f.graph("state", json!({})).unwrap()["activity"]["s"]["status"],
        "done"
    );

    assert!(f
        .graph("instruct", json!({"text":"   ","revision":1}))
        .is_err());
    assert!(f
        .graph("instruct", json!({"text":"x","revision":9}))
        .unwrap_err()
        .contains("Unknown graph revision"));
    f.turn = None;
    assert!(f
        .graph("instruct", json!({"text":"x","revision":1}))
        .is_err());
}

#[test]
fn host_config_overrides_limits_and_rejects_unknown_settings() {
    let f = Fixture::graph_task();
    std::fs::write(
        f.root.join("config.json"),
        json!({"instruction_max_bytes":4}).to_string(),
    )
    .unwrap();
    assert!(f
        .graph("instruct", json!({"text":"abcd","revision":0}))
        .is_ok());
    assert!(f
        .graph("instruct", json!({"text":"abcde","revision":0}))
        .unwrap_err()
        .contains("exceeds 4 bytes"));
    std::fs::write(
        f.root.join("config.json"),
        json!({"instruction_max_byte":4}).to_string(),
    )
    .unwrap();
    assert!(f
        .graph("view", json!({}))
        .unwrap_err()
        .contains("Unknown setting"));
    std::fs::write(
        f.root.join("config.json"),
        json!({"mcp_deadline_s":0}).to_string(),
    )
    .unwrap();
    assert!(f
        .graph("view", json!({}))
        .unwrap_err()
        .contains("positive integer"));
}

#[test]
fn new_submissions_require_admitted_panels_but_legacy_receipts_and_deliveries_reopen() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let f = Fixture::graph_task();
    let legacy = json!({"id":"old","kind":"surface","package":"ui","inputs":{"ui":{"input":"package"}},"response":{"type":"object","properties":{}},"outputs":["response"]});
    let submission =
        |node: Value| json!({"base_revision":0,"idempotency_key":"one","nodes":[node]});
    for node in [
        legacy.clone(),
        json!({"id":"pub","kind":"delivery","package":"ui","inputs":{"ui":{"input":"package"}},"outputs":[]}),
        {
            let mut n = Fixture::surface("bad");
            n["spool"] = json!("other@demo");
            n
        },
        {
            let mut n = Fixture::surface("bad");
            n["panel"] = json!("absent");
            n
        },
        {
            let mut n = Fixture::surface("bad");
            n["response"] = json!({});
            n
        },
    ] {
        assert!(f.graph("submit", submission(node)).is_err());
    }
    let args = submission(legacy.clone());
    let receipt = json!({"revision":1});
    f.insert(
        "graph_submission",
        r#"["task", "one"]"#,
        json!({"args":args,"receipt":receipt}),
    );
    f.insert("graph","task",json!({"revision":1,"nodes":{"old":legacy},"activity":{"old":{"status":"done"}},"events":[]}));
    assert_eq!(f.graph("submit", args).unwrap(), receipt);
    let html = "<p>Historical delivery</p>";
    let pkg = json!({"version":1,"entry":"index.html","files":{"index.html":html},"bindings":[],"response":null});
    f.insert(
        "revision",
        "old-ui",
        json!({"panel.json":STANDARD.encode(pkg.to_string())}),
    );
    f.insert("graph_delivery","published",json!({"id":"published","task":"task","package":"ui","bindings":{"ui":{"revision":"old-ui"}}}));
    assert_eq!(
        f.graph("open", json!({"delivery":"published"})).unwrap()["html"],
        html
    );

    // An already waiting v1 surface retains its original attempt and authority.
    use sha2::{Digest, Sha256};
    let stamp = format!("{:x}", Sha256::digest(br#"["active", "legacy", "legacy"]"#));
    let mut interactive = pkg;
    interactive["response"] = legacy["response"].clone();
    f.insert(
        "revision",
        "old-decision",
        json!({"panel.json":STANDARD.encode(interactive.to_string())}),
    );
    f.insert("graph","task",json!({"revision":1,"nodes":{"old":legacy},"activity":{"old":{
        "id":"existing","status":"waiting","authority":stamp,"bindings":{"ui":{"revision":"old-decision"}}
    }},"events":[]}));
    let request = json!({"node":"old","request":"existing"});
    assert_eq!(f.graph("panel", request.clone()).unwrap()["html"], html);
    f.graph("viewed", request.clone()).unwrap();
    assert_eq!(
        f.graph(
            "respond",
            json!({"node":"old","request":"existing","value":{}})
        )
        .unwrap()["status"],
        "done"
    );
}

#[test]
fn interactive_and_json_answers_compete_for_one_attempt() {
    use std::{io::Write, process::Stdio};
    let f = Fixture::graph_task();
    f.graph(
        "submit",
        json!({"base_revision":0,"idempotency_key":"ask","nodes":[Fixture::surface("question")]}),
    )
    .unwrap();
    f.decide_graph("once");
    let list = f.call(&["decision", "list", "--task", "task"]);
    let request = list[0]["request"].as_str().unwrap();
    let answer = |json: bool, data: &[u8]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_an"));
        cmd.args([
            "decision",
            "answer",
            "--task",
            "task",
            "--node",
            "question",
            "--request",
            request,
        ]);
        if json {
            cmd.args(["--json", "-"]);
        }
        let mut child = cmd
            .env("AN_HOME", &f.root)
            .env("AN_TURN", "active")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(data).unwrap();
        child
    };
    assert!(!answer(true, br#"{"choice":"invalid"}"#)
        .wait()
        .unwrap()
        .success());
    let mut a = answer(false, b"invalid\nA\n");
    let mut b = answer(true, br#"{"choice":"B"}"#);
    assert_ne!(a.wait().unwrap().success(), b.wait().unwrap().success());
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
    assert!(!answer(true, br#"{"choice":"A"}"#).wait().unwrap().success());
}

#[test]
fn intent_denial_resolves_an_existing_preview_request() {
    let f = Fixture::graph_task();
    f.graph("submit", json!({"base_revision":0,"nodes":[Fixture::surface("preview")],"idempotency_key":"preview"})).unwrap();
    let pending = ["permission", "pending", "--task", "task"];
    assert_eq!(f.call(&pending).as_array().unwrap().len(), 1);
    let path = f.root.join("intent.json");
    let routes = json!([{"operation":"preview","origin":"task-input:package","source":{"input":"package"},"destination":"studio"}]);
    fs::write(&path, routes.to_string()).unwrap();
    let card = f.call(&[
        "permission",
        "intent-propose",
        "--task",
        "task",
        "--file",
        path.to_str().unwrap(),
    ]);
    f.call(&[
        "permission",
        "intent-decide",
        "--task",
        "task",
        "--request-id",
        card["id"].as_str().unwrap(),
        "--decision",
        "deny",
    ]);
    assert_eq!(f.call(&pending), json!([]));
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["preview"]["status"],
        "failed"
    );
}

#[test]
fn later_instructions_can_add_routes_without_reopening_earlier_decisions() {
    let mut f = Fixture::graph_task();
    // Resume a graph whose surface already produced a response before this turn.
    f.insert(
        "graph",
        "task",
        json!({"revision":1, "events":[],
        "nodes":{"source":Fixture::surface("source")},
        "activity":{"source":{"id":"source-v1","attempt":1,"status":"done",
                              "outputs":{"response":{"revision":"panel"}}}}}),
    );
    f.insert(
        "task",
        "task",
        json!({"work":{"test@demo":"work"},
        "inputs":{"package":"panel","addition":"panel"}}),
    );
    let preview = |origin: &str, source: Value| json!({"operation":"preview","origin":origin,"source":source,"destination":"studio"});
    let response = json!({"node":"source","port":"response"});
    let first_route = preview("response", response.clone());
    let denied_route = preview("task-input:package", json!({"input":"package"}));
    let new_route = preview("task-input:addition", json!({"input":"addition"}));

    let instruct = |text: &str| {
        f.graph("instruct", json!({"revision":1,"text":text}))
            .unwrap()["cursor"]
            .clone()
    };
    let initial_cursor = instruct("Review the first response");
    let initial = f.propose(json!({"turn_scope":[first_route, denied_route]}));
    f.decide_intent(&initial, json!(["allow", "deny"]));
    let preview_from = |id: &str, source: &Value| {
        let mut node = Fixture::surface(id);
        node["inputs"]["ui"] = source.clone();
        let revision = f.graph("view", json!({})).unwrap()["revision"].clone();
        f.graph(
            "submit",
            json!({"base_revision":revision,"nodes":[node],"idempotency_key":id}),
        )
        .unwrap();
        f.graph("state", json!({})).unwrap()["activity"][id].clone()
    };
    assert_eq!(preview_from("first", &response)["status"], "waiting");

    let state = f.graph("state", json!({})).unwrap();
    let agent_or_host_event = state["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] != "instruction")
        .unwrap()["cursor"]
        .clone();
    let proposal =
        |cursor: &Value, route: &Value| json!({"turn_scope":[route],"instruction_cursor":cursor});
    assert!(!f
        .propose_output(json!({"turn_scope":[new_route]}))
        .status
        .success());
    for cursor in [
        initial_cursor,
        agent_or_host_event,
        json!(0),
        json!(9999),
        json!("2"),
    ] {
        assert!(!f
            .propose_output(proposal(&cursor, &new_route))
            .status
            .success());
    }
    let superseded = instruct("Review the additional input");
    let next = instruct("Preview the additional input in Studio");
    assert!(!f
        .propose_output(proposal(&superseded, &new_route))
        .status
        .success());
    // A later instruction cannot turn an existing denial into a fresh allow vote.
    assert!(!f
        .propose_output(proposal(&next, &denied_route))
        .status
        .success());
    let card = f.propose(proposal(&next, &new_route));
    assert_ne!(card, initial);
    let pending = f.call(&["permission", "intent-show", "--task", "task"]);
    assert_eq!(pending["id"], card.as_str());
    assert_eq!(pending["routes"].as_array().unwrap().len(), 1);
    assert_eq!(pending["status"], "pending");
    let scopes = f.graph("state", json!({})).unwrap()["turn_scopes"].clone();
    assert_eq!(scopes[0]["id"], initial.as_str());
    assert_eq!(scopes[0]["decisions"], json!(["allow", "deny"]));
    assert_eq!(scopes[1]["id"], card.as_str());
    assert!(scopes
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c.get("stamp").is_none()));
    assert_eq!(preview_from("retained", &response)["status"], "waiting");
    let denied = preview_from("denied", &json!({"input":"package"}));
    assert_eq!(denied["status"], "failed");
    assert!(denied["error"].as_str().unwrap().contains("Denied"));
    assert_eq!(
        preview_from("addition", &json!({"input":"addition"}))["status"],
        "permission_required"
    );

    f.decide_intent(&card, json!(["allow"]));
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["addition"]["status"],
        "waiting"
    );
    // A recovered source now has a different immutable output. Approval for
    // its original bytes must still be pinned to the first card's identity.
    f.insert("revision", "v2", json!({"changed":"Yg=="}));
    f.db.execute("UPDATE objects SET body=json_set(body,'$.activity.source.outputs.response.revision','v2') WHERE kind='graph' AND id='task'", []).unwrap();
    assert_eq!(
        preview_from("changed", &response)["status"],
        "permission_required"
    );
    assert_eq!(
        preview_from("still-denied", &json!({"input":"package"}))["status"],
        "failed"
    );

    let replacement = f.call(&["permission", "begin", "--task", "task"]);
    f.turn = Some(replacement["turn"].as_str().unwrap().to_owned());
    assert!(f
        .call(&["permission", "intent-show", "--task", "task"])
        .is_null());
    assert!(!f
        .run(&[
            "permission",
            "intent-decide",
            "--task",
            "task",
            "--request-id",
            &card,
            "--decision",
            "allow"
        ])
        .status
        .success());
    let fresh = f.propose(json!({"turn_scope":[first_route]}));
    f.decide_intent(&fresh, json!(["allow"]));
    assert!(!f
        .propose_output(proposal(&next, &new_route))
        .status
        .success());
}

#[test]
fn delegated_mcp_can_propose_an_amendment_only_for_a_current_instruction() -> anyhow::Result<()> {
    use rmcp::{model::CallToolRequestParams, transport::TokioChildProcess, ServiceExt};

    let mut f = Fixture::graph_task();
    let owner = f.call(&["permission", "begin", "--task", "task"]);
    f.turn = Some(owner["turn"].as_str().unwrap().to_owned());
    let initial = f.propose(json!({"turn_scope":[{
        "operation":"preview","origin":"task-input:package",
        "source":{"input":"package"},"destination":"studio"
    }]}));
    f.decide_intent(&initial, json!(["allow"]));
    let added = json!({"operation":"publish","origin":"task-input:package",
                       "source":{"input":"package"},"destination":"task"});
    let cursor = f
        .graph(
            "instruct",
            json!({"revision":0,"text":"Deliver the source as well"}),
        )
        .unwrap()["cursor"]
        .clone();
    let token = f.call(&["permission", "delegate", "--task", "task"])["agent_token"].clone();

    tokio::runtime::Runtime::new()?.block_on(async {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_an"));
        command
            .args(["mcp", "--task", "task"])
            .env("AN_HOME", &f.root)
            .env_remove("AN_TURN")
            .env("AN_AGENT_TOKEN", token.as_str().unwrap());
        let peer = ().serve(TokioChildProcess::new(command)?).await?;
        let request = |cursor: Value| {
            let args = json!({"routes":[added],"instruction_cursor":cursor});
            CallToolRequestParams::new("an_scope_propose")
                .with_arguments(args.as_object().unwrap().clone())
        };
        let bad = peer.call_tool(request(json!(9999))).await?;
        assert_eq!(bad.is_error, Some(true));
        let response = peer.call_tool(request(cursor.clone())).await?;
        let text = &response.content[0].as_text().unwrap().text;
        assert_ne!(response.is_error, Some(true), "{text}");
        let card: Value = serde_json::from_str(text)?;
        assert!(card.get("stamp").is_none());
        assert_eq!(card["status"], "pending");
        assert_eq!(
            f.call(&["permission", "intent-show", "--task", "task"])["id"],
            card["id"]
        );
        let tools = peer.list_all_tools().await?;
        assert!(!tools.iter().any(|tool| tool.name.contains("decide")));
        f.decide_intent(card["id"].as_str().unwrap(), json!(["allow"]));
        f.call(&["permission", "begin", "--task", "task"]);
        let stale = peer.call_tool(request(cursor)).await?;
        assert_eq!(stale.is_error, Some(true));
        peer.cancel().await?;
        Ok(())
    })
}

#[test]
fn intent_decisions_survive_process_restart_and_expire_with_authority() {
    for change in ["turn", "policy"] {
        let f = Fixture::graph_task();
        let file = f.root.join("intent.json");
        fs::write(&file,json!([{"operation":"preview","origin":"task-input:package","source":{"input":"package"},"destination":"studio"}]).to_string()).unwrap();
        let card = f.call(&[
            "permission",
            "intent-propose",
            "--task",
            "task",
            "--file",
            file.to_str().unwrap(),
        ]);
        assert_eq!(card["routes"][0]["revision"], "panel");
        assert!(card.get("stamp").is_none());
        let decide = [
            "permission",
            "intent-decide",
            "--task",
            "task",
            "--request-id",
            card["id"].as_str().unwrap(),
            "--decision",
            "allow",
        ];
        let first = f.call(&decide);
        assert_eq!(first, f.call(&decide));
        assert_eq!(
            first,
            f.call(&["permission", "intent-show", "--task", "task"])
        );
        f.graph("submit",json!({"base_revision":0,"nodes":[Fixture::surface("before")],"idempotency_key":"first"})).unwrap();
        assert_eq!(
            f.graph("state", json!({})).unwrap()["activity"]["before"]["status"],
            "waiting"
        );
        if change == "turn" {
            f.call(&["permission", "begin", "--task", "task"]);
        } else {
            fs::write(&file, r#"{"default":"ask","effects":[]}"#).unwrap();
            f.call(&[
                "permission",
                "policy",
                "--task",
                "task",
                "--file",
                file.to_str().unwrap(),
            ]);
            assert_eq!(
                f.call(&["permission", "intent-show", "--task", "task"]),
                Value::Null
            );
            f.graph("submit",json!({"base_revision":1,"nodes":[Fixture::surface("after")],"idempotency_key":"second"})).unwrap();
            assert_eq!(
                f.graph("state", json!({})).unwrap()["activity"]["after"]["status"],
                "permission_required"
            );
        }
        assert!(!f.run(&decide).status.success());
    }
}

#[test]
fn mixed_intent_settles_pending_inputs_before_the_node_can_proceed() {
    for decision in ["ask", "deny"] {
        let f = Fixture::graph_task();
        f.insert(
            "task",
            "task",
            json!({"work":{"test@demo":"work"},"inputs":{"a":"panel","b":"panel","other":"panel"}}),
        );
        let delivery = |id: &str, inputs: Value| json!({"id":id,"kind":"delivery","inputs":inputs,"outputs":[]});
        let nodes = [
            delivery("both", json!({"a":{"input":"a"},"b":{"input":"b"}})),
            delivery("independent", json!({"c":{"input":"other"}})),
        ];
        f.graph(
            "submit",
            json!({"base_revision":0,"nodes":nodes,"idempotency_key":"deliver"}),
        )
        .unwrap();
        let route = |name: &str| json!({"operation":"publish","origin":format!("task-input:{name}"),"source":{"input":name},"destination":"task"});
        let card = f.propose(json!([route("a"), route("b")]));
        let pending = f.graph("requests", json!({})).unwrap();
        let pending = pending.as_array().unwrap();
        assert_eq!(pending.len(), 2);
        let covered = pending.iter().find(|r| r["scope"][1] == "a").unwrap();
        assert_eq!(covered["intent"], card);
        let independent = pending.iter().find(|r| r["scope"][1] == "c").unwrap();
        assert!(independent.get("intent").is_none());
        f.call(&[
            "permission",
            "decide",
            "--task",
            "task",
            "--request-id",
            independent["id"].as_str().unwrap(),
            "--decision",
            "once",
        ]);
        f.decide_intent(&card, json!(["allow", decision]));
        let pending = f.call(&["permission", "pending", "--task", "task"]);
        let state = f.graph("state", json!({})).unwrap();
        assert_eq!(state["activity"]["independent"]["status"], "done");
        if decision == "deny" {
            assert_eq!(pending, json!([]));
            assert_eq!(state["activity"]["both"]["error"], "Denied by turn intent");
        } else {
            assert_eq!(pending.as_array().unwrap().len(), 1);
            assert_eq!(pending[0]["scope"][1], "b");
            f.decide_graph("once");
            assert_eq!(
                f.graph("state", json!({})).unwrap()["activity"]["both"]["status"],
                "done"
            );
        }
    }
}

#[test]
fn explicit_ask_requires_a_new_confirmation_despite_a_turn_grant() {
    let f = Fixture::graph_task();
    let submit = |id: &str, rev: u64| {
        f.graph(
            "submit",
            json!({"base_revision":rev,"nodes":[Fixture::surface(id)],"idempotency_key":id}),
        )
        .unwrap()
    };
    submit("first", 0);
    f.decide_graph("turn");
    let id = f.propose(json!([{"operation":"preview","origin":"task-input:package","source":{"input":"package"},"destination":"studio"}]));
    f.decide_intent(&id, json!(["ask"]));
    submit("second", 1);
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["second"]["status"],
        "permission_required"
    );
    f.decide_graph("once");
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["second"]["status"],
        "waiting"
    );
}

#[test]
fn explicit_ask_requires_a_new_confirmation_despite_a_host_allowance() {
    let f = Fixture::graph_task();
    f.graph(
        "submit",
        json!({"base_revision":0,"nodes":[Fixture::surface("review")],"idempotency_key":"review"}),
    )
    .unwrap();
    let requests = f.graph("requests", json!({})).unwrap();
    f.graph("control", json!({"action":"pause"})).unwrap();
    f.call(&[
        "permission",
        "effect",
        "--task",
        "task",
        "--request-id",
        requests[0]["id"].as_str().unwrap(),
        "--decision",
        "allow",
    ]);
    let card = f.propose(json!([{"operation":"preview","origin":"task-input:package","source":{"input":"package"},"destination":"studio"}]));
    f.decide_intent(&card, json!(["ask"]));
    f.graph("control", json!({"action":"resume"})).unwrap();
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["review"]["status"],
        "permission_required"
    );
    f.decide_graph("once");
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["review"]["status"],
        "waiting"
    );
}

#[test]
fn pause_gates_direct_load_and_both_run_selectors_before_reservation() {
    let f = Fixture::graph_task();
    f.insert("task", "other", json!({"work":{}}));
    f.graph("control", json!({"action":"pause"})).unwrap();
    for args in [
        vec!["load", "--task", "task", "--spool", "unavailable"],
        vec![
            "run",
            "--task",
            "task",
            "--spool",
            "unavailable",
            "--",
            "true",
        ],
        vec![
            "run",
            "--task",
            "task",
            "--instance",
            "unavailable",
            "--",
            "true",
        ],
    ] {
        let out = f.run(&args);
        assert!(String::from_utf8_lossy(&out.stderr).contains("Task paused"));
    }
    assert_eq!(
        f.call(&["permission", "pending", "--task", "task"]),
        json!([])
    );
    let count: usize =
        f.db.query_row(
            "SELECT count(*) FROM objects WHERE kind IN ('instance','run','model_call')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    let different = f.run(&["run", "--task", "other", "--spool", "missing", "--", "true"]);
    assert!(String::from_utf8_lossy(&different.stderr).contains("Load the spool first"));
    f.graph("control", json!({"action":"resume"})).unwrap();
    let out = f.run(&["run", "--task", "task", "--spool", "missing", "--", "true"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("Load the spool first"));
}

#[test]
fn repeated_stop_includes_direct_runs_and_leaves_other_tasks_alone() {
    let f = Fixture::graph_task();
    for (task, id) in [("task", "direct"), ("other", "independent")] {
        f.insert("run", id, json!({"id":id,"task":task,"status":"running"}));
    }
    for _ in 0..2 {
        let stopped = f.graph("control", json!({"action":"stop"})).unwrap();
        assert_eq!(stopped["cancelling"], json!(["direct"]));
        assert_eq!(
            f.call(&["status", "--task", "task", "direct"])["status"],
            "cancel_pending"
        );
    }
    let status:String=f.db.query_row("SELECT json_extract(body,'$.status') FROM objects WHERE kind='run' AND id='independent'",[],|r|r.get(0)).unwrap();
    assert_eq!(status, "running");
}

#[test]
fn submission_records_only_an_authorized_instruction_and_replays_once() {
    let f = Fixture::graph_task();
    f.graph("control", json!({"action":"pause"})).unwrap();
    let sent = f
        .graph("instruct", json!({"revision":0,"text":"Keep the original"}))
        .unwrap();
    let args = json!({"base_revision":0,"idempotency_key":"redirect","instruction_cursor":sent["cursor"],"nodes":[Fixture::surface("replacement")]});
    for cursor in [json!(1), json!(0), json!(999), json!("2")] {
        let mut bad = args.clone();
        bad["instruction_cursor"] = cursor;
        assert!(f
            .graph("submit", bad)
            .unwrap_err()
            .contains("current task and authority"));
    }
    let accepted = f.graph("submit", args.clone()).unwrap();
    assert_eq!(f.graph("submit", args.clone()).unwrap(), accepted);
    let state = f.graph("state", json!({})).unwrap();
    let linked: Vec<_> = state["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["instruction_cursor"] == sent["cursor"])
        .collect();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0]["nodes"], json!(["replacement"]));
    let policy = f.root.join("policy");
    fs::write(&policy, r#"{"default":"ask","effects":[]}"#).unwrap();
    f.call(&[
        "permission",
        "policy",
        "--task",
        "task",
        "--file",
        policy.to_str().unwrap(),
    ]);
    let mut stale = args;
    stale["idempotency_key"] = json!("another");
    stale["base_revision"] = json!(1);
    assert!(f
        .graph("submit", stale)
        .unwrap_err()
        .contains("current task and authority"));
}

#[test]
fn direct_start_queued_on_the_task_lock_observes_the_committed_pause() {
    use std::process::Stdio;
    let f = Fixture::graph_task();
    f.graph("control", json!({"action":"resume"})).unwrap();
    let lock = fs::File::open(f.root.join("locks/task")).unwrap();
    lock.lock().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_an"))
        .env("AN_HOME", &f.root)
        .args(["run", "--task", "task", "--spool", "missing", "--", "true"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(child.try_wait().unwrap().is_none());
    f.db.execute("UPDATE objects SET body=json_set(body,'$.control','paused') WHERE kind='graph' AND id='task'",[]).unwrap();
    drop(lock);
    let result = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&result.stderr).contains("Task paused"));
}

#[test]
fn redirect_closes_an_already_viewed_decision() {
    let f = Fixture::graph_task();
    f.graph(
        "submit",
        json!({"base_revision":0,"idempotency_key":"start","nodes":[Fixture::surface("choice")]}),
    )
    .unwrap();
    f.decide_graph("once");
    let state = f.graph("state", json!({})).unwrap();
    let request = json!({"node":"choice","request":state["activity"]["choice"]["id"]});
    f.graph("viewed", request.clone()).unwrap();
    let sent = f
        .graph("instruct", json!({"revision":1,"text":"Keep the original"}))
        .unwrap();
    f.graph("submit",json!({"base_revision":1,"instruction_cursor":sent["cursor"],"idempotency_key":"redirect","supersede":["choice"],"nodes":[]})).unwrap();
    let mut old = request;
    old["value"] = json!({"choice":"A"});
    assert!(f.graph("respond", old).is_err());
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["choice"]["status"],
        "superseded"
    );
}

#[test]
fn stop_reconciles_idle_instance_for_an_indeterminate_attempt() {
    let f = Fixture::graph_task();
    f.insert("instance","idle", json!({"id":"idle","name":"an-idle","task":"task","intent":"stop","status":"cleanup_pending","active":null}));
    f.insert(
        "run",
        "attempt",
        json!({"id":"attempt","instance":"idle","task":"task","status":"unknown"}),
    );
    f.insert(
        "graph",
        "task",
        json!({"revision":1,"events":[],"nodes":{"job":{"id":"job","kind":"run"}},
        "activity":{"job":{"id":"attempt","run":"attempt","status":"unknown"}}}),
    );
    let stopped = f.graph("control", json!({"action":"stop"})).unwrap();
    assert_eq!(stopped["cancelling"], json!(["attempt"]));
    // Simulate runtime confirmation on a later connection.
    f.insert(
        "instance",
        "idle",
        json!({"id":"idle","task":"task","active":null,"intent":"stop","status":"unloaded"}),
    );
    assert_eq!(
        f.graph("state", json!({})).unwrap()["activity"]["job"]["status"],
        "cancelled"
    );
}

#[test]
fn agent_messages_reach_the_person_once_and_never_answer_surfaces() {
    let mut f = Fixture::graph_task();
    let asked = f
        .graph("instruct", json!({"text":"做一张海报","revision":0}))
        .unwrap();
    let cursor = asked["cursor"].clone();
    let say = json!({"text":"收到，先出两张草图","purpose":"progress","reply_to":cursor,"idempotency_key":"m1"});
    let first = f.graph("say", say.clone()).unwrap();
    assert_eq!(first, f.graph("say", say.clone()).unwrap(), "idempotent");
    let mut changed = say.clone();
    changed["text"] = json!("different");
    assert!(f.graph("say", changed).unwrap_err().contains("Idempotency"));

    let view = f.graph("view", json!({})).unwrap();
    let messages: Vec<_> = view["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "message")
        .collect();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["text"], "收到，先出两张草图");
    assert_eq!(messages[0]["reply_to"], cursor);

    for bad in [
        json!({"text":"  ","idempotency_key":"e"}),
        json!({"text":"x","purpose":"decide","idempotency_key":"p"}),
        json!({"text":"x","reply_to":999,"idempotency_key":"r"}),
    ] {
        assert!(f.graph("say", bad).is_err());
    }
    // Without the turn the agent cannot speak, and the timeline hides messages.
    f.turn = None;
    assert!(f
        .graph("say", json!({"text":"x","idempotency_key":"n"}))
        .is_err());
    assert!(!f
        .graph("view", json!({}))
        .unwrap()
        .to_string()
        .contains("先出两张草图"));
}
