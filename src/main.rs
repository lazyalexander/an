mod artifacts;
mod config;
mod decision;
mod engine;
mod graph;
mod instances;
mod intent;
mod manifest;
mod mcp;
mod panels;
mod permission;
mod provenance;
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
        /// Start a new attempt after an unknown outcome.
        #[arg(long)]
        retry: bool,
    },
    /// Task-host graph interface. Read arguments as JSON on stdin.
    Graph {
        #[command(flatten)]
        target: Task,
        action: String,
    },
    Provenance {
        #[command(flatten)]
        target: Task,
        action: String,
    },
    Decision {
        #[command(subcommand)]
        action: decision::Action,
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
    #[command(name = "_model-worker", hide = true)]
    ModelWorker {
        id: String,
        fd: i32,
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
    ensure!(
        !provenance::scoped() || matches!(op.as_str(), "provenance" | "mcp"),
        "Delegation limited to provenance"
    );
    match op.as_str() {
        "task-cursors" => {
            display(permission::cursors(serde_json::from_reader(
                std::io::stdin(),
            )?)?);
            return Ok(());
        }
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
        Core::ModelWorker { id, fd } => graph::model::worker(&id, fd),
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
                engine::apple::invoke(
                    &[
                        "image",
                        "load",
                        "--input",
                        p.to_str().context("Invalid archive path")?,
                    ],
                    None,
                )?;
            }
            println!("{}", runtime::register(&mut store::connect()?, &image)?);
            Ok(())
        }
        Core::Export {
            delivery,
            destination,
            task,
            request_id,
            retry,
        } => {
            display(provenance::export::run(
                &delivery,
                &destination,
                task.as_deref(),
                request_id.as_deref(),
                retry,
            )?);
            Ok(())
        }
        Core::Graph { target, action } => {
            let args = serde_json::from_reader(std::io::stdin().lock())?;
            display(graph::host(&target.task, &action, &args)?);
            Ok(())
        }
        Core::Provenance { target, action } => {
            let args = serde_json::from_reader(std::io::stdin().lock())?;
            display(provenance::execute(&target.task, &action, &args)?);
            Ok(())
        }
        Core::Decision { action } => decision::execute(action),
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
