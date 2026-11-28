use super::inputs::bridge_artifact;
use super::{lock, path, save, string};
use crate::engine::{apple, capacity, observe, storage, GuestState};
use crate::store::{connect, get, put, revision, root};
use crate::{artifacts, permission};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn spawn_worker(id: &str, lease: &File) -> std::io::Result<std::process::Child> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["_worker", id, &lease.as_raw_fd().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    apple::inherited(&mut cmd, lease);
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    cmd.spawn()
}
pub(super) fn reap(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

pub fn worker(id: &str, fd: i32) -> Result<()> {
    // The descriptor is the inherited run lease. Do not leak it into container exec.
    ensure!(
        fd >= 3 && unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
        "Missing worker lease"
    );
    let mut db = connect()?;
    let task = string(&get(&db, "run", id)?, "task")?.to_owned();
    let worker_lock = lock(&db, &task)?;
    let r = get(&db, "run", id)?;
    ensure!(
        r["worker_pid"].as_u64() == Some(u64::from(std::process::id())),
        "Run belongs to another worker"
    );
    let instance = string(&r, "instance")?.to_owned();
    let mut attempt = WorkerRun {
        task: &task,
        instance: &instance,
        id,
        run: r,
        guest_settled: true,
    };
    if let Err(error) = attempt.execute(&mut db, worker_lock) {
        attempt.record_failure(&mut db, error)?;
    }
    if attempt.run["graph_bindings"].is_object() {
        crate::graph::wake(&task)?;
    }
    Ok(())
}

struct WorkerRun<'a> {
    task: &'a str,
    instance: &'a str,
    id: &'a str,
    run: Value,
    // False between guest exec and confirmed termination. An ambiguous exit retains its slot.
    guest_settled: bool,
}

