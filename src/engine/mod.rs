pub(crate) mod apple;
pub(crate) mod capacity;
mod state;
pub(crate) mod storage;
use anyhow::{Context, Result};
use serde_json::{json, Value};
pub(crate) use state::{GuestState, InstanceIntent, InstanceStatus, Settlement, Termination};
use std::fs::File;

pub(crate) fn observe(instance: &mut Value, lock: &File) -> GuestState {
    let guests = match apple::list(lock) {
        Ok(guests) => guests,
        Err(error) => {
            instance["error"] = json!(error.to_string());
            return GuestState::Unknown;
        }
    };
    let Some(guest) = guests.iter().find(|guest| guest["id"] == instance["name"]) else {
        return GuestState::Absent;
    };
    let config = &guest["configuration"];
    let digest = instance["spool"]
        .as_str()
        .and_then(|reference| reference.rsplit_once('@'))
        .map(|(_, digest)| digest);
    if !instance["id"].is_string()
        || config["labels"]["an.instance"] != instance["id"]
        || digest.is_none()
        || config["image"]["descriptor"]["digest"].as_str() != digest
    {
        return GuestState::IdentityMismatch;
    }
    if let Some(resources) = config.get("resources") {
        instance["resources"] =
            json!({"cpus":resources["cpus"],"memory_bytes":resources["memoryInBytes"]});
    }
    GuestState::reported(guest["status"]["state"].as_str())
}

pub(crate) fn settle(
    instance: &mut Value,
    lock: &File,
    mut guest: GuestState,
) -> Result<Settlement> {
    let intent = InstanceIntent::recorded(instance);
    if intent.is_some_and(InstanceIntent::is_shutdown) && guest == GuestState::Running {
        if let Err(error) = apple::stop(
            instance["name"].as_str().context("Missing instance name")?,
            lock,
        ) {
            instance["error"] = json!(error.to_string());
        }
        guest = observe(instance, lock);
    }
    if intent == Some(InstanceIntent::Unload) && guest.can_delete() {
        if let Err(error) = apple::delete(
            instance["name"].as_str().context("Missing instance name")?,
            lock,
        ) {
            instance["error"] = json!(error.to_string());
        }
        guest = observe(instance, lock);
    }
    Ok(Settlement::from_observation(intent, guest))
}
