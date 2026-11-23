"use strict";

// Macro UI skeleton. Instance labels (title, stage names, drawer sections)
// come from the manifest fetched once at boot; everything else renders from
// the state JSON polled every second. The only fixed copy is the page's own
// chrome: health banner strings and decision button names.

const KEY = new URLSearchParams(location.search).get("key") || "";
const POLL_MS = 1000;

const DOTS = {done: "✓", running: "●", waiting: "○", failed: "✕"};

let manifest = null;

const $ = (id) => document.getElementById(id);

function withKey(url) {
  const separator = url.includes("?") ? "&" : "?";
  return url + separator + "key=" + encodeURIComponent(KEY);
}

async function fetchJson(url) {
  const response = await fetch(withKey(url));
  if (!response.ok) throw new Error(url + " → HTTP " + response.status);
  return response.json();
}

// Every command goes through the instance action port; the page knows no
// route beyond /api/action/<name>.
async function post(action, body) {
  const response = await fetch(withKey("/api/action/" + encodeURIComponent(action)), {
    method: "POST",
    headers: {"Content-Type": "application/json"},
    body: JSON.stringify(body || {}),
  });
  if (!response.ok) {
    const error = await response.json().catch(() => ({}));
    throw new Error(error.error || ("HTTP " + response.status));
  }
}

// A button press: POST, show a refusal (e.g. "archive not pushed yet") in the
// decision bar, and re-fetch state immediately either way.
async function act(action, body) {
  const note = $("decision-note");
  try {
    await post(action, body);
    note.textContent = "";
  } catch (err) {
    note.textContent = err.message;
  }
  await refresh();
}

function render(state) {
  renderHeader(state);
  renderStages(state);
  renderPreview(state);
  renderDecisions(state);
  renderArchive(state);
  renderDrawer(state);
}

function renderHeader(state) {
  $("phase").textContent = state.phase;
  const active = (state.surfaces || []).find((surface) => surface.active);
  $("surface").textContent = active
    ? (active.app ? active.label + " · " + active.app : active.label)
    : "";
  const documentChannel = state.document;
  const documentCopy = manifest.document || {};
  $("document").textContent = documentChannel
    ? (documentChannel.dirty
       ? (documentCopy.dirty || "")
       : (documentCopy.saved || ""))
    : "";
  const button = $("lock-toggle");
  const locked = state.phase === "LOCKED";
  button.textContent = locked ? "解锁" : "锁定";
  // Locking freezes the pose stream, so it is offered only while it is live.
  // Instances without a pose channel (no state.health.pose) are not gated.
  const poseChannel = state.health && state.health.pose;
  const adapterChannel = state.health && state.health.adapter;
  const busy = !!state.pending_decision;
  button.disabled = busy || (!locked && !!poseChannel && !poseChannel.ok);
  // Unlocking persists the live canvas, so the instance makes it an
  // adapter-acked action; locking only freezes a pose. When the adapter is
  // offline there is no canvas at risk, so the decision is forced.
  button.onclick = () => act(
    locked ? "unlock" : "lock",
    locked ? {force: !(adapterChannel && adapterChannel.ok)} : {});
}

function renderStages(state) {
  const byId = {};
  for (const stage of state.stages || []) byId[stage.id] = stage;
  $("stages").replaceChildren(...manifest.stages.map((def) => {
    const stage = byId[def.id] || {status: "waiting", detail: null};
    const node = document.createElement("span");
    node.className = "stage " + (DOTS[stage.status] ? stage.status : "waiting");
    const dot = document.createElement("span");
    dot.className = "dot";
    dot.textContent = DOTS[stage.status] || "○";
    const label = document.createElement("span");
    label.textContent = def.label;
    node.append(dot, label);
    if (stage.detail !== null && stage.detail !== undefined) {
      const detail = document.createElement("span");
      detail.className = "stage-detail";
      detail.textContent = String(stage.detail);
      node.append(detail);
    }
    return node;
  }));
}

function renderPreview(state) {
  const img = $("preview-img");
  const empty = $("preview-empty");
  let url = null;
  if (state.preview) {
    // The preview file is overwritten in place; the cache-buster makes each
    // poll pick up the newest write.
    url = withKey(state.preview) + "&t=" + Date.now();
  } else if (state.archive && state.archive.length) {
    url = withKey(state.archive[state.archive.length - 1].thumb);
  }
  img.onerror = () => { img.hidden = true; empty.hidden = false; };
  img.hidden = !url;
  empty.hidden = !!url;
  if (url) img.src = url;
}

