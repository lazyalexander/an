mod artifacts;
mod instances;
mod manifest;
mod mcp;
mod permission;
mod runtime;
mod store;
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
        #[arg(long)]
        request_id: Option<String>,
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
            request_id,
        } => {
            let mut db = store::connect()?;
            let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let r = store::get(&tx, "run", &delivery)?;
            ensure!(r["status"] == "delivered", "Expected published delivery");
            let dir = std::env::current_dir()?.join(destination);
            let dest = if dir.exists() {
                dir.canonicalize()?
            } else {
                dir.parent()
                    .context("Invalid export destination")?
                    .canonicalize()?
                    .join(dir.file_name().context("Invalid export destination")?)
            };
            if let Some(p) = permission::gate(
                &tx,
                r["task"].as_str().context("Invalid delivery")?,
                "export",
                json!([delivery, r["output"], dest]),
                json!({}),
                request_id.as_deref(),
                Some(json!(["export", r["spool"], dest])),
            )? {
                display(p);
            } else {
                artifacts::stage(
                    &dest,
                    &store::get(
                        &tx,
                        "revision",
                        r["output"].as_str().context("Missing revision")?,
                    )?,
                    false,
                )?;
            }
            tx.commit()?;
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
            outputs,
            request_id,
            argv,
        } => call(
            target,
            &op,
            json!({"spool":spool,"instance":instance,"input":input,"outputs":outputs,"request_id":request_id,"argv":argv}),
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
