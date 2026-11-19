use crate::{
    permission,
    store::*,
    workspace::{guard, resolve, Selector},
};
use anyhow::{anyhow, ensure, Context, Result};
use clap::{Parser, Subcommand};
use rmcp::{
    model::*,
    service::{RequestContext, RunningService},
    transport::{stdio, TokioChildProcess},
    RoleClient, RoleServer, ServerHandler, ServiceExt,
};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{process::Command, time::timeout};
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    Mcp {
        #[arg(long)]
        task: Option<String>,
        #[command(flatten)]
        selector: Selector,
    },
    Server {
        #[command(subcommand)]
        action: Server,
    },
}
#[derive(Subcommand)]
enum Server {
    Register {
        name: String,
        #[arg(trailing_var_arg = true, required = true)]
        argv: Vec<String>,
    },
    Tools {
        name: String,
    },
}
type Peer = RunningService<RoleClient, ()>;
const DEADLINE: Duration = Duration::from_secs(30);
async fn peer(config: &Value) -> Result<(Peer, Vec<Tool>)> {
    let mut cmd = Command::new(config["command"].as_str().context("Expected command")?);
    cmd.args(serde_json::from_value::<Vec<String>>(
        config["args"].clone(),
    )?)
    .current_dir(config["cwd"].as_str().context("Expected cwd")?)
    .env_clear();
    for key in ["HOME", "PATH", "USER", "SHELL", "LOGNAME", "TERM", "TMPDIR"] {
        if let Some(v) = env::var_os(key) {
            cmd.env(key, v);
        }
    }
    let service = timeout(DEADLINE, ().serve(TokioChildProcess::new(cmd)?)).await??;
    let tools = timeout(DEADLINE, service.list_all_tools()).await??;
    Ok((service, tools))
}
struct Route {
    server: String,
    raw: String,
    revision: Value,
    tool: Tool,
}
struct Gateway {
    task: String,
    binding: Option<String>,
    bootstrap: bool,
    builtins: BTreeMap<String, Tool>,
    routes: BTreeMap<String, Route>,
    peers: BTreeMap<String, Peer>,
    states: std::sync::Mutex<BTreeMap<String, String>>,
}
impl Gateway {
    fn binding_guard(&self) -> Result<Option<fs::File>> {
        if let Some(key) = &self.binding {
            let lock = guard(key, false)?;
            ensure!(
                get(&connect()?, "binding", key)?["task"] == self.task,
                "Task binding changed; reconnect"
            );
            Ok(Some(lock))
        } else {
            Ok(None)
        }
    }
    fn allowed(&self, db: &Connection, r: &Route) -> Result<bool> {
        Ok(
            get(db, "task", &self.task)?["tools"][format!("{}/{}", r.server, r.raw)] == r.revision
                && get(db, "server", &r.server)?["revision"] == r.revision,
        )
    }
    async fn invoke(&self, request: CallToolRequestParams) -> Result<CallToolResult> {
        let _binding = self.binding_guard()?;
        let name = request.name.as_ref();
        let mut args = Value::Object(request.arguments.unwrap_or_default());
        if self.bootstrap {
            ensure!(name == "an_state", "Active host turn required");
            return Ok(text(
                json!({"task":self.task,"workspace":get(&connect()?,"task",&self.task)?["workspace"]}),
            ));
        }
        permission::current(&connect()?, &self.task)?;
        if let Some(tool) = self.builtins.get(name) {
            validate_builtin(&tool.input_schema, &args)?;
            let op = &name[3..];
            let mut value = if op == "deliveries" {
                deliveries(&connect()?, &self.task, args["after"].as_i64().unwrap_or(0))?
            } else {
                crate::instances::dispatch(&self.task, op, &args)?
            };
            if op == "state" {
                for (name, peer) in &self.peers {
                    if !matches!(
                        timeout(
                            Duration::from_secs(3),
                            peer.send_request(ClientRequest::PingRequest(PingRequest::default()))
                        )
                        .await,
                        Ok(Ok(_))
                    ) {
                        self.states
                            .lock()
                            .unwrap()
                            .insert(name.clone(), "disconnected".into());
                    }
                }
                value["connections"] = json!(*self.states.lock().unwrap());
                let db = connect()?;
                let mut stmt=db.prepare("SELECT body FROM objects WHERE kind='call' AND json_extract(body,'$.task')=? ORDER BY rowid DESC LIMIT 50")?;
                let mut calls = vec![];
                let mut rows = stmt.query([&self.task])?;
                while let Some(row) = rows.next()? {
                    calls.push(serde_json::from_str::<Value>(&row.get::<_, String>(0)?)?);
                }
                value["calls"] = json!(calls);
            }
            return Ok(text(value));
        }
        let route = self.routes.get(name).context("Tool outside task grant")?;
        ensure!(
            self.states
                .lock()
                .unwrap()
                .get(&route.server)
                .is_some_and(|s| s == "connected"),
            "Downstream disconnected; reconnect"
        );
        let approval = args.as_object_mut().unwrap().remove("_an_request_id");
        let id = uid();
        let mut audit = json!({"id":id,"task":self.task,"server":route.server,"tool":route.raw,"status":"unknown","start":now()});
        {
            let mut db = connect()?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            ensure!(self.allowed(&tx, route)?, "Tool grant is stale");
            let pending = permission::gate(
                &tx,
                &self.task,
                "mcp",
                json!([route.server, route.raw, route.revision, args]),
                json!({}),
                approval
                    .as_ref()
                    .map(|v| v.as_str().context("Invalid request ID"))
                    .transpose()?,
                None,
            )?;
            if let Some(p) = pending {
                tx.commit()?;
                return Ok(text(p));
            }
            put(&tx, "call", &id, &audit)?;
            tx.commit()?;
        }
        let forwarded = CallToolRequestParams::new(route.raw.clone())
            .with_arguments(args.as_object().unwrap().clone());
        let result = timeout(DEADLINE, self.peers[&route.server].call_tool(forwarded)).await;
        audit["end"] = json!(now());
        match result {
            Ok(Ok(result)) => {
                audit["status"] = json!(if result.is_error == Some(true) {
                    "tool_error"
                } else {
                    "completed"
                });
                put(&connect()?, "call", &id, &audit)?;
                Ok(result)
            }
            Ok(Err(rmcp::service::ServiceError::McpError(e)))
                if !matches!(e.code.0, 408 | -32000) =>
            {
                audit["status"] = json!("protocol_error");
                put(&connect()?, "call", &id, &audit)?;
                Err(anyhow!("Downstream MCP error: {e}"))
            }
            _ => {
                self.states
                    .lock()
                    .unwrap()
                    .insert(route.server.clone(), "disconnected".into());
                put(&connect()?, "call", &id, &audit)?;
                Err(anyhow!("Downstream outcome unknown; call {id}; no replay"))
            }
        }
    }
}
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn text(v: Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(v.to_string())])
}
fn validate_builtin(schema: &JsonObject, args: &Value) -> Result<()> {
    for key in schema["required"].as_array().into_iter().flatten() {
        ensure!(args.get(key.as_str().unwrap()).is_some(), "Missing {key}");
    }
    for (key, v) in args.as_object().unwrap() {
        let def = schema["properties"]
            .get(key)
            .context("Unknown tool argument")?;
        ensure!(
            match def["type"].as_str() {
                Some("string") => v.is_string(),
                Some("integer") => v.as_u64().is_some_and(|n| i64::try_from(n).is_ok()),
                Some("array") => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)
                    && a.len() >= def["minItems"].as_u64().unwrap_or(0) as usize),
                _ => false,
            },
            "Invalid {key}"
        );
    }
    Ok(())
}
impl ServerHandler for Gateway {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let _lock = self
            .binding_guard()
            .map_err(|e| ErrorData::invalid_request(e.to_string(), None))?;
        let mut tools = self.builtins.values().cloned().collect::<Vec<_>>();
        let db = connect().map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        for r in self.routes.values() {
            if self.allowed(&db, r).unwrap_or(false)
                && self
                    .states
                    .lock()
                    .unwrap()
                    .get(&r.server)
                    .is_some_and(|s| s == "connected")
            {
                tools.push(r.tool.clone());
            }
        }
        Ok(ListToolsResult::with_all_items(tools))
    }
    async fn call_tool(
        &self,
        req: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        Ok(self
            .invoke(req)
            .await
            .unwrap_or_else(|e| CallToolResult::error(vec![ContentBlock::text(e.to_string())]))
            .into())
    }
}
fn executable(path: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    path.is_file()
        && std::ffi::CString::new(path.as_os_str().as_bytes())
            .is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::X_OK) } == 0)
}

