//! Additional routes can only be proposed in response to a newer user instruction.
use super::*;

pub(super) fn prepare(
    db: &Connection,
    task: &str,
    stamp: &Value,
    existing: &Value,
    spec: &Value,
    routes: &[Value],
) -> Result<(u64, Vec<Value>)> {
    let authority = hash(canonical(stamp).as_bytes());
    let graph = find(db, "graph", task)?;
    let latest = graph
        .as_ref()
        .and_then(|g| g["events"].as_array())
        .and_then(|events| {
            events.iter().rev().find(|e| {
                e["actor"] == "user"
                    && e["kind"] == "instruction"
                    && e["data"]["authority"] == authority
            })
        })
        .and_then(|event| event["cursor"].as_u64())
        .unwrap_or(0);
    if let Some(cursor) = spec.get("instruction_cursor") {
        ensure!(
            latest > 0 && cursor.as_u64() == Some(latest),
            "Expected the latest user instruction from the current turn"
        );
    }
    if existing.is_null() {
        return Ok((latest, Vec::new()));
    }
    ensure!(
        existing["status"] == "decided" && existing["turn_scope"] == true && spec.is_object(),
        "An intent already exists for this turn and policy"
    );
    ensure!(
        spec.get("instruction_cursor").is_some()
            && existing["instruction_cursor"]
                .as_u64()
                .is_some_and(|prior| latest > prior),
        "An additional scope requires a newer user instruction"
    );

    // Keep each original decision and card ID so output version pins and effect
    // records remain bound to the card that the person actually approved.
    let mut previous = existing.clone();
    let history = previous
        .as_object_mut()
        .context("Invalid intent card")?
        .remove("previous");
    let mut history = match history {
        Some(Value::Array(cards)) => cards,
        None => Vec::new(),
        _ => anyhow::bail!("Invalid intent history"),
    };
    history.push(previous);
    let mut count = routes.len();
    for card in &history {
        let old = card["routes"].as_array().context("Invalid intent routes")?;
        count += old.len();
        ensure!(
            !old.iter()
                .any(|r| routes.iter().any(|new| overlaps(r, new))),
            "An earlier card already covers this route"
        );
    }
    ensure!(count <= 128, "A turn scope may contain at most 128 routes");
    Ok((latest, history))
}

// Exact node rules match their source before checking argv, even for a denial.
// Dynamic rules match the actual producer command as well as its output port.
pub(super) fn overlaps(a: &Value, b: &Value) -> bool {
    if ["operation", "origin", "destination"]
        .iter()
        .any(|key| a[key] != b[key])
    {
        return false;
    }
    let (left, right) = (&a["source"], &b["source"]);
    match (
        left["producer_port"].as_str(),
        right["producer_port"].as_str(),
    ) {
        (Some(x), Some(y)) => x == y && a["producer_argv"] == b["producer_argv"],
        (Some(x), None) => right["port"] == x,
        (None, Some(y)) => left["port"] == y,
        (None, None) => left == right,
    }
}
