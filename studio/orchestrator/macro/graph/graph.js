"use strict";
const el = id => document.getElementById(id);
const key = location.hash.slice(1) || sessionStorage.getItem("studio-key");
if (location.hash) {
  sessionStorage.setItem("studio-key", key);
  history.replaceState(null, "", location.pathname);
}
let view, selection, instanceKey, channel, frame, panelTimer, panelEpoch = 0;
// All user-facing text and timing come from studio/config via /config.json.
let strings = {}, settings = {};
function t(key, vars = {}) {
  const text = key.split(".").reduce((node, part) => node?.[part], strings);
  return typeof text === "string" ? text.replace(/\{(\w+)\}/g, (_, name) => vars[name] ?? "") : key;
}
const statusLabel = status => strings.status?.[status] ?? status;
async function api(action, body = {}) {
  const response = await fetch("/api/" + action, {method:"POST",
    headers:{"Authorization":"Bearer " + key, "Content-Type":"application/json"},
    body:JSON.stringify(body)});
  const value = await response.json();
  if (!response.ok) throw Error(value.error || response.statusText);
  return value;
}
function button(label, action) {
  const b = document.createElement("button");
  b.textContent = label;
  b.onclick = () => Promise.resolve().then(action).catch(error => status(error.message));
  return b;
}
function status(message) { el("status").textContent = message; }
let flushPanel = () => Promise.resolve();
function clearPanel() {
  flushPanel = () => Promise.resolve();
  panelEpoch++;
  el("content").inert = false;
  clearTimeout(panelTimer);
  channel?.close();
  channel = frame = null;
  el("content").replaceChildren();
  el("controls").replaceChildren();
  el("trace").replaceChildren();
  el("sources").open = false;
}
function filesPanel(files, parent = el("content")) {
  for (const [name, group] of Object.entries(files)) {
    for (const [filename, entry] of Object.entries(group)) {
      const a = document.createElement("a");
      a.href = "data:application/octet-stream;base64," + (entry.data || entry);
      a.download = filename;
      a.textContent = name + " / " + filename;
      const row = document.createElement("p");
      row.append(a);
      parent.append(row);
    }
  }
}
async function basicPanel(pkg, ref, epoch) {
  if (epoch !== panelEpoch) return;
  filesPanel(pkg.files);
  const form = document.createElement("form");
  const inputs = new Map();
  for (const [key, rule] of Object.entries(pkg.response.properties)) {
    const label = document.createElement("label");
    label.textContent = key + " ";
    const options = rule.enum || (rule.type === "boolean" ? [true,false] : null);
    const input = document.createElement(options ? "select" : "input");
    if (options) {
      for (const value of options) input.add(new Option(String(value), JSON.stringify(value)));
    } else if (rule.type !== "string") {
      input.type = "number";
      input.step = rule.type === "integer" ? "1" : "any";
      input.required = true;
      if (rule.minimum !== undefined) input.min = rule.minimum;
      if (rule.maximum !== undefined) input.max = rule.maximum;
    }
    const decision = pkg.declaration.decision;
    const initial = pkg.draft?.[key] ?? (decision.field === key ? decision.default : undefined);
    if (initial !== undefined) input.value = options ? JSON.stringify(initial) : initial;
    inputs.set(key, () => options ? JSON.parse(input.value) : input.type === "number" ? input.valueAsNumber : input.value);
    label.append(input);
    form.append(label);
  }
  const values = () => Object.fromEntries([...inputs].map(([key,read]) => [key,read()]));
  let writes = Promise.resolve();
  flushPanel = () => writes;
  form.oninput = () => {
    const value = values();
    writes = writes.catch(()=>{}).then(()=>api("draft",{...ref,value}));
    writes.catch(error=>status(error.message));
  };
  const submit = document.createElement("button");
  submit.disabled = true;
  submit.textContent = t("panel.submit");
  form.append(submit);
  form.onsubmit = async event => {
    event.preventDefault();
    if (submit.disabled || epoch !== panelEpoch) return;
    try { await writes; await api("respond",{...ref,value:values()}); await update(); }
    catch (error) { status(error.message); }
  };
  el("content").append(form);
  await api("viewed", ref);
  if (epoch === panelEpoch) submit.disabled = false;
}
let customRequest = null;
function mount(pkg, ref, epoch) {
  frame = document.createElement("iframe");
  frame.title = el("title").textContent;
  frame.setAttribute("sandbox", "allow-scripts");
  frame.referrerPolicy = "no-referrer";
  // Opaque origin. The panel receives this request's data and a single port.
  const csp = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; media-src data: blob:; form-action 'none'; base-uri 'none'";
  // The first document contains trusted bootstrap code only. The host transfers
  // the capability before any package code can navigate the frame.
  function bootstrap() {
    addEventListener("message", function start(e) {
      if(e.source!==parent || e.data!=="an:init" || !e.ports[0])return;
      removeEventListener("message",start);
      const port=e.ports[0];
      const fault=message=>port.postMessage({type:"error",message:String(message)});
      addEventListener("error",e=>fault(e.message));
      port.onmessage=({data})=>{
        if(data.type==="flush"){port.postMessage({type:"flushed"});return;}
        if(data.type!=="init"){
          dispatchEvent(new CustomEvent("an-result",{detail:data}));
          return;
        }
        try {
          window.an={files:data.files,draft:data.draft,saveDraft:value=>port.postMessage({type:"draft",value}),
            respond:value=>port.postMessage({type:"respond",value})};
          const doc=new DOMParser().parseFromString(data.html,"text/html");
          const scripts=[...doc.querySelectorAll("script")];
          for(const script of scripts)script.remove();
          const asset=name=>{
            if(!Object.hasOwn(data.assets,name) || typeof data.assets[name]!=="string")throw Error("Missing panel asset "+name);
            return data.assets[name];
          };
          for(const link of doc.querySelectorAll('link[rel="stylesheet"]')){
            const style=doc.createElement("style");style.textContent=asset(link.getAttribute("href"));link.replaceWith(style);
          }
          document.head.append(...doc.head.childNodes);
          document.body.replaceChildren(...doc.body.childNodes);
          for(const original of scripts){
            const script=document.createElement("script");
            script.textContent=original.hasAttribute("src")?asset(original.getAttribute("src")):original.textContent;
            document.body.append(script);
          }
          port.postMessage({type:"ready"});
          dispatchEvent(new Event("an-ready"));
        }catch(e){fault(e.message);}
      };
    });
  }
  frame.srcdoc='<!doctype html><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="'+csp+'"><body><script>('+bootstrap.toString()+')();<\/script></body>';
  const target = frame;
  target.onload = () => {
    if (epoch !== panelEpoch) return;
    target.onload = () => { if(epoch===panelEpoch) panelError(t("panel.left")); };
    const pair = new MessageChannel();
    channel = pair.port1;
    // Keep edits and submission in message order, including a slow draft write.
    let chain = Promise.resolve();
    let flushed;
    flushPanel = () => new Promise(resolve => {
      flushed = resolve;
      pair.port1.postMessage({type:"flush"});
    }).then(()=>chain);
    channel.onmessage = ({data}) => {
      if (epoch !== panelEpoch || !data) return;
      if(data.type==="flushed"){flushed?.();return;}
      if(data.type==="error"){panelError(data.message);return;}
      if (data.type === "ready") {
        clearTimeout(panelTimer);
        if(ref) chain=chain.then(()=>api("viewed",ref)).catch(e=>{if(epoch===panelEpoch)panelError(e.message);});
        return;
      }
      if (!ref || !["draft","respond"].includes(data.type)) return;
      chain = chain.catch(()=>{}).then(async () => {
        if (epoch !== panelEpoch) return;
        const result = await api(data.type, {...ref,value:data.value});
        if (epoch !== panelEpoch) return;
        channel.postMessage({type:"saved",operation:data.type,status:result.status});
        if (data.type === "respond") {status(t("panel.responded"));await update();}
      });
      chain.catch(e => {if(epoch===panelEpoch)status(e.message);});
    };
    target.contentWindow.postMessage("an:init", "*", [pair.port2]);
    channel.postMessage({type:"init",html:pkg.html,assets:pkg.assets,files:pkg.files,draft:pkg.draft});
  };
  panelTimer = setTimeout(() => {if(epoch===panelEpoch)panelError(t("panel.timeout"));},settings.panel_timeout_ms);
  el("content").append(target);
}
function panelError(message) {
  clearPanel();
  status(message);
  if (customRequest && selection?.kind === "node" && view.activity[selection.id]?.id === customRequest) {
    el("controls").append(button(t("panel.basic"), () => {customRequest=null;instanceKey=null;return render();}));
  }
  el("controls").append(button(t("panel.reload"), () => {instanceKey=null;return render();}));
}
function select(kind,id) {selection = {kind,id};return render();}
function nav(items, active, action) {
  return items.map(([id,label])=>{
    const b=button(label,()=>action(id));
    b.setAttribute("aria-current",String(active===id));
    return b;
  });
}
let lastDelivery;
async function render() {
  const activity = Object.entries(view.activity || {});
  const deliveries = view.deliveries || [];
  const localDelivery = deliveries.find(d => d.task === view.task);
  if (localDelivery && localDelivery.id !== lastDelivery) {
    lastDelivery=localDelivery.id; selection={kind:"delivery",id:lastDelivery};
  }
  const pending = activity.find(([,s])=>s.status==="waiting");
  if ((!selection || (selection.kind==="node" && view.activity[selection.id]?.status!=="waiting" &&
      selection.auto)) && pending) selection={kind:"node",id:pending[0],auto:true};
  if (!selection && activity.length) selection={kind:"node",id:activity[0][0],auto:true};
  const title=id=>view.nodes[id]?.title || id;
  el("task").textContent = view.task;
  renderControls();
  el("graph-nodes").replaceChildren(...nav(activity.map(([id,s])=>[id,title(id)+" · "+statusLabel(s.status)]),
    selection?.kind==="node" && selection.id,id=>select("node",id)));
  const active=activity.filter(([id,s])=>!["done","queued","superseded"].includes(s.status) || (selection?.kind==="node" && selection.id===id));
  el("activity").replaceChildren(...nav(active.map(([id,s])=>[id,title(id)+" · "+statusLabel(s.status)]),
    selection?.kind==="node" && selection.id,id=>select("node",id)));
  el("deliveries").replaceChildren(...nav(deliveries.map(d=>[d.id,d.title || d.node]),selection?.kind==="delivery" && selection.id,id=>select("delivery",id)));
  el("permissions").replaceChildren(...view.permissions.map(r=>{
    const article=document.createElement("article");
    article.append(t("permission.request",{operation:r.operation,scope:r.scope.join(" / ")}));
    const details=document.createElement("details"), summary=document.createElement("summary"), detail=document.createElement("pre");
    summary.textContent=t("permission.details");detail.textContent=JSON.stringify(r.details,null,2);details.append(summary,detail);article.append(details);
    for (const [text,decision] of [[t("permission.allow_once"),"once"],[t("permission.deny"),"deny"]]) article.append(button(text,async()=>{
      await api("decide",{request:r.id,decision}); await update();
    }));
    return article;
  }));
  if (!selection) {status(t("task.awaiting_submission"));return;}
  const {kind,id}=selection;
  const s=view.activity[id] || {};
  const current=JSON.stringify([kind,id,s.id,s.status,view.authorized]);
  if (current===instanceKey) return;
  instanceKey=current;
  clearPanel();
  const epoch=panelEpoch;
  el("title").textContent=kind==="delivery" ? (deliveries.find(d=>d.id===id)?.title || deliveries.find(d=>d.id===id)?.node) : title(id);
  status(kind==="delivery"?t("task.delivered"):(s.error || statusLabel(s.status)));
  try {
    if(kind==="delivery" || (s.status==="waiting" && view.authorized)) {
      const ref=kind==="delivery"?null:{node:id,request:s.id};
      const pkg=ref?await api("panel",ref):await api("open",{delivery:id});
      if(epoch!==panelEpoch)return;
      if (ref && pkg.declaration && pkg.html) {
        const custom = customRequest === ref.request;
        const toggle = button(t(custom ? "panel.basic" : "panel.custom"), async () => {
          toggle.disabled = true;
          el("content").inert = true;
          try { await flushPanel(); }
          finally {
            if (epoch === panelEpoch) {
              toggle.disabled = false;
              el("content").inert = false;
            }
          }
          if (epoch !== panelEpoch) return;
          customRequest = custom ? null : ref.request;
          instanceKey = null;
          return render();
        });
        el("controls").append(toggle);
      }
      if (ref && pkg.declaration && customRequest !== ref.request) {
        await basicPanel(pkg,ref,epoch);
      } else if(pkg.html) {
        mount(pkg,ref,epoch);
        if(kind==="delivery"){
          const details=document.createElement("details"),heading=document.createElement("summary");
          heading.textContent=t("task.delivery_files");details.append(heading);filesPanel(pkg.files,details);el("content").append(details);
        }
      }else filesPanel(pkg.files);
      if(kind==="delivery") el("controls").append(button(t("task.resume_from"),async()=>{
        const fresh=await api("resume",{delivery:id});status(t("task.new_task",{task:fresh.task}));
      }));
    }
    if(kind==="node" && !view.closed && !view.authorized) status(t("task.awaiting_authority"));
  }catch(e){if(epoch===panelEpoch)panelError(e.message);}
}
el("sources").ontoggle=async()=>{
  if(!el("sources").open || !selection)return;
  const epoch=panelEpoch;
  try{
    const body=selection.kind==="delivery"?{delivery:selection.id}:{node:selection.id};
    const trace=await api("sources",body);
    if(epoch!==panelEpoch)return;
    const rows=[];
    for(const [id,record] of Object.entries(trace)){
      const d=document.createElement("details"), heading=document.createElement("summary"), pre=document.createElement("pre");
      heading.textContent=id+" · "+statusLabel(record.execution.status);
      pre.textContent=JSON.stringify(record,null,2);
      d.append(heading,pre);
      const types = record.node.kind==="model" ? ["input","output"].filter(k=>record.execution.evidence?.[k] || record.execution.evidence?.available?.includes(k)) : ["code","work","input","log"];
      if(record.execution.evidence) for(const type of types){
        d.append(button(t("evidence."+type),async()=>{
          const value=await api("evidence",{...body,source:id,type});
          if(value.status==="permission_required"){pre.textContent=t("evidence.waiting");await update();}
          else if(typeof value==="string")pre.textContent=value;
          else pre.textContent=Object.entries(value).map(([name,entry])=>{
            const bytes=Uint8Array.from(atob(entry.data || entry),c=>c.charCodeAt(0));
            try{return name+"\n"+new TextDecoder("utf-8",{fatal:true}).decode(bytes);}
            catch{return t("evidence.binary",{name,bytes:bytes.length});}
          }).join("\n\n") || t("evidence.empty");
        }));
      }
      rows.push(d);
    }
    el("trace").replaceChildren(...rows);
  }catch(e){el("trace").textContent=e.message;}
};
// Person controls are executed by An itself, never relayed to the agent.
function renderControls() {
  const paused = view.control === "paused";
  el("control-state").textContent = view.closed ? t("control.closed") : paused ? t("control.paused") : "";
  if (!view.authorized || view.closed) {
    el("instruct").hidden = true;
    return el("task-controls").replaceChildren();
  }
  const control = action => api("control", {action}).then(update);
  el("task-controls").replaceChildren(
    paused ? button(t("control.resume"), () => control("resume")) : button(t("control.pause"), () => control("pause")),
    button(t("control.stop"), () => confirm(t("control.stop_confirm")) && control("stop")));
  el("instruct").hidden = false;
}
let inFlight;
function update(){
  if(!inFlight)inFlight=api("view").then(v=>{view=v;return render();}).catch(e=>status(e.message)).finally(()=>{inFlight=null;});
  return inFlight;
}
// Free text goes to the agent unchanged, tagged with the revision on screen.
// It never answers a waiting panel; controls stay separate buttons.
el("instruct").onsubmit = async event => {
  event.preventDefault();
  const text = el("instruction").value;
  if (!text.trim()) return;
  try {
    const sent = await api("instruct", {text, revision: view.revision ?? 0});
    el("instruction").value = "";
    status(t(sent.stale ? "instruction.stale" : "instruction.sent"));
  } catch (e) { status(e.message); }
};
fetch("/config.json").then(r => r.json()).then(config => {
  strings = config.strings; settings = config.settings;
  document.documentElement.lang = config.locale;
  document.title = t("title");
  for (const node of document.querySelectorAll("[data-t]")) node.textContent = t(node.dataset.t);
  for (const node of document.querySelectorAll("[data-t-label]")) node.setAttribute("aria-label", t(node.dataset.tLabel));
  for (const node of document.querySelectorAll("[data-t-placeholder]")) node.placeholder = t(node.dataset.tPlaceholder);
  update();
  setInterval(update, settings.poll_ms);
}).catch(e => status(e.message));