pub async fn execute() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Server {
            action: Server::Register { name, argv },
        } => {
            ensure!(
                !name.is_empty()
                    && name.len() <= 32
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "Invalid server name"
            );
            let path = if argv[0].contains('/') {
                PathBuf::from(&argv[0])
            } else {
                env::split_paths(&env::var_os("PATH").unwrap_or_default())
                    .map(|p| p.join(&argv[0]))
                    .find(|p| executable(p))
                    .context("Executable not found")?
            };
            let cwd = env::current_dir()?;
            let path = cwd.join(path);
            ensure!(executable(&path), "Executable not found");
            let config = json!({"command":path,"args":argv[1..],"cwd":cwd});
            let mut db = connect()?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !find(&tx, "server", &name)?.is_some_and(|s| s["config"] == config) {
                put(
                    &tx,
                    "server",
                    &name,
                    &json!({"revision":uid(),"config":config}),
                )?;
            }
            tx.commit()?;
        }
        Cmd::Server {
            action: Server::Tools { name },
        } => {
            let (_, tools) = peer(&get(&connect()?, "server", &name)?["config"]).await?;
            println!("{}", json!(tools));
        }
        Cmd::Mcp { task, selector } => {
            let binding = selector.identity()?;
            ensure!(
                task.is_some() != binding.is_some(),
                "Select task or bound identity"
            );
            let task = if let Some(t) = task {
                t
            } else {
                let key = binding.as_deref().unwrap();
                let _guard = guard(key, false)?;
                let mut db = connect()?;
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let id = resolve(&tx, key, false)?;
                tx.commit()?;
                id
            };
            let record = get(&connect()?, "task", &task)?;
            let bootstrap = env::var("AN_TURN").unwrap_or_default().is_empty();
            let mut builtins = BTreeMap::new();
            for tool in serde_json::from_str::<Vec<Tool>>(include_str!("tools.json"))? {
                if (tool.name != "an_deliveries" || record["workspace"].is_string())
                    && (!bootstrap || tool.name == "an_state")
                {
                    builtins.insert(tool.name.to_string(), tool);
                }
            }
            let mut routes = BTreeMap::new();
            let mut peers = BTreeMap::new();
            let mut states = BTreeMap::new();
            if let Some(grants) = record["tools"].as_object().filter(|_| !bootstrap) {
                let mut discovery = tokio::task::JoinSet::new();
                for server in grants
                    .keys()
                    .filter_map(|k| k.split_once('/').map(|p| p.0))
                    .collect::<BTreeSet<_>>()
                {
                    let cfg = get(&connect()?, "server", server)?;
                    let rev = &cfg["revision"];
                    if !grants
                        .iter()
                        .any(|(k, v)| k.split_once('/').is_some_and(|p| p.0 == server) && v == rev)
                    {
                        states.insert(server.into(), "requires authorization".into());
                        continue;
                    }
                    let server = server.to_owned();
                    discovery.spawn(async move {
                        let result = peer(&cfg["config"]).await;
                        (server, cfg["revision"].clone(), result)
                    });
                }
                while let Some(result) = discovery.join_next().await {
                    let (server, rev, result) = result?;
                    let server = server.as_str();
                    let Ok((service, tools)) = result else {
                        states.insert(server.into(), "disconnected".into());
                        continue;
                    };
                    for mut tool in tools {
                        let raw = tool.name.to_string();
                        let reference = format!("{server}/{raw}");
                        if grants.get(&reference) != Some(&rev) {
                            continue;
                        }
                        let schema = std::sync::Arc::make_mut(&mut tool.input_schema);
                        let Some(props) = schema
                            .entry("properties")
                            .or_insert(json!({}))
                            .as_object_mut()
                        else {
                            continue;
                        };
                        if props.contains_key("_an_request_id") {
                            continue;
                        }
                        props.insert("_an_request_id".into(), json!({"type":"string"}));
                        let prefix: String = format!("{server}_{raw}")
                            .chars()
                            .take(43)
                            .map(|c| {
                                if c.is_ascii_alphanumeric() || c == '-' {
                                    c
                                } else {
                                    '_'
                                }
                            })
                            .collect();
                        let alias = format!("d_{prefix}_{}", &hash(reference.as_bytes())[..12]);
                        tool.name = alias.clone().into();
                        tool.output_schema = None;
                        routes.insert(
                            alias,
                            Route {
                                server: server.into(),
                                raw,
                                revision: rev.clone(),
                                tool,
                            },
                        );
                    }
                    states.insert(server.into(), "connected".into());
                    peers.insert(server.into(), service);
                }
            }
            Gateway {
                task,
                binding,
                bootstrap,
                builtins,
                routes,
                peers,
                states: states.into(),
            }
            .serve(stdio())
            .await?
            .waiting()
            .await?;
        }
    }
    Ok(())
}
