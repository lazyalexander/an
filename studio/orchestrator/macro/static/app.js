"use strict";

// Macro UI skeleton. Instance labels (title, stage names, drawer sections)
// come from the manifest fetched once at boot; everything else renders from
// the state JSON polled every second. The only fixed copy is the page's own
// chrome: health banner strings and decision button names.

const KEY = new URLSearchParams(location.search).get("key") || "";
const POLL_MS = 1000;

const DOTS = {done: "✓", running: "●", waiting: "○", failed: "✕"};

// state.pending kinds -> the buttons they surface.
const DECISIONS = {
  adopt_or_rerender: [
    {id: "adopt", path: "/api/decision/adopt"},
    {id: "rerender", path: "/api/decision/rerender"},
  ],
  retry: [
    {id: "retry", path: "/api/decision/retry"},
  ],
};

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

// Decision endpoints are token-gated; the legacy lock/unlock pair is plain
// (the docker channel) and must not carry the key.
async function post(path, body, keyed) {
  const response = await fetch(keyed ? withKey(path) : path, {
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
async function act(path, body, keyed) {
  const note = $("decision-note");
  try {
    await post(path, body, keyed);
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
  const button = $("lock-toggle");
  const locked = state.phase === "LOCKED";
  button.textContent = locked ? "解锁" : "锁定";
  // Locking freezes the pose stream, so it is offered only while it is live.
  button.disabled = !locked && !(state.health && state.health.pose.ok);
  button.onclick = () => act(locked ? "/api/unlock" : "/api/lock", {}, false);
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

function renderDecisions(state) {
  const visible = new Set();
  for (const pending of state.pending || []) {
    for (const def of DECISIONS[pending.kind] || []) visible.add(def.id);
  }
  for (const buttons of Object.values(DECISIONS)) {
    for (const def of buttons) {
      const button = $(def.id);
      button.hidden = !visible.has(def.id);
      button.onclick = () => act(def.path, {}, true);
    }
  }

  const health = state.health || {};
  const problems = [];
  if (health.adapter && !health.adapter.ok) problems.push("Krita 未连接");
  if (health.pipeline && !health.pipeline.ok) problems.push("容器管线不可用");
  if (health.pose && !health.pose.ok) problems.push("位姿流已断开");
  $("health-banner").hidden = problems.length === 0;
  $("health-text").textContent = problems.join(" · ");
  const failedSession = state.session && state.session.state === "failed";
  const bannerRetry = $("banner-retry");
  bannerRetry.hidden = !(health.pipeline && !health.pipeline.ok && failedSession);
  bannerRetry.onclick = () => act("/api/decision/retry", {}, true);
}

function renderArchive(state) {
  const archiveStage = manifest.stages.find((def) => def.id === "archive");
  $("archive-title").textContent = archiveStage ? archiveStage.label : "";
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
    const restore = document.createElement("button");
    restore.type = "button";
    restore.textContent = "恢复";
    restore.onclick = () => act("/api/decision/restore", {version: version.id}, true);
    row.append(thumb, meta, restore);
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
