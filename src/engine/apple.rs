use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs::File,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Child, Command, Stdio},
};
pub const CONTAINER: &str = "/usr/local/bin/container";
pub fn inherited(command: &mut Command, file: &File) {
    let fd = file.as_raw_fd();
    // Only the child needs the lock across exec; keep CLOEXEC in the parent.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}
fn command() -> Command {
    #[cfg(not(test))]
    let executable = CONTAINER;
    #[cfg(test)]
    let executable = std::env::var_os("AN_TEST_CONTAINER").unwrap_or_else(|| CONTAINER.into());
    Command::new(executable)
}
pub fn invoke(args: &[&str], lock: Option<&File>) -> Result<Vec<u8>> {
    let mut cmd = command();
    cmd.args(args);
    if let Some(f) = lock {
        inherited(&mut cmd, f);
    }
    let out = cmd.output()?;
    ensure!(
        out.status.success(),
        "container {}: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}
pub fn inspect(args: &[&str], lock: Option<&File>) -> Result<Value> {
    let rows: Vec<Value> = serde_json::from_slice(&invoke(args, lock)?)?;
    rows.into_iter()
        .next()
        .context("Missing container inspection")
}
pub fn list(lock: &File) -> Result<Vec<Value>> {
    Ok(serde_json::from_slice(&invoke(
        &["list", "--all", "--format", "json"],
        Some(lock),
    )?)?)
}
pub fn start(
    image: &str,
    id: &str,
    label: &str,
    path: &Path,
    editable: bool,
    platform: &Value,
    lock: &File,
) -> Result<Value> {
    let p = path.to_str().context("Invalid mount path")?;
    ensure!(!p.contains(','), "Unsupported comma in mount path");
    let plat = ["os", "architecture", "variant"]
        .iter()
        .filter_map(|k| platform[*k].as_str())
        .collect::<Vec<_>>()
        .join("/");
    let config = crate::config::load()?;
    let cpus = config.limit("container_cpus");
    let memory = config
        .limit("container_memory_mib")
        .checked_mul(1024 * 1024)
        .context("container_memory_mib exceeds the runtime byte range")?;
    let cpu_arg = cpus.to_string();
    let memory_arg = memory.to_string();
    let mut args = vec![
        "run",
        "--detach",
        "--cpus",
        &cpu_arg,
        "--memory",
        &memory_arg,
        "--platform",
        &plat,
        "--name",
        id,
        "--label",
        label,
        "--network",
        "none",
        "--read-only",
        "-w",
        "/work",
        "--entrypoint",
        "/bin/sleep",
    ];
    let mounts = ["input", "output", "work"].map(|d| {
        format!(
            "type=bind,source={p}/{d},target=/{d}{}",
            if d == "input" || (d == "work" && !editable) {
                ",readonly"
            } else {
                ""
            }
        )
    });
    for m in &mounts {
        args.extend(["--mount", m]);
    }
    args.extend([image, "infinity"]);
    invoke(&args, Some(lock))?;
    let actual = inspect(&["inspect", id], Some(lock))?;
    let resources = &actual["configuration"]["resources"];
    ensure!(resources["cpus"].as_u64() == Some(cpus) && resources["memoryInBytes"].as_u64() == Some(memory),
        "Container compute allocation mismatch: requested {cpus} CPUs and {memory} bytes, observed {resources}");
    Ok(actual)
}

pub fn exec(name: &str, argv: &[String], log: File) -> Result<Child> {
    Ok(command()
        .args(["exec", name])
        .args(argv)
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?)
}
pub fn export(name: &str, lock: &File) -> Result<Child> {
    let mut cmd = command();
    cmd.args(["export", name]).stdout(Stdio::piped());
    inherited(&mut cmd, lock);
    Ok(cmd.spawn()?)
}

pub fn stop(name: &str, lock: &File) -> Result<()> {
    invoke(&["stop", name], Some(lock)).map(drop)
}
pub fn restart(name: &str, lock: &File) -> Result<()> {
    invoke(&["start", name], Some(lock)).map(drop)
}
pub fn delete(name: &str, lock: &File) -> Result<()> {
    invoke(&["delete", name], Some(lock)).map(drop)
}
