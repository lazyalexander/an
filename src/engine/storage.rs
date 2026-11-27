//! A fixed-capacity filesystem backs the three bind mounts of each new instance.
//! The image and mount point are siblings, outside the guest's directory tree.
use anyhow::{ensure, Context, Result};
use std::{
    fs::{self, File},
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::Command,
};

fn run(command: &mut Command, task_lock: &File) -> Result<()> {
    super::apple::inherited(command, task_lock);
    let result = command.output()?;
    ensure!(
        result.status.success(),
        "Instance disk: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(())
}

fn mounted(directory: &Path) -> Result<bool> {
    match fs::symlink_metadata(directory.join("data")) {
        Ok(data) => {
            ensure!(data.is_dir(), "Invalid instance disk mount point");
            Ok(data.dev() != fs::metadata(directory)?.dev())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub fn data(directory: &Path) -> Result<PathBuf> {
    ensure!(
        mounted(directory)?,
        "Instance disk is offline; unload and reload the instance"
    );
    Ok(directory.join("data"))
}

pub fn create(directory: &Path, mib: u64, lock: &File) -> Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let mount = directory.join("data");
    fs::create_dir(&mount)?;
    let image = directory.join("storage.sparseimage");
    run(
        Command::new("/usr/bin/hdiutil")
            .args([
                "create",
                "-type",
                "SPARSE",
                "-size",
                &format!("{mib}m"),
                "-fs",
                "Case-sensitive HFS+",
                "-volname",
                "AnInstance",
            ])
            .arg(&image),
        lock,
    )?;
    run(
        Command::new("/usr/bin/hdiutil")
            .args(["attach", "-nobrowse", "-mountpoint"])
            .arg(&mount)
            .arg(&image),
        lock,
    )?;
    data(directory)
}

pub fn remove(directory: &Path, lock: &File) -> Result<()> {
    if mounted(directory)? {
        run(
            Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(directory.join("data")),
            lock,
        )?;
        ensure!(!mounted(directory)?, "Instance disk still mounted");
    }
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("Removing instance directory"),
    }
}