function pendingActions(state) {
  const kinds = new Set((state.pending || []).map((item) => item.kind));
  const actions = [];
  for (const decision of manifest.decisions || []) {
    if (!kinds.has(decision.kind)) continue;
    for (const action of decision.actions || []) actions.push(action);
  }
  return actions;
}

function renderDecisions(state) {
  const bar = $("decision-bar");
  const note = $("decision-note");
  for (const button of bar.querySelectorAll("button[data-action]")) {
    button.remove();
  }
  const pendingDecision = state.pending_decision;
  if (!pendingDecision) {
    for (const action of pendingActions(state)) {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.action = action.action;
      button.textContent = action.label;
      if (action.style) button.className = action.style;
      button.onclick = () => {
        const body = Object.assign({}, action.body || {});
        if (action.force_when_adapter_down) {
          const adapter = state.health && state.health.adapter;
          body.force = !(adapter && adapter.ok);
        }
        act(action.action, body);
      };
      bar.insertBefore(button, note);
    }
  }
  // While the adapter is persisting the canvas, show why nothing happens;
  // ordinary refusals from act() stay visible until the next press.
  if (pendingDecision) {
    note.textContent = manifest.pending_decision || "正在保存…";
    note.dataset.pending = "1";
  } else if (note.dataset.pending === "1") {
    note.textContent = "";
    delete note.dataset.pending;
  }

  const health = state.health || {};
  const copy = manifest.health || {};
  const problems = [];
  for (const [channel, value] of Object.entries(health)) {
    if (value && value.ok === false) problems.push(copy[channel] || channel);
  }
  $("health-banner").hidden = problems.length === 0;
  $("health-text").textContent = problems.join(" · ");
  const retry = (manifest.decisions || [])
    .flatMap((decision) => decision.actions || [])
    .find((action) => action.role === "retry");
  const failedSession = state.session && state.session.state === "failed";
  const bannerRetry = $("banner-retry");
  bannerRetry.hidden = !(health.pipeline && !health.pipeline.ok
    && failedSession && retry);
  if (retry) {
    bannerRetry.textContent = retry.label;
    bannerRetry.onclick = () => act(retry.action, {});
  }
}

function renderArchive(state) {
  $("archive-title").textContent = (manifest.archive && manifest.archive.label) || "";
  const rows = (state.archive || []).map((version) => {
    const row = document.createElement("li");
    row.className = "archive-row";
    const thumb = document.createElement("img");
    thumb.className = "thumb";
    thumb.alt = "";
    thumb.onerror = () => { thumb.hidden = true; };
    thumb.src = withKey(version.thumb);
    const meta = document.createElement("div");
    meta.className = "meta";
    const id = document.createElement("strong");
    id.textContent = version.id;
    const sub = document.createElement("span");
    sub.className = "sub";
    const lineage = version.restored_from ? " · ← " + version.restored_from : "";
    sub.textContent = version.created + " · " + version.status + lineage;
    meta.append(id, sub);
    const restore = manifest.restore;
    if (restore) {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = restore.label;
      button.onclick = () => act(restore.action, {version: version.id});
      row.append(button);
    }
    row.prepend(thumb, meta);
    return row;
  });
  $("archive-list").replaceChildren(...rows);
}

function renderDrawer(state) {
  const sections = manifest.drawer.map((def) => {
    const section = document.createElement("section");
    const heading = document.createElement("h3");
    heading.textContent = def.label;
    const list = document.createElement("ul");
    const items = (state.drawer && state.drawer[def.id]) || [];
    for (const item of items) {
      const li = document.createElement("li");
      // poses/snapshots are summary strings; failures are ledger entries.
      li.textContent = typeof item === "string"
        ? item : (item.summary || JSON.stringify(item));
      list.append(li);
    }
    if (items.length === 0) {
      const li = document.createElement("li");
      li.className = "empty";
      li.textContent = "—";
      list.append(li);
    }
    section.append(heading, list);
    return section;
  });
  $("drawer-sections").replaceChildren(...sections);
}

async function refresh() {
  try {
    render(await fetchJson("/api/macro/state"));
  } catch (err) {
    // Keep the last good frame; an unreachable orchestrator cannot be
    // reported by a page the orchestrator serves.
    console.error(err);
  }
}

async function init() {
  try {
    manifest = await fetchJson("/api/macro/manifest");
  } catch (err) {
    $("phase").textContent = String(err);
    return;
  }
  document.title = manifest.title;
  $("title").textContent = manifest.title;
  await refresh();
  setInterval(refresh, POLL_MS);
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", init);
} else {
  init();
}
