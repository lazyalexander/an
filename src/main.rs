mod artifacts;
mod graph;
mod instances;
mod manifest;
mod mcp;
mod permission;
mod runtime;
mod store;
mod transfer;
mod workspace;

use anyhow::{ensure, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::{fs::File, io::Read, path::PathBuf};
#[derive(Parser)]
#[command(
    name = "an",
    about = "Task-scoped MCP gateway and OCI spool scheduler",
    after_help = "Additional commands: workspace, task, deliveries, permission, mcp, server. Run an <command> --help for options."
)]
struct Cli {
    #[command(subcommand)]
    command: Core,
}
#[derive(Args)]
struct Task {
    #[arg(long)]
    task: String,
}
#[derive(Args)]
struct RunId {
    #[command(flatten)]
    target: Task,
    run: String,
    #[arg(long)]
    request_id: Option<String>,
}
#[derive(Subcommand)]
enum Core {
    Check {
        path: PathBuf,
    },
    Register {
        image: String,
        #[arg(long)]
        archive: Option<PathBuf>,
    },
    Export {
        delivery: String,
        destination: PathBuf,
        /// Active task in the published graph delivery's workspace.
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
    },
    /// Task-host graph interface. Read arguments as JSON on stdin.
    Graph {
        #[command(flatten)]
        target: Task,
        action: String,
    },
    State(Task),
    Load {
        #[command(flatten)]
        target: Task,
        #[arg(long)]
        spool: String,
    },
    Unload {
        #[command(flatten)]
        target: Task,
        #[arg(long)]
        instance: String,
    },
    Run {
        #[command(flatten)]
        target: Task,
        #[arg(
            long,
            required_unless_present = "instance",
            conflicts_with = "instance"
        )]
        spool: Option<String>,
        #[arg(long)]
        instance: Option<String>,
        #[arg(long)]
        input: Option<String>,
        /// JSON object mapping manifest port names to immutable source references.
        #[arg(long, conflicts_with = "input")]
        inputs: Option<Value>,
        #[arg(long, requires = "inputs")]
        input_requests: Option<Value>,
        #[arg(long = "output")]
        outputs: Vec<String>,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(trailing_var_arg = true)]
        argv: Vec<String>,
    },
    Publish(RunId),
    Status(RunId),
    Log(RunId),
    Cancel(RunId),
    Handoff {
        #[command(flatten)]
        run: RunId,
        #[arg(long)]
        recipient_spool: String,
    },
    #[command(name = "_worker", hide = true)]
    Worker {
        id: String,
        fd: i32,
    },
}
fn display(value: Value) {
    println!("{value}")
}
fn call(t: Task, op: &str, a: Value) -> Result<()> {
    display(instances::dispatch(&t.task, op, &a)?);
    Ok(())
}
fn run() -> Result<()> {
    let op = std::env::args().nth(1).unwrap_or_default();
    match op.as_str() {
        "mcp" | "server" => return tokio::runtime::Runtime::new()?.block_on(mcp::execute()),
        "workspace" | "task" | "deliveries" => {
            display(workspace::execute()?);
            return Ok(());
        }
        "permission" => {
            display(permission::execute()?);
            return Ok(());
        }
        _ => {}
    }
    match Cli::parse().command {
        Core::Worker { id, fd } => instances::worker(&id, fd),
        Core::Check { path } => {
            ensure!(path.is_file(), "Expected regular manifest file");
            let mut bytes = vec![];
            File::open(path)?
                .take(manifest::LIMIT + 1)
                .read_to_end(&mut bytes)?;
            println!("{}", manifest::parse(&bytes)?["name"].as_str().unwrap());
            Ok(())
        }
        Core::Register { image, archive } => {
            if let Some(p) = archive {
                runtime::apple(
                    &[
                        "image",
                        "load",
                        "--input",
                        p.to_str().context("Invalid archive path")?,
                    ],
                    None,
                )?;
            }
            println!("{}", runtime::register(&store::connect()?, &image)?);
            Ok(())
        }
        Core::Export {
            delivery,
            destination,
            task,
            request_id,
        } => {
            let mut db = store::connect()?;
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let (owner, revision, origins, details) = if let Some(group) =
                store::find(&tx, "graph_delivery", &delivery)?
            {
                let owner = group["task"].as_str().context("Invalid graph delivery")?;
                let target = task.as_deref().unwrap_or(owner);
                ensure!(
                    target == owner
                        || (group["workspace"].is_string()
                            && store::get(&tx, "task", target)?["workspace"] == group["workspace"]),
                    "Delivery outside workspace"
                );
                let origins: std::collections::BTreeSet<_> = group["bindings"]
                    .as_object()
                    .context("Missing delivery bindings")?
                    .values()
                    .map(graph::binding_origin)
                    .collect();
                ensure!(!origins.is_empty(), "Missing delivery origins");
                (
                    target.to_owned(),
                    group["revision"].clone(),
                    origins.into_iter().collect::<Vec<_>>(),
                    json!({"bindings":group["bindings"]}),
                )
            } else {
                let r = store::get(&tx, "run", &delivery)?;
                ensure!(r["status"] == "delivered", "Expected published delivery");
                let owner = r["task"].as_str().context("Invalid delivery")?;
                ensure!(
                    task.as_deref().is_none_or(|t| t == owner),
                    "Run outside task"
                );
                (
                    owner.to_owned(),
                    r["output"].clone(),
                    vec![r["spool"].as_str().context("Invalid spool")?.to_owned()],
                    json!({}),
                )
            };
            let dir = std::env::current_dir()?.join(destination);
            let dest = if dir.exists() {
                dir.canonicalize()?
            } else {
                dir.parent()
                    .context("Invalid export destination")?
                    .canonicalize()?
                    .join(dir.file_name().context("Invalid export destination")?)
            };
            let flows = origins
                .iter()
                .map(|origin| json!(["export", origin, dest]))
                .collect::<Vec<_>>();
            if let Some(p) = permission::gate_flows(
                &tx,
                &owner,
                "export",
                json!([delivery, revision, dest]),
                details,
                request_id.as_deref(),
                &flows,
            )? {
                display(p);
            } else {
                artifacts::stage(
                    &dest,
                    &store::get(
                        &tx,
                        "revision",
                        revision.as_str().context("Missing revision")?,
                    )?,
                    false,
                )?;
            }
            tx.commit()?;
            Ok(())
        }
        Core::Graph { target, action } => {
            let args = serde_json::from_reader(std::io::stdin().lock())?;
            display(graph::host(&target.task, &action, &args)?);
            Ok(())
        }
        Core::State(t) => call(t, &op, json!({})),
        Core::Load { target, spool } => call(target, &op, json!({"spool":spool})),
        Core::Unload { target, instance } => call(target, &op, json!({"instance":instance})),
        Core::Run {
            target,
            spool,
            instance,
            input,
            inputs,
            input_requests,
            outputs,
            request_id,
            argv,
        } => call(
            target,
            &op,
            json!({"spool":spool,"instance":instance,"input":input,"inputs":inputs,"input_requests":input_requests,"outputs":outputs,"request_id":request_id,"argv":argv}),
        ),
        Core::Cancel(r) | Core::Publish(r) | Core::Status(r) | Core::Log(r) => call(
            r.target,
            &op,
            json!({"run":r.run,"request_id":r.request_id}),
        ),
        Core::Handoff {
            run: r,
            recipient_spool,
        } => call(
            r.target,
            &op,
            json!({"run":r.run,"request_id":r.request_id,"recipient_spool":recipient_spool}),
        ),
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("an: {e:#}");
        std::process::exit(1)
    }
}
