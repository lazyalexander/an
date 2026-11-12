use crate::plan::{pair, validate_plan, AskReason, PairError, RenderPlan};
use serde::Deserialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Code,
    Dsh,
    Codex,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub dsh: CommandSpec,
    pub codex: CommandSpec,
}

#[derive(Debug)]
pub enum BackendError {
    Ask(AskReason),
    Failed(String),
}

pub fn load_config(root: &Path) -> BackendConfig {
    let path = root.join("backends.toml");
    let Ok(text) = fs::read_to_string(path) else {
        return defaults();
    };
    let Ok(file) = toml::from_str::<ConfigFile>(&text) else {
        return defaults();
    };
    BackendConfig {
        dsh: spec(file.dsh),
        codex: spec(file.codex),
    }
}

pub fn run(
    kind: BackendKind,
    task: &Path,
    config: &BackendConfig,
    description: &str,
    component_count: usize,
    code_calls: &mut u32,
) -> Result<RenderPlan, BackendError> {
    if kind == BackendKind::Code {
        *code_calls += 1;
        return match pair(description, component_count) {
            Ok(plan) => Ok(plan),
            Err(PairError::Ask(reason)) => Err(BackendError::Ask(reason)),
            Err(PairError::InvalidName) => Err(BackendError::Failed(
                "layer name is empty or contains a slash".into(),
            )),
        };
    }
    let command = match kind {
        BackendKind::Dsh => &config.dsh,
        BackendKind::Codex => &config.codex,
        BackendKind::Code => unreachable!(),
    };
    if command.argv.is_empty() {
        return Err(BackendError::Failed("backend command is empty".into()));
    }
    let prompt = format!(
        "Write only a RenderPlan JSON file named plan.json in the current directory. \
         The sentence is: {description}. \
         There are {component_count} stroke components, numbered from the left starting at 0. \
         Each layer has name, color as #rrggbb, and component. \
         Do not paint pixels and do not write any other file."
    );
    fs::write(task.join("prompt.txt"), &prompt).map_err(|e| BackendError::Failed(e.to_string()))?;
    fs::write(task.join("schema.json"), SCHEMA).map_err(|e| BackendError::Failed(e.to_string()))?;
    let mut argv = command.argv.clone();
    argv.push(prompt);
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(task)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BackendError::Failed(e.to_string()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > command.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BackendError::Failed("backend timed out".into()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(BackendError::Failed(error.to_string())),
        }
    };
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if !status.success() {
        return Err(BackendError::Failed(format!(
            "backend exited {}",
            status.code().unwrap_or(-1)
        )));
    }
    let plan = read_plan(task, &stdout)?;
    validate_plan(&plan, component_count).map_err(BackendError::Failed)?;
    Ok(plan)
}

fn read_plan(task: &Path, stdout: &str) -> Result<RenderPlan, BackendError> {
    let path = task.join("plan.json");
    if path.exists() {
        let bytes = fs::read(&path).map_err(|e| BackendError::Failed(e.to_string()))?;
        return serde_json::from_slice(&bytes).map_err(|e| BackendError::Failed(e.to_string()));
    }
    extract_json(stdout).ok_or_else(|| BackendError::Failed("backend returned no plan".into()))
}

fn extract_json(stdout: &str) -> Option<RenderPlan> {
    let bytes = stdout.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        if bytes[start] == b'{' {
            if let Ok(plan) = serde_json::from_slice::<RenderPlan>(&bytes[start..]) {
                return Some(plan);
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout[start..]) {
                if let Ok(plan) = serde_json::from_value(value) {
                    return Some(plan);
                }
            }
        }
        start += 1;
    }
    None
}

fn defaults() -> BackendConfig {
    BackendConfig {
        dsh: CommandSpec {
            argv: vec!["dsh".into(), "headless".into()],
            timeout: Duration::from_secs(120),
        },
        codex: CommandSpec {
            argv: vec![
                "codex".into(),
                "exec".into(),
                "--skip-git-repo-check".into(),
                "--sandbox".into(),
                "workspace-write".into(),
                "-o".into(),
                "last.txt".into(),
            ],
            timeout: Duration::from_secs(120),
        },
    }
}

fn spec(value: CommandToml) -> CommandSpec {
    CommandSpec {
        argv: value.argv,
        timeout: Duration::from_secs(value.timeout_secs),
    }
}

#[derive(Deserialize)]
struct ConfigFile {
    dsh: CommandToml,
    codex: CommandToml,
}

#[derive(Deserialize)]
struct CommandToml {
    argv: Vec<String>,
    timeout_secs: u64,
}

const SCHEMA: &str =
    r#"{"type":"object","required":["layers"],"properties":{"layers":{"type":"array"}}}"#;

pub fn task_dir(root: &Path, attempt: &str) -> PathBuf {
    root.join("state/default/tasks").join(attempt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("an-backend-{}-{}", std::process::id(), unique()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn unique() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    fn script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    fn config(argv: Vec<String>, timeout: Duration) -> BackendConfig {
        let command = CommandSpec { argv, timeout };
        BackendConfig {
            dsh: command.clone(),
            codex: command,
        }
    }

    #[test]
    fn code_pairs_without_a_process() {
        let mut calls = 0;
        let plan = run(
            BackendKind::Code,
            Path::new("."),
            &defaults(),
            "红色轮子",
            1,
            &mut calls,
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(plan.layers[0].name, "轮子");
    }

    #[test]
    fn a_failing_command_does_not_call_code() {
        let root = dir();
        let program = script(&root, "fail.sh", "#!/bin/sh\nexit 1\n");
        let mut calls = 0;
        let error = run(
            BackendKind::Dsh,
            &root,
            &config(vec![program], Duration::from_secs(2)),
            "红色轮子",
            1,
            &mut calls,
        );
        assert!(error.is_err());
        assert_eq!(calls, 0);
    }

    #[test]
    fn invalid_plan_json_is_rejected() {
        let root = dir();
        let program = script(
            &root,
            "bad.sh",
            "#!/bin/sh\nprintf '%s\\n' '{' > plan.json\n",
        );
        let mut calls = 0;
        assert!(run(
            BackendKind::Dsh,
            &root,
            &config(vec![program], Duration::from_secs(2)),
            "红色轮子",
            1,
            &mut calls
        )
        .is_err());
    }

    #[test]
    fn a_plan_file_is_accepted() {
        let root = dir();
        let program = script(
            &root,
            "ok.sh",
            "#!/bin/sh\nprintf '%s\\n' '{\"layers\":[{\"name\":\"轮子\",\"color\":\"#c0392b\",\"component\":0}]}' > plan.json\n",
        );
        let mut calls = 0;
        let plan = run(
            BackendKind::Codex,
            &root,
            &config(vec![program], Duration::from_secs(2)),
            "红色轮子",
            1,
            &mut calls,
        )
        .unwrap();
        assert_eq!(plan.layers[0].name, "轮子");
        assert_eq!(calls, 0);
    }

    #[test]
    fn a_slow_command_is_killed() {
        let root = dir();
        let program = script(&root, "slow.sh", "#!/bin/sh\nsleep 5\n");
        let mut calls = 0;
        let started = Instant::now();
        let error = run(
            BackendKind::Dsh,
            &root,
            &config(vec![program], Duration::from_millis(200)),
            "红色轮子",
            1,
            &mut calls,
        );
        assert!(matches!(error, Err(BackendError::Failed(_))));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(calls, 0);
    }
}