impl WorkerRun<'_> {
    fn execute(&mut self, db: &mut Connection, worker_lock: File) -> Result<()> {
        let config = crate::config::load()?;
        let Some((mut child, log)) = self.start(db, worker_lock, config.limit("run_log_bytes"))?
        else {
            return Ok(());
        };
        // The task lock is released while the guest runs. Recheck ownership on settlement.
        let code = child.wait()?;
        self.run["exited_at"] = json!(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64());
        self.run["exit_code"] = json!(code.code());
        self.settle(db, code, log, &config)
    }

    fn start(
        &mut self,
        db: &mut Connection,
        worker_lock: File,
        log_limit: u64,
    ) -> Result<Option<(std::process::Child, File)>> {
        let task_lock = if self.run["capacity_managed"] == true {
            let Some(lock) = capacity::wait(db, self.task, self.instance, self.id, worker_lock)?
            else {
                return Ok(None);
            };
            lock
        } else {
            worker_lock
        };
        let mut instance = get(db, "instance", self.instance)?;
        if instance["active"] != self.id || instance["intent"] != "load" {
            return Ok(None);
        }
        ensure!(
            observe(&mut instance, &task_lock) == GuestState::Running,
            "Cannot verify running instance"
        );
        let directory = storage::data(&path(&instance)?)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !self.run["input_stamp"].is_null() || !self.run["source"].is_null() {
            let authority = permission::stamp(&tx, self.task);
            if self.run["graph_bindings"].is_object()
                && authority.as_ref().ok() != Some(&self.run["input_stamp"])
            {
                if self.run["capacity_managed"] == true {
                    capacity::release(&tx, self.id)?;
                }
                self.run["status"] = json!("authorization_expired");
                instance["active"] = Value::Null;
                save(&tx, "run", &self.run)?;
                save(&tx, "instance", &instance)?;
                tx.commit()?;
                return Ok(None);
            }
            ensure!(
                self.run["input_stamp"] == authority?,
                "Input authority changed"
            );
        }
        let t = get(&tx, "task", self.task)?;
        let sources = self.run["inputs"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
            .chain(std::iter::once(&self.run["source"]))
            .filter_map(Value::as_str);
        for source in sources.filter(|s| s.starts_with("artifact-") && t["inputs"].get(s).is_none())
        {
            bridge_artifact(&tx, self.task, &t, source)?;
        }
        artifacts::stage(&directory.join("output"), &json!({}), true)?;
        artifacts::stage(
            &directory.join("input"),
            &get(&tx, "revision", string(&self.run, "input")?)?,
            true,
        )?;
        tx.commit()?;
        artifacts::stage(
            &directory.join("work"),
            &get(db, "revision", string(&self.run, "work")?)?,
            true,
        )?;
        let log = File::create(root()?.join("runs").join(self.id).join("log"))?;
        let argv: Vec<String> = serde_json::from_value(self.run["argv"].clone())?;
        let child = apple::exec(
            string(&instance, "name")?,
            &argv,
            log.try_clone()?,
            log_limit.checked_add(1).context("Invalid run_log_bytes")?,
        )?;
        self.guest_settled = false;
        self.run["started_at"] = json!(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64());
        save(db, "run", &self.run)?;
        Ok(Some((child, log)))
    }

    fn settle(
        &mut self,
        db: &mut Connection,
        code: std::process::ExitStatus,
        log: File,
        config: &crate::config::Config,
    ) -> Result<()> {
        let task_lock = lock(db, self.task)?;
        let mut instance = get(db, "instance", self.instance)?;
        if instance["active"] != self.id || instance["intent"] != "load" {
            return Ok(());
        }
        ensure!(
            observe(&mut instance, &task_lock) == GuestState::Running,
            "Cannot verify instance at settlement"
        );
        apple::stop(string(&instance, "name")?, &task_lock)?;
        ensure!(
            observe(&mut instance, &task_lock) == GuestState::Stopped,
            "Cannot confirm guest termination"
        );
        self.guest_settled = true;
        apple::restart(string(&instance, "name")?, &task_lock)?;
        ensure!(
            observe(&mut instance, &task_lock) == GuestState::Running,
            "Instance restart not confirmed"
        );
        let log_limit = config.limit("run_log_bytes");
        if log.metadata()?.len() > log_limit {
            log.set_len(log_limit)?;
            bail!("Run log budget exceeded ({log_limit} bytes)");
        }
        ensure!(code.success(), "Guest command exited {code}");
        let directory = storage::data(&path(&instance)?)?;
        let mut budget = artifacts::OutputBudget::new(config.limit("run_output_bytes"));
        let names: Vec<String> = serde_json::from_value(self.run["outputs"].clone())?;
        let files = budget.declared(&directory.join("output"), &names)?;
        let work = if self.run["editable"] == true {
            Some(budget.snapshot(&directory.join("work"))?)
        } else {
            None
        };
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if files == json!({}) {
            self.run["status"] = json!("completed");
        } else {
            self.run["status"] = json!("awaiting_publication");
            self.run["candidate"] = json!(revision(&tx, &files)?);
        }
        if let Some(work) = work {
            let mut t = get(&tx, "task", self.task)?;
            self.run["result_work"] = json!(revision(&tx, &work)?);
            t["work"][string(&instance, "spool")?] = self.run["result_work"].clone();
            put(&tx, "task", self.task, &t)?;
        }
        if self.run["capacity_managed"] == true {
            capacity::release(&tx, self.id)?;
        }
        instance["active"] = Value::Null;
        save(&tx, "run", &self.run)?;
        save(&tx, "instance", &instance)?;
        tx.commit()?;
        Ok(())
    }

    fn record_failure(&mut self, db: &mut Connection, error: anyhow::Error) -> Result<()> {
        let _guard = lock(db, self.task)?;
        let mut instance = get(db, "instance", self.instance)?;
        if instance["active"] == self.id && instance["intent"] == "load" {
            self.run["status"] = json!(if self.guest_settled {
                "failed"
            } else {
                "unknown"
            });
            self.run["error"] = json!(error.to_string());
            if self.guest_settled
                && self.run["editable"] == true
                && instance["disk_mib"].is_number()
            {
                let restore = || -> Result<()> {
                    let directory = storage::data(&path(&instance)?)?;
                    artifacts::stage(&directory.join("output"), &json!({}), true)?;
                    artifacts::stage(
                        &directory.join("work"),
                        &get(db, "revision", string(&self.run, "work")?)?,
                        true,
                    )
                };
                if let Err(e) = restore() {
                    self.guest_settled = false;
                    instance["error"] = json!(e.to_string());
                }
            }
            if !self.guest_settled {
                instance["intent"] = json!("stop");
                instance["status"] = json!("cleanup_pending");
            }
            instance["active"] = Value::Null;
            let tx = db.transaction()?;
            if self.run["capacity_managed"] == true {
                if self.guest_settled {
                    capacity::release(&tx, self.id)?;
                } else {
                    instance["unsettled_capacity"] = json!(self.id);
                }
            }
            save(&tx, "run", &self.run)?;
            save(&tx, "instance", &instance)?;
            tx.commit()?;
        }
        Ok(())
    }
}
