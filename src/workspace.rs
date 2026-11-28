mod binding;
pub(crate) mod inbox;
mod policy;
mod registry;
mod task;
use crate::store::*;
use anyhow::{anyhow, ensure, Result};
pub(crate) use binding::{guard, resolve, Selector};
use clap::{Args, Parser, Subcommand};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
pub(crate) use task::resume_graph;

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Workspace {
        #[command(subcommand)]
        action: Workspace,
    },
    Task {
        #[command(subcommand)]
        action: Task,
    },
    Deliveries {
        #[arg(long)]
        task: String,
        #[arg(long, default_value_t = 0)]
        after: i64,
    },
}
#[derive(Subcommand)]
enum Workspace {
    Review {
        workspace: String,
        #[arg(value_enum)]
        decision: crate::provenance::review::Disposition,
        request: String,
    },
    Create {
        /// Bind the optional primary agent identity when creating this workspace.
        #[command(flatten)]
        primary: Selector,
        #[arg(long)]
        review_owner_digest: Option<String>,
        #[command(flatten)]
        grants: Grants,
    },
    Bind {
        workspace: String,
        #[command(flatten)]
        selector: Selector,
    },
    Policy {
        #[command(subcommand)]
        action: Policy,
    },
}
#[derive(Subcommand)]
enum Policy {
    Show {
        workspace: String,
    },
    Set {
        workspace: String,
        #[arg(long)]
        file: PathBuf,
    },
}
#[derive(Subcommand)]
enum Task {
    New {
        #[command(flatten)]
        grants: Grants,
        #[command(flatten)]
        selector: Selector,
        #[arg(long)]
        input: Vec<PathBuf>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, requires = "model")]
        model_calls: Option<u64>,
    },
    Resume {
        delivery: String,
        #[arg(long)]
        editable: Vec<String>,
    },
}
#[derive(Args, Default)]
struct Grants {
    #[arg(long)]
    spool: Vec<String>,
    #[arg(long)]
    editable: Vec<String>,
    #[arg(long)]
    allow_tool: Vec<String>,
}
fn config(args: Value) -> Result<Value> {
    let env = json!({"AN_HOME":root()?});
    Ok(json!({"an":{"command":std::env::current_exe()?,"args":args,"env":env}}))
}
fn resolve_grants(db: &Connection, g: &Grants) -> Result<(Vec<String>, Value)> {
    ensure!(
        g.editable.iter().all(|e| g.spool.contains(e)),
        "Editable spool must be granted"
    );
    let mut images = vec![];
    for r in &g.spool {
        images.push(
            get(db, "image", r)?
                .as_str()
                .ok_or_else(|| anyhow!("Invalid image record"))?
                .to_owned(),
        );
    }
    let mut tools = json!({});
    for t in &g.allow_tool {
        let (server, tool) = t
            .split_once('/')
            .ok_or_else(|| anyhow!("Expected SERVER/TOOL"))?;
        ensure!(!tool.is_empty(), "Expected tool name");
        tools[t] = get(db, "server", server)?["revision"].clone();
    }
    Ok((images, tools))
}
pub fn execute() -> Result<Value> {
    let cli = Cli::parse();
    let mut db = connect()?;
    let mut lock = None;
    if let Command::Task {
        action: Task::New { selector, .. },
    } = &cli.command
    {
        if let Some(id) = selector.identity()? {
            lock = Some(guard(&id, true)?);
        }
    }
    if let Command::Workspace {
        action:
            Workspace::Review {
                workspace,
                decision,
                request,
            },
    } = cli.command
    {
        let key: String = serde_json::from_reader(std::io::stdin().lock())?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let receipt = crate::provenance::review::settle(&tx, &workspace, decision, &request, &key)?;
        tx.commit()?;
        return Ok(receipt);
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = match cli.command {
        Command::Workspace {
            action: Workspace::Review { .. },
        } => unreachable!(),
        Command::Deliveries { task, after } => deliveries(&tx, &task, after)?,
        Command::Workspace {
            action:
                Workspace::Create {
                    grants,
                    review_owner_digest,
                    primary,
                },
        } => registry::create(&tx, grants, primary, review_owner_digest)?,
        Command::Workspace {
            action:
                Workspace::Bind {
                    workspace,
                    selector,
                },
        } => binding::bind(&tx, workspace, selector)?,
        Command::Workspace {
            action: Workspace::Policy { action },
        } => {
            let (id, rules) = match action {
                Policy::Show { workspace } => (workspace, None),
                Policy::Set { workspace, file } => (
                    workspace,
                    Some(serde_json::from_slice::<Value>(&fs::read(file)?)?),
                ),
            };
            policy::apply(&tx, id, rules)?
        }
        Command::Task {
            action:
                Task::New {
                    grants,
                    selector,
                    input,
                    model,
                    model_calls,
                },
        } => task::create(&tx, grants, selector, input, model, model_calls)?,
        Command::Task {
            action: Task::Resume { delivery, editable },
        } => task::resume(&tx, delivery, editable)?,
    };
    tx.commit()?;
    drop(lock);
    Ok(result)
}
