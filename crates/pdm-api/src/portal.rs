//! Minimal Phase-1 web file browser served at `/`.
//!
//! A single self-contained page styled after the Onshape Explorer: recently
//! modified cards up top, folder tiles, and an items table/grid with model
//! thumbnails and revision badges. Connect with an API token, browse projects
//! and items, upload/check-in versions, check out/in, and download. No
//! external assets, no build step.

use axum::response::{Html, IntoResponse};

pub async fn default_index() -> impl IntoResponse {
    Html(PAGE)
}

const PAGE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Foundry PDM</title>
<style>
  :root { --bg:#10151c; --panel:#1a2230; --line:#2a3648; --fg:#d7e0ec; --dim:#8494a8;
          --acc:#5aa9ff; --ok:#4caf7d; --err:#e05d5d; --warm:#e0a53f;
          --mono:ui-monospace,Menlo,Consolas,monospace; }
  * { box-sizing:border-box; }
  body { margin:0; background:var(--bg); color:var(--fg); font:14px/1.5 system-ui,sans-serif; }
  header { background:var(--panel); border-bottom:1px solid var(--line); padding:10px 16px;
           display:flex; align-items:center; gap:12px; position:sticky; top:0; z-index:5; }
  header h1 { font-size:15px; margin:0; font-weight:600; }
  header .dot { width:9px; height:9px; border-radius:50%; background:var(--err); }
  header .dot.on { background:var(--ok); }
  main { padding:16px; max-width:1200px; margin:0 auto; }
  .bar { display:flex; flex-wrap:wrap; gap:8px; margin-bottom:14px; align-items:center; }
  input, select, button { background:var(--panel); color:var(--fg); border:1px solid var(--line);
           border-radius:6px; padding:7px 10px; font:inherit; }
  button { cursor:pointer; background:#223049; }
  button:hover { background:#2a3b5c; }
  button:disabled { opacity:.45; cursor:default; }
  button.danger { color:var(--err); }
  button.danger:hover { background:#3a2530; }
  button.kebab { width:30px; padding:7px 4px; font-size:14px; line-height:1; color:var(--dim);
                 text-align:center; }
  button.kebab:hover { color:var(--fg); }
  .kebabmenu { position:fixed; z-index:60; background:var(--panel); border:1px solid var(--line);
               border-radius:8px; box-shadow:0 10px 30px rgba(0,0,0,.5); min-width:176px;
               padding:4px; font-size:13px; }
  .kebabmenu .ki { padding:6px 10px; border-radius:5px; cursor:pointer; }
  .kebabmenu .ki:hover { background:#223049; }
  .kebabmenu .ki.danger { color:var(--err); }
  .kebabmenu .sep { height:1px; background:var(--line); margin:4px 6px; }
  .arch-badge { display:inline-block; padding:0 5px; margin-left:6px; border:1px solid var(--warm);
               color:var(--warm); border-radius:4px; font:10px var(--mono); vertical-align:middle; }
  label.dim { color:var(--dim); font-size:12px; }
  table { width:100%; border-collapse:collapse; background:var(--panel); border:1px solid var(--line);
          border-radius:8px; overflow:hidden; }
  th, td { text-align:left; padding:8px 10px; border-bottom:1px solid var(--line); font-size:13px; }
  th { background:#151d29; color:var(--dim); font-weight:600; text-transform:uppercase; font-size:11px; }
  tr:last-child td { border-bottom:none; }
  .muted { color:var(--dim); } .mono { font-family:var(--mono); font-size:12px; }
  .thumb { width:40px; height:40px; border-radius:4px;
           border:1px solid var(--line); background:#0d1118; vertical-align:middle; position:relative; }
  .thumb-sm { width:32px; height:32px; border-radius:4px;
              border:1px solid var(--line); background:#0d1118; flex:0 0 auto; position:relative; }
  .thumb-lg { width:96px; height:96px; border-radius:6px;
              border:1px solid var(--line); background:#0d1118; display:block; margin:0 auto; position:relative; }
  .thumb img, .thumb-sm img, .thumb-lg img { width:100%; height:100%; object-fit:cover;
              display:block; border-radius:inherit; }
  .thumb::after, .thumb-sm::after { content:"⬢"; position:absolute; inset:0; display:flex;
              align-items:center; justify-content:center; color:#3a485e; font-size:11px; }
  .thumb-lg::after { content:"⬢"; position:absolute; inset:0; display:flex;
              align-items:center; justify-content:center; color:#3a485e; font-size:18px; }
  .thumb:has(> img)::after, .thumb-sm:has(> img)::after, .thumb-lg:has(> img)::after { content:none; }
  .badge { display:inline-block; padding:0 5px; margin-left:6px; border:1px solid var(--line);
           color:var(--dim); border-radius:4px; font:10px var(--mono); vertical-align:middle;
           letter-spacing:.03em; }
  #buildinfo { font-size:11px; color:var(--dim); }
  .lock { color:var(--warm); font-size:12px; }
  .err { color:var(--err); white-space:pre-wrap; background:#2a1d22; border:1px solid var(--err);
         padding:8px 10px; border-radius:6px; margin:10px 0; font-family:var(--mono); font-size:12px; }
  .ok { color:var(--ok); }
  .row { display:flex; gap:10px; align-items:center; flex-wrap:wrap; margin:6px 0; }
  h2 { font-size:13px; color:var(--dim); text-transform:uppercase; letter-spacing:.05em;
       margin:20px 0 8px; font-weight:600; }
  .section-head { display:flex; align-items:center; gap:10px; margin:20px 0 8px; }
  .section-head h2 { margin:0; }
  .grow { flex:1; }
  .ver td { vertical-align:top; }
  .hidden { display:none; }
  .rev { display:inline-block; padding:1px 5px; margin-left:6px; border:1px solid var(--acc);
         color:var(--acc); border-radius:4px; font:11px var(--mono); vertical-align:middle; }
  .rev.wip { border-color:var(--line); color:var(--dim); }
  .recent { display:flex; gap:12px; overflow-x:auto; padding:2px 0 12px; }
  .recent-card { min-width:180px; width:196px; flex:0 0 auto; background:var(--panel);
                 border:1px solid var(--line); border-radius:8px; padding:10px; cursor:pointer; }
  .recent-card:hover { border-color:var(--acc); }
  .rc-name { font-size:14px; font-weight:600; margin:8px 0 2px; white-space:nowrap;
             overflow:hidden; text-overflow:ellipsis; }
  .rc-meta { font-size:11px; color:var(--dim); white-space:nowrap; overflow:hidden;
             text-overflow:ellipsis; }
  .folders { display:grid; grid-template-columns:repeat(auto-fill, minmax(150px,1fr)); gap:10px; }
  .folder-tile { background:var(--panel); border:1px solid var(--line); border-radius:8px;
                 padding:14px 10px; text-align:center; cursor:pointer; font-size:13px; }
  .folder-tile:hover { border-color:var(--acc); }
  .folder-tile.active { border-color:var(--acc); background:#1d2b42; }
  .ficon { font-size:20px; display:block; margin-bottom:6px; }
  .chips { display:flex; gap:6px; }
  .chip { background:var(--panel); border:1px solid var(--line); color:var(--dim);
          border-radius:999px; padding:3px 12px; font-size:12px; cursor:pointer; user-select:none; }
  .chip:hover { border-color:var(--acc); }
  .chip.active { color:var(--fg); border-color:var(--acc); background:#1d2b42; }
  .viewswitch { display:flex; gap:4px; }
  .viewswitch button { padding:4px 10px; font-size:14px; line-height:1; }
  .viewswitch button.on { border-color:var(--acc); color:var(--acc); background:#1d2b42; }
  .grid { display:grid; grid-template-columns:repeat(auto-fill, minmax(190px,1fr)); gap:10px; }
  .grid-card { background:var(--panel); border:1px solid var(--line); border-radius:8px;
               padding:10px; text-align:center; cursor:pointer; }
  .grid-card:hover { border-color:var(--acc); }
  .gc-name { font-size:13px; font-weight:600; margin-top:8px; white-space:nowrap;
             overflow:hidden; text-overflow:ellipsis; }
  .gc-pn { font-size:11px; color:var(--dim); font-family:var(--mono); margin-top:2px; }
  .modal-backdrop { position:fixed; inset:0; background:rgba(5,8,12,.65); z-index:50;
                    display:flex; align-items:flex-start; justify-content:center; padding-top:12vh; }
  .modal { background:var(--panel); border:1px solid var(--line); border-radius:10px; min-width:380px;
           max-width:540px; box-shadow:0 18px 50px rgba(0,0,0,.5); }
  .modal-head { display:flex; align-items:center; padding:12px 16px; border-bottom:1px solid var(--line); }
  .modal-head h3 { margin:0; font-size:14px; font-weight:600; flex:1; }
  .modal-x { background:none; border:none; color:var(--dim); font-size:15px; padding:2px 8px; }
  .modal-body { padding:14px 16px; }
  .modal-field { margin-bottom:12px; }
  .modal-field label { display:block; color:var(--dim); font-size:12px; margin-bottom:4px; }
  .modal-field input, .modal-field select { width:100%; }
   .modal-foot { display:flex; justify-content:flex-end; gap:8px; padding:12px 16px;
                 border-top:1px solid var(--line); }
   .auth { min-height:calc(100vh - 120px); display:flex; align-items:flex-start;
           justify-content:center; padding-top:9vh; }
   .auth-card { background:var(--panel); border:1px solid var(--line); border-radius:12px;
                width:400px; max-width:94vw; padding:26px 28px;
                box-shadow:0 14px 44px rgba(0,0,0,.4); }
   .auth-logo { font-size:18px; font-weight:700; letter-spacing:.02em; }
   .auth-logo .mark { color:var(--acc); margin-right:8px; }
   .auth-sub { color:var(--dim); font-size:12.5px; margin:4px 0 20px; }
   .auth h3 { font-size:12px; text-transform:uppercase; letter-spacing:.05em; color:var(--dim);
              margin:0 0 8px; font-weight:600; }
   .auth .f { margin-bottom:10px; }
   .auth-card input { width:100%; }
   .auth-card button { width:100%; }
   .auth-card button.primary { background:#2f4a77; font-weight:600; }
   .auth-card button.primary:hover { background:#3a5a8c; }
   .divider { display:flex; align-items:center; gap:10px; color:var(--dim); font-size:11px;
              margin:16px 0; text-transform:uppercase; letter-spacing:.08em; }
   .divider:before, .divider:after { content:""; flex:1; height:1px; background:var(--line); }
   .tokshow { margin-top:10px; }
   .auth-help { font-size:11.5px; margin-top:14px; }
</style>
</head>
<body>
<header>
  <span class="dot" id="dot"></span>
  <h1>Foundry PDM</h1>
  <span class="muted" id="who" style="font-size:12px;"></span>
  <span id="buildinfo"></span>
  <div style="flex:1"></div>
  <button id="account" class="hidden">Account</button>
  <button id="signout" class="hidden">Sign out</button>
  <button id="relog" class="hidden">Disconnect</button>
</header>
<main>
  <div id="conn">
    <div class="auth">
      <div class="auth-card">
        <div class="auth-logo"><span class="mark">⬢</span>Foundry PDM</div>
        <p class="auth-sub">Product data vault for your engineering team</p>
        <h3>Sign in</h3>
        <div class="f"><input id="login_email" placeholder="email" autocomplete="email"></div>
        <div class="f"><input id="login_password" type="password" placeholder="password" autocomplete="current-password"></div>
        <div class="f"><input id="login_org" placeholder="organization (only if your email exists in several)" autocomplete="off"></div>
        <button id="signin" class="primary">Sign in</button>
        <div id="tokshow" class="tokshow hidden">
          <div class="f">
            <div class="row" style="margin:0;">
              <span class="muted" style="font-size:12px; white-space:nowrap;">your token</span>
              <span id="tokval" class="mono" style="user-select:all; flex:1; overflow:hidden; text-overflow:ellipsis;"></span>
              <button id="tokcopy" style="width:auto;">Copy</button>
            </div>
          </div>
        </div>
        <div class="divider">or connect with a token</div>
        <div class="f"><input id="token" placeholder="API token (pdm_…)" autocomplete="off"></div>
        <button id="connect">Connect</button>
        <p class="auth-help">API tokens: an administrator creates them, or on the server with
          <span class="mono">pdm-cli create-token</span>.</p>
        <div id="authbuild" class="muted" style="font-size:11px; margin-top:12px;"></div>
      </div>
    </div>
  </div>

  <div id="app" class="hidden">
    <div class="bar">
      <select id="project"></select>
      <button id="newproj">+ Project</button>
      <button id="projkebab" class="kebab" title="Project actions">⋮</button>
      <button id="newitem">+ Item</button>
      <input id="search" placeholder="search part no / description" style="min-width:220px; flex:1; max-width:340px;">
      <button id="searchgo">Search</button>
    </div>

    <h2>Recently modified</h2>
    <div id="recent" class="recent"></div>

    <div class="section-head">
      <h2>Folders</h2>
      <div class="grow"></div>
      <button id="newfolder">+ Folder</button>
    </div>
    <div id="folders" class="folders"></div>

    <div class="section-head">
      <h2>Items</h2>
      <div class="chips" id="chips">
        <span class="chip active" data-f="all">All</span>
        <span class="chip" data-f="mine">Mine</span>
        <span class="chip" data-f="recent">Recent</span>
        <span class="chip" data-f="archived">Archived</span>
      </div>
      <div class="grow"></div>
      <div class="viewswitch">
        <button id="viewlist" title="List view">≡</button>
        <button id="viewgrid" title="Grid view">▦</button>
      </div>
    </div>
    <div id="searchnote" class="muted hidden" style="font-size:12px;margin-bottom:8px"></div>
    <table id="items"><thead><tr>
      <th>Name</th><th>Part no</th><th>State</th><th>Version</th><th>Modified</th>
      <th>Modified by</th><th>Owner</th><th>Lock</th><th></th>
    </tr></thead><tbody></tbody></table>
    <div id="itemsgrid" class="grid hidden"></div>

    <div id="detail" class="hidden">
      <h2 id="detailtitle">Item</h2>
      <div id="dmeta" style="margin:4px 0 8px;"></div>
      <div class="row">
        <label class="dim">file</label><input id="dfile" style="width:220px">
        <label class="dim">message</label><input id="dmsg" placeholder="commit message" style="width:260px">
        <button id="co">Check out</button>
        <button id="uc">Undo checkout</button>
        <button id="ci">Check in (upload)</button>
        <label style="display:inline-flex;align-items:center;gap:4px;font-size:12px;color:var(--dim)" title="Tick to start a new major version (V3 - R1); unticked saves a new revision of the current version">
          <input type="checkbox" id="cinewver"> new version
        </label>
        <button id="rel">Release revision</button>
        <button id="itemkebab" class="kebab" title="Item actions">⋮</button>
      </div>
      <h2>Versions</h2>
      <table id="vers" class="ver"><thead><tr>
        <th>Preview</th><th>Version</th><th>Type</th><th>SHA-256</th><th>Author</th><th>When</th><th>Message</th><th></th>
      </tr></thead><tbody></tbody></table>
    </div>

    <h2>Audit</h2>
    <table id="audit"><thead><tr>
      <th>When</th><th>Actor</th><th>Action</th><th>Object</th><th>Detail</th>
    </tr></thead><tbody></tbody></table>
  </div>

  <div id="errbox" class="err hidden"></div>
  <div id="kebabmenu" class="kebabmenu hidden"></div>
</main>

<script>
"use strict";
let TOKEN = localStorage.getItem("pdm_token") || "";
let ME = null, CUR_ITEM = null, LOCK = null, CUR_ITEM_ARCHIVED = false;
let ITEMS = [], FOLDERS = [], PROJECTS = [];
let CUR_PROJECT = null, FOLDER = null, FILTER = "all", SEARCH_Q = "";
let VIEW = localStorage.getItem("pdm_view") || "list";

const $ = id => document.getElementById(id);
function esc(s){ return String(s ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c])); }
function err(m){ const b=$("errbox"); b.textContent = m || "error"; b.classList.remove("hidden");
  setTimeout(()=>b.classList.add("hidden"), 6000); }
function ok(m){ const b=$("errbox"); b.textContent = m; b.classList.remove("hidden");
  b.style.borderColor = "var(--ok)"; b.style.color = "var(--ok)";
  setTimeout(()=>{ b.classList.add("hidden"); b.style.borderColor=""; b.style.color=""; }, 4000); }
function thumbCell(src, cls){
  // Thumbnail placeholders are filled in by attachThumbs(), which fetches the
  // image with the API token (the /thumbnail routes are behind auth).
  cls = cls || "thumb";
  return "<span class='" + cls + "' data-thumb='" + esc(src) + "'></span>";
}
const THUMB_CACHE = new Map();
function thumbImg(url){
  return "<img src='" + url + "' alt='' style='width:100%;height:100%;object-fit:cover;display:block'>";
}
async function fillThumb(el){
  const src = el.getAttribute("data-thumb");
  if (!src) return;
  if (THUMB_CACHE.has(src)){ el.innerHTML = thumbImg(THUMB_CACHE.get(src)); return; }
  try {
    const r = await fetch(src, { headers: { "Authorization": "Bearer " + TOKEN } });
    if (!r.ok) return; // leave the placeholder glyph
    const url = URL.createObjectURL(await r.blob());
    THUMB_CACHE.set(src, url);
    el.innerHTML = thumbImg(url);
  } catch(e){ /* keep placeholder */ }
}
function attachThumbs(root){
  root.querySelectorAll("[data-thumb]").forEach(el => fillThumb(el));
}
function itemName(it){ return it.description || it.part_number; }
// Short file-kind label: extension from the filename if derivable, else a
// short form of the stored MIME type. Shows only what the server actually stored.
function fileKind(mime, filename){
  if (filename){
    const dot = filename.lastIndexOf(".");
    if (dot >= 0 && dot < filename.length - 1) return filename.slice(dot + 1).toUpperCase();
  }
  const m = String(mime || "");
  const sub = m.split("/").pop() || "";
  return sub.toUpperCase().slice(0, 10);
}
function typeBadge(mime, filename){
  const k = fileKind(mime, filename);
  if (!k) return "";
  return "<span class='badge' title='" + esc(String(mime || "") + (filename ? " · " + filename : "")) + "'>" + esc(k) + "</span>";
}
function revBadge(label){
  return label ? "<span class='rev'>" + esc(label) + "</span>" : "<span class='rev wip'>WIP</span>";
}
function verLabel(version, revision, seq){
  if (version != null && revision != null) return "V" + version + " - R" + revision;
  if (seq != null) return "V" + seq;
  return "—";
}
function fmtTime(iso){
  if (!iso) return "—";
  const s = (Date.now() - Date.parse(iso)) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return Math.floor(s/60) + "m ago";
  if (s < 86400) return Math.floor(s/3600) + "h ago";
  if (s < 7*86400) return Math.floor(s/86400) + "d ago";
  return iso.slice(0,10);
}
function nameCell(it){
  const arch = it.archived_at ? "<span class='arch-badge'>archived</span>" : "";
  return "<span style='display:inline-flex;align-items:center;gap:8px;max-width:320px'>" +
    thumbCell("/items/" + it.id + "/thumbnail", "thumb-sm") +
    "<span style='overflow:hidden;text-overflow:ellipsis'>" + esc(itemName(it)) + "</span>" +
    revBadge(it.latest_revision) + typeBadge(it.mime_type, it.filename) + arch + "</span>";
}

// Modal window: openModal(title, fields, submitLabel) resolves to {fieldId: value}
// on submit, or null when closed (Esc, backdrop, ✕, Cancel). To add more data to a
// dialog, just append another field spec here — no other changes needed.
// Field: {id, label, type:"text"|"select", placeholder, value, required, options}
// options entries may be strings or {value, label} objects.
function openModal(title, fields, submitLabel){
  return new Promise(resolve => {
    const wrap = document.createElement("div"); wrap.className = "modal-backdrop";
    const win = document.createElement("div"); win.className = "modal";
    const head = document.createElement("div"); head.className = "modal-head";
    const ht = document.createElement("h3"); ht.textContent = title; head.appendChild(ht);
    const x = document.createElement("button"); x.className = "modal-x"; x.textContent = "✕"; head.appendChild(x);
    const body = document.createElement("form"); body.className = "modal-body";
    const inputs = {};
    for (const f of fields){
      const div = document.createElement("div"); div.className = "modal-field";
      const lab = document.createElement("label"); lab.textContent = f.label || f.id; div.appendChild(lab);
      const el = f.type === "select" ? document.createElement("select") : document.createElement("input");
      if (f.type !== "select") el.type = f.type || "text";
      if (f.placeholder) el.placeholder = f.placeholder;
      if (f.required) el.required = true;
      if (f.options){ for (const o of f.options){ const op = document.createElement("option");
          const isObj = typeof o === "object" && o !== null;
          op.value = isObj ? o.value : o; op.textContent = isObj ? o.label : o; el.appendChild(op); } }
      if (f.value !== undefined) el.value = f.value;
      div.appendChild(el); body.appendChild(div); inputs[f.id] = el;
    }
    const foot = document.createElement("div"); foot.className = "modal-foot";
    const cancel = document.createElement("button"); cancel.textContent = "Cancel";
    const submit = document.createElement("button"); submit.textContent = submitLabel || "OK"; submit.style.background = "#2f4a77";
    foot.appendChild(cancel); foot.appendChild(submit);
    let closed = false;
    const done = v => { if (closed) return; closed = true; wrap.remove();
      document.removeEventListener("keydown", onKey); resolve(v); };
    const onKey = e => { if (e.key === "Escape" && document.querySelector(".modal-backdrop:last-of-type") === wrap) done(null); };
    cancel.onclick = () => done(null);
    x.onclick = () => done(null);
    wrap.onmousedown = e => { if (e.target === wrap) done(null); };
    const submitForm = () => {
      if (!body.reportValidity()) return;
      const v = {}; for (const k in inputs) v[k] = inputs[k].value;
      done(v);
    };
    body.onsubmit = e => { e.preventDefault(); submitForm(); };
    const implicitSubmit = document.createElement("button");
    implicitSubmit.type = "submit"; implicitSubmit.hidden = true;
    body.appendChild(implicitSubmit);
    submit.onclick = submitForm;
    win.appendChild(head); win.appendChild(body); win.appendChild(foot);
    wrap.appendChild(win); document.body.appendChild(wrap);
    document.addEventListener("keydown", onKey);
    const first = body.querySelector("input,select"); if (first) first.focus();
  });
}

// Kebab (⋮) dropdown: showKebab(anchorButton, entries); entries may contain
// "sep" separators and {label, danger, act} items. Closes on outside click / Esc.
function showKebab(btn, entries){
  const m = $("kebabmenu");
  m.innerHTML = entries.map((en, i) => en === "sep" ? "<div class='sep'></div>" :
    "<div class='ki" + (en.danger ? " danger" : "") + "' data-i='" + i + "'>" + esc(en.label) + "</div>").join("");
  m.classList.remove("hidden");
  const r = btn.getBoundingClientRect();
  const w = m.offsetWidth;
  const left = Math.max(6, Math.min(r.right - w, window.innerWidth - w - 6));
  let top = r.bottom + 4;
  if (top + m.offsetHeight > window.innerHeight - 6) top = Math.max(6, r.top - m.offsetHeight - 4);
  m.style.left = left + "px";
  m.style.top = top + "px";
  m.onclick = e => {
    const ki = e.target.closest(".ki"); if (!ki) return;
    const en = entries[+ki.dataset.i];
    hideKebab(); if (en && en.act) en.act();
  };
}
function hideKebab(){ $("kebabmenu").classList.add("hidden"); }
document.addEventListener("click", e => {
  if (!e.target.closest("#kebabmenu") && !e.target.closest("button.kebab")) hideKebab();
});
document.addEventListener("keydown", e => { if (e.key === "Escape") hideKebab(); });

async function api(method, path, body, rawBody){
  const headers = {};
  if (TOKEN) headers["Authorization"] = "Bearer " + TOKEN;
  let payload;
  if (rawBody !== undefined){ payload = rawBody; }
  else if (body !== undefined){ headers["Content-Type"] = "application/json"; payload = JSON.stringify(body); }
  const r = await fetch(path, { method, headers, body: payload });
  const ctype = r.headers.get("content-type") || "";
  let data;
  if (ctype.includes("application/json")) data = await r.json();
  else if (ctype === "text/plain") data = (await r.text());
  else data = await r.arrayBuffer();
  if (!r.ok){
    let msg;
    try { msg = (typeof data === "object" && data !== null && data.error) ?
      (data.error.message || data.error) : (typeof data === "string" ? data : "HTTP " + r.status); }
    catch(e){ msg = "HTTP " + r.status; }
    throw new Error(msg);
  }
  return data;
}

// Running build info (GET /version, public): version, test-build tag, commit.
async function loadBuild(){
  try {
    const d = await api("GET", "/version");
    const parts = [
      d.version ? "v" + d.version : null,
      d.build ? "build " + d.build : null,
      d.commit ? d.commit : null,
      d.built_at ? "built " + d.built_at : null,
    ].filter(Boolean);
    const s = parts.join(" · ");
    if (s){ $("buildinfo").textContent = s; $("authbuild").textContent = s; }
  } catch(e){ /* public endpoint, best-effort */ }
}

function authBusy(on, label){
  $("signin").disabled = on;
  const c = $("connect");
  c.disabled = on;
  c.textContent = on ? (label || "Connecting…") : "Connect";
}

async function connect(){
  const typed = $("token").value.trim();
  const saved = TOKEN;
  TOKEN = typed || saved;
  if (!TOKEN){ err("Sign in, or enter an API token"); return; }
  const silent = !typed && !!saved;      // a page refresh reconnecting from storage
  if (silent) authBusy(true, "Connecting…");
  try {
    ME = await api("GET", "/me");
    localStorage.setItem("pdm_token", TOKEN);
    if (silent) authBusy(false);
    showApp();
  } catch(e){
    if (silent){
      TOKEN = ""; localStorage.removeItem("pdm_token");
      authBusy(false);
      err("Your saved session ended (" + e.message + ") — sign in again.");
      $("login_email").focus();
    } else err("Connect failed: " + e.message);
  }
}

async function signin(){
  const email = $("login_email").value.trim();
  const password = $("login_password").value;
  const org = $("login_org").value.trim();
  if (!email || !password){ err("Enter your email and password"); return; }
  authBusy(true, "Signing in…");
  try {
    const r = await fetch("/auth/login", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ username: email, password, ...(org ? { org } : {}) }) });
    const d = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error((d.error && d.error.message) || ("HTTP " + r.status));
    TOKEN = d.token;
    localStorage.setItem("pdm_token", TOKEN);
    $("login_password").value = "";
    $("tokval").textContent = TOKEN;
    $("tokshow").classList.remove("hidden");
    ME = { org_id: d.org && d.org.id, user: d.user, scopes: "full" };
    showApp();
  } catch(e){
    authBusy(false);
    err("Sign in failed: " + e.message);
  }
}

async function signout(){
  if (TOKEN){
    try { await api("POST", "/auth/logout"); } catch(e){ /* token may already be gone */ }
  }
  TOKEN = ""; localStorage.removeItem("pdm_token");
  location.reload();
}

$("account").onclick = e => {
  e.stopPropagation();
  if (!ME.user) return;
  const entries = [{ label: "Change my password", act: () => changePassword() }];
  if (ME.user.role === "admin")
    entries.push("sep", { label: "Manage members", act: () => manageMembers() });
  showKebab(e.currentTarget, entries);
};

async function changePassword(){
  const f = await openModal("Change password",
    [ {id:"current", label:"Current password", type:"password", required:true},
      {id:"new", label:"New password", type:"password", required:true, placeholder:"minimum 4 characters"},
      {id:"confirm", label:"Confirm new password", type:"password", required:true} ],
    "Change password");
  if (!f) return;
  if (f.new !== f.confirm){ err("New passwords do not match"); return; }
  try {
    await api("POST", "/me/password", { current: f.current, new: f.new });
    ok("Password changed");
  } catch(e){ err("change password: " + e.message); }
}

// Admin: manage org members' account status and passwords.
async function manageMembers(){
  const wrap = document.createElement("div"); wrap.className = "modal-backdrop";
  const win = document.createElement("div"); win.className = "modal"; win.style.minWidth = "460px";
  const head = document.createElement("div"); head.className = "modal-head";
  const ht = document.createElement("h3"); ht.textContent = "Members"; head.appendChild(ht);
  const x = document.createElement("button"); x.className = "modal-x"; x.textContent = "✕"; head.appendChild(x);
  const body = document.createElement("div"); body.className = "modal-body";
  body.innerHTML = "<table><thead><tr><th>User</th><th>Role</th><th>Status</th><th></th></tr></thead>" +
    "<tbody class='mm-rows'><tr><td class='muted' colspan='4'>Loading…</td></tr></tbody></table>";
  win.appendChild(head); win.appendChild(body); wrap.appendChild(win);
  document.body.appendChild(wrap);
  const close = () => { wrap.remove(); document.removeEventListener("keydown", onKey); };
  const onKey = e => { if (e.key === "Escape" &&
    document.querySelectorAll(".modal-backdrop").length === 1) close(); };
  document.addEventListener("keydown", onKey);
  x.onclick = close;
  wrap.onmousedown = e => { if (e.target === wrap) close(); };
  try {
    const d = await api("GET", "/users");
    const tb = body.querySelector(".mm-rows"); tb.innerHTML = "";
    for (const u of d.users){
      const tr = document.createElement("tr");
      const name = u.display_name || u.email;
      const isMe = ME.user && u.id === ME.user.id;
      tr.innerHTML =
        "<td>" + esc(name) + (isMe ? " <span class='muted'>(you)</span>" : "") +
        "<div class='muted' style='font-size:11px'>" + esc(u.email) + "</div></td>" +
        "<td class='muted'>" + esc(u.role) + "</td>" +
        "<td class='muted mm-status'>" + (u.active ? "Active" : "Disabled") + "</td>" +
        "<td style='text-align:right'><button class='mm-reset'>Reset password</button> " +
        "<button class='mm-active'>" + (u.active ? "Disable" : "Enable") + "</button></td>";
      const activeButton = tr.querySelector(".mm-active");
      activeButton.onclick = async () => {
        const active = !u.active;
        if (!active && !confirm("Disable account for " + u.email + "? Sign-in and existing API tokens will be blocked until the account is enabled again." +
          (isMe ? " This is your account; you will be signed out." : ""))) return;
        activeButton.disabled = true;
        try {
          await api("PUT", "/users/" + u.id, { active });
          u.active = active;
          tr.querySelector(".mm-status").textContent = active ? "Active" : "Disabled";
          activeButton.textContent = active ? "Disable" : "Enable";
          ok("Account for " + name + " " + (active ? "enabled" : "disabled"));
          if (!active && isMe) { close(); await signout(); }
        } catch(e){ err("account status: " + e.message); }
        finally { activeButton.disabled = false; }
      };
      tr.querySelector(".mm-reset").onclick = async () => {
        const f = await openModal("Reset password — " + name,
          [ {id:"new", label:"New password", type:"password", required:true, placeholder:"minimum 4 characters"},
            {id:"confirm", label:"Confirm new password", type:"password", required:true} ],
          "Reset password");
        if (!f) return;
        if (f.new !== f.confirm){ err("Passwords do not match"); return; }
        try {
          await api("POST", "/users/" + u.id + "/password", { password: f.new });
          ok("Password reset for " + name);
        } catch(e){ err("reset password: " + e.message); }
      };
      tb.appendChild(tr);
    }
    if (!d.users.length)
      tb.innerHTML = "<tr><td class='muted' colspan='4'>No users.</td></tr>";
  } catch(e){ err("members: " + e.message); }
}

function showApp(){
  $("dot").classList.add("on");
  $("who").textContent = ME.user ? (ME.user.display_name || ME.user.email) : "service account";
  $("conn").classList.add("hidden");
  $("app").classList.remove("hidden");
  $("relog").classList.remove("hidden");
  $("signout").classList.remove("hidden");
  $("account").classList.toggle("hidden", !ME.user);
  loadProjects(); loadAudit();
}

async function loadProjects(){
  try {
    const d = await api("GET", "/projects?include_archived=true");
    PROJECTS = d.projects;
    const sel = $("project");
    const prev = sel.value;
    sel.innerHTML = "";
    for (const p of d.projects){
      const o = document.createElement("option"); o.value = p.id;
      o.textContent = p.name + (p.description ? " — " + p.description : "") + (p.archived_at ? " (archived)" : "");
      sel.appendChild(o);
    }
    if (d.projects.length){
      const target = d.projects.some(p => p.id === prev) ? prev : d.projects[0].id;
      loadItems(target);
    }
  } catch(e){ err("projects: " + e.message); }
}
$("project").onchange = () => { const pid = $("project").value; if (pid) loadItems(pid); };

async function loadItems(projectId){
  CUR_PROJECT = projectId; SEARCH_Q = ""; FOLDER = null;
  try {
    const d = await api("GET", "/projects/" + projectId + "/items?include_archived=true");
    ITEMS = d.items;
    await loadFolders(projectId);
    renderAll();
  } catch(e){ err("items: " + e.message); }
}

async function loadFolders(projectId){
  try {
    const d = await api("GET", "/projects/" + projectId + "/folders");
    FOLDERS = d.folders || [];
  } catch(e){ FOLDERS = []; }
  renderFolders();
}

function renderAll(){ renderNote(); renderRecent(); renderFolders(); renderItems(); syncChips(); syncViewButtons(); }

function renderNote(){
  const n = $("searchnote");
  if (SEARCH_Q){
    n.classList.remove("hidden");
    n.innerHTML = "Search results for “" + esc(SEARCH_Q) + "” (" + ITEMS.length + ") " +
      "<button id='clearsearch' style='padding:2px 10px;font-size:11px'>× clear</button>";
    $("clearsearch").onclick = () => { if (CUR_PROJECT) loadItems(CUR_PROJECT); };
  } else n.classList.add("hidden");
}

function renderRecent(){
  const box = $("recent"); box.innerHTML = "";
  const recent = ITEMS.filter(it => it.modified_at)
    .sort((a,b) => Date.parse(b.modified_at) - Date.parse(a.modified_at)).slice(0,4);
  if (!recent.length){
    box.innerHTML = "<span class='muted' style='font-size:12px'>Nothing modified yet — check in a version to see thumbnails here.</span>";
    return;
  }
  for (const it of recent){
    const c = document.createElement("div"); c.className = "recent-card";
    c.innerHTML = thumbCell("/items/" + it.id + "/thumbnail", "thumb-lg") +
      "<div class='rc-name'>" + esc(itemName(it)) + "</div>" +
      "<div class='rc-meta'>modified by " + esc(it.modified_by || "—") + " · " + fmtTime(it.modified_at) + "</div>";
    c.onclick = () => openItem(it.id);
    box.appendChild(c);
  }
  attachThumbs(box);
}

function renderFolders(){
  const box = $("folders"); box.innerHTML = "";
  if (!FOLDERS.length){
    box.innerHTML = "<span class='muted' style='font-size:12px'>No folders yet.</span>";
    return;
  }
  for (const f of FOLDERS){
    const t = document.createElement("div");
    t.className = "folder-tile" + (FOLDER === f.id ? " active" : "");
    t.innerHTML = "<span class='ficon'>📁</span>" + esc(f.name);
    t.onclick = () => { FOLDER = FOLDER === f.id ? null : f.id; renderAll(); };
    box.appendChild(t);
  }
}

function filteredItems(){
  const me = ME && ME.user ? ME.user.id : null;
  const week = 7*86400*1000, now = Date.now();
  let list = ITEMS;
  // "archived" isolates soft-deleted rows; the other chips show live rows only.
  if (FILTER === "archived") list = list.filter(it => it.archived_at);
  else list = list.filter(it => !it.archived_at);
  if (FOLDER) list = list.filter(it => it.folder_id === FOLDER);
  if (FILTER === "mine") list = list.filter(it => me && it.owner_id === me);
  else if (FILTER === "recent") list = list.filter(it => it.modified_at && (now - Date.parse(it.modified_at)) < week);
  return list;
}

function renderTable(list){
  const tb = $("items").querySelector("tbody"); tb.innerHTML = "";
  for (const it of list){
    const tr = document.createElement("tr");
    if (it.archived_at) tr.style.opacity = "0.55";
    tr.innerHTML =
      "<td>" + nameCell(it) + "</td>" +
      "<td class='mono'>" + esc(it.part_number) + "</td>" +
      "<td class='muted'>" + esc(it.lifecycle_state) + "</td>" +
       "<td>" + (it.head_version_seq != null ? verLabel(it.head_version, it.head_revision, it.head_version_seq) : "—") + "</td>" +
      "<td class='muted'>" + fmtTime(it.modified_at) + "</td>" +
      "<td class='muted'>" + esc(it.modified_by || "—") + "</td>" +
      "<td class='muted'>" + esc(it.owner_name || "—") + "</td>" +
      "<td>" + (it.locked_by ? "<span class='lock'>🔒 " + esc(it.locked_by) + "</span>" : "") + "</td>" +
      "<td style='text-align:right'><button class='kebab' data-kb='" + it.id + "' title='Item actions'>⋮</button></td>";
    tr.querySelector("[data-kb]").onclick = e => {
      e.stopPropagation();
      showKebab(e.currentTarget, [
        { label: "Open details", act: () => openItem(it.id) },
        { label: it.archived_at ? "Unarchive" : "Archive", act: () => toggleArchiveItem(it.id) },
        "sep",
        { label: "Delete", danger: true, act: () => delItem(it.id) },
      ]);
    };
    tb.appendChild(tr);
  }
  attachThumbs($("items"));
  if (!list.length) tb.innerHTML = "<tr><td class='muted' colspan='9'>No items match.</td></tr>";
}

function renderGrid(list){
  const box = $("itemsgrid"); box.innerHTML = "";
  if (!list.length){
    box.innerHTML = "<span class='muted' style='font-size:12px'>No items match.</span>";
    return;
  }
  for (const it of list){
    const c = document.createElement("div"); c.className = "grid-card";
    if (it.archived_at) c.style.opacity = "0.55";
    c.innerHTML = thumbCell("/items/" + it.id + "/thumbnail", "thumb-lg") +
      "<div class='gc-name'>" + esc(itemName(it)) + revBadge(it.latest_revision) +
        (it.archived_at ? " <span class='arch-badge'>archived</span>" : "") + "</div>" +
      "<div class='gc-pn'>" + esc(it.part_number) + "</div>";
    c.onclick = () => openItem(it.id);
    box.appendChild(c);
  }
  attachThumbs(box);
}

function renderItems(){
  const list = filteredItems();
  if (VIEW === "grid"){
    $("items").classList.add("hidden");
    $("itemsgrid").classList.remove("hidden");
    renderGrid(list);
  } else {
    $("itemsgrid").classList.add("hidden");
    $("items").classList.remove("hidden");
    renderTable(list);
  }
}

function syncChips(){ for (const c of $("chips").children) c.classList.toggle("active", c.dataset.f === FILTER); }
function syncViewButtons(){
  $("viewlist").classList.toggle("on", VIEW === "list");
  $("viewgrid").classList.toggle("on", VIEW === "grid");
}

async function openItem(id){
  CUR_ITEM = id;
  try {
    const d = await api("GET", "/items/" + id);
    $("detail").classList.remove("hidden");
    $("detailtitle").textContent = d.item.part_number + "  ·  " + itemName(d.item);
    CUR_ITEM_ARCHIVED = !!d.item.archived_at;
    const f = d.file || {};
    $("dmeta").innerHTML =
      (d.item.archived_at ? "<span class='arch-badge'>archived</span> " : "") +
      revBadge(d.item.latest_revision) +
      typeBadge(f.mime_type || d.item.mime_type, f.filename || d.item.filename) +
      "<span class='muted' style='font-size:12px;margin-left:8px'>owner: " + esc(d.item.owner_name || "—") +
      " · last modified " + fmtTime(d.item.modified_at) + " by " + esc(d.item.modified_by || "—") + "</span>";
    $("dfile").value = d.file ? (d.file.filename || "") : "";
    LOCK = d.lock;
    loadVersions(d);
  } catch(e){ err("open item: " + e.message); }
}

async function loadVersions(d){
  const tb = $("vers").querySelector("tbody"); tb.innerHTML = "";
  for (const v of (d.versions || [])){
    const tr = document.createElement("tr");
    tr.innerHTML =
      "<td>" + (v.thumbnail_sha256 ? thumbCell("/versions/" + v.id + "/thumbnail") : "") + "</td>" +
      "<td>" + verLabel(v.version, v.revision, v.seq) + "</td>" +
      "<td>" + typeBadge(v.mime_type, v.native_format) + "</td>" +
      "<td class='mono muted'>" + esc(v.sha256 || "").slice(0,16) + "…</td>" +
      "<td class='muted'>" + esc(v.author_name || "—") + "</td>" +
      "<td class='muted'>" + esc(v.committed_at ? v.committed_at.slice(0,19).replace("T"," ") : "") + "</td>" +
      "<td>" + esc(v.commit_message || "") + "</td>" +
      "<td><button data-v='" + v.id + "'>Download</button></td>";
    tr.querySelector("button").onclick = () => downloadVersion(v.id);
    tb.appendChild(tr);
  }
  attachThumbs($("vers"));
}

async function downloadVersion(vid){
  try {
    const r = await fetch("/versions/" + vid + "/blob", { headers: { "Authorization": "Bearer " + TOKEN } });
    if (!r.ok) throw new Error("HTTP " + r.status);
    const buf = await r.arrayBuffer();
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([buf]));
    a.download = (CUR_ITEM ? $("dfile").value || "download" : "download");
    a.click(); URL.revokeObjectURL(a.href);
  } catch(e){ err("download: " + e.message); }
}

async function loadAudit(){
  try {
    const d = await api("GET", "/audit?limit=25");
    const tb = $("audit").querySelector("tbody"); tb.innerHTML = "";
    for (const ev of (d.events || [])){
      const tr = document.createElement("tr");
      tr.innerHTML =
        "<td class='muted'>" + esc(ev.at ? ev.at.slice(0,19).replace("T"," ") : "") + "</td>" +
        "<td class='muted'>" + esc(ev.actor_name || "system") + "</td>" +
        "<td class='mono'>" + esc(ev.action) + "</td>" +
        "<td class='muted'>" + esc(ev.object_type || "") + "</td>" +
        "<td class='muted'>" + esc(ev.detail ? JSON.stringify(ev.detail) : "") + "</td>";
      tb.appendChild(tr);
    }
  } catch(e){ /* audit is best-effort */ }
}

async function checkout(){
  if (!CUR_ITEM) return;
  try {
    const d = await api("POST", "/items/" + CUR_ITEM + "/checkout", { workstation: location.hostname });
    LOCK = d.lock; ok("Checked out (base " + (d.base_version ? verLabel(d.base_version.version, d.base_version.revision, d.base_version.seq) : "—") + ")");
    loadAudit();
    const fresh = await api("GET", "/items/" + CUR_ITEM); openItem(CUR_ITEM, null);
  } catch(e){ err("checkout: " + e.message); }
}

async function undoCheckout(){
  if (!CUR_ITEM) return;
  try { await api("DELETE", "/items/" + CUR_ITEM + "/checkout");
    LOCK = null; ok("Checkout undone"); loadAudit();
    const fresh = await api("GET", "/items/" + CUR_ITEM); openItem(CUR_ITEM, null);
  } catch(e){ err("undo: " + e.message); }
}

async function checkin(){
  if (!CUR_ITEM) return;
  const f = $("dfile").files && $("dfile").files[0];
  if (!f){ err("Pick a file to check in"); return; }
  try {
    const buf = await f.arrayBuffer();
    const digest = await crypto.subtle.digest("SHA-256", buf);
    const sha = Array.from(new Uint8Array(digest)).map(b => b.toString(16).padStart(2,"0")).join("");
    const up = await api("PUT", "/blobs", null, new Uint8Array(buf));
    const base = (await api("GET", "/items/" + CUR_ITEM)).head_version;
    const newVersion = $("cinewver").checked;
    const d = await api("POST", "/items/" + CUR_ITEM + "/checkin", {
      sha256: up.blob.sha256, size: up.blob.size_bytes,
      base_version: base ? base.seq : null,
      commit_message: $("dmsg").value || "", filename: f.name,
      mime_type: f.type || null,
      new_version: newVersion,
    });
    ok("Checked in " + verLabel(d.version.version, d.version.revision, d.version.seq) + "  (" + sha.slice(0,12) + "…)");
    LOCK = null; $("dmsg").value = ""; $("cinewver").checked = false;
    loadAudit();
    const fresh = await api("GET", "/items/" + CUR_ITEM); openItem(CUR_ITEM, null);
  } catch(e){ err("checkin: " + e.message); }
}

async function release(){
  if (!CUR_ITEM) return;
  try {
    const d = await api("POST", "/items/" + CUR_ITEM + "/release", {});
    ok("Released as revision " + d.revision.label); loadAudit();
  } catch(e){ err("release: " + e.message); }
}

// ── Archive / delete (soft-delete + hard-delete) ─────────────────────────
function curProject(){ const v = $("project").value; return PROJECTS.find(p => p.id === v) || null; }

async function toggleArchiveItem(id){
  try {
    const it = ITEMS.find(x => x.id === id);
    const wasArchived = !!(it && it.archived_at);
    if (wasArchived) await api("PATCH", "/items/" + id + "/unarchive");
    else await api("PATCH", "/items/" + id + "/archive");
    ok(wasArchived ? "Item unarchived" : "Item archived");
    loadAudit();
    if (CUR_ITEM === id) CUR_ITEM_ARCHIVED = !wasArchived;
    if (!wasArchived && CUR_ITEM === id){ CUR_ITEM = null; $("detail").classList.add("hidden"); }
    const pid = CUR_PROJECT || $("project").value; if (pid) loadItems(pid);
  } catch(e){ err("item archive: " + e.message); }
}

async function delItem(id){
  const it = ITEMS.find(x => x.id === id);
  const name = it ? itemName(it) : id;
  const f = await openModal("Delete item",
    [ {id:"confirm", label:"Permanently delete “" + name + "”? This cannot be undone.", type:"text", placeholder:"type 'delete' to confirm"} ],
    "Delete item");
  if (!f || f.confirm !== "delete") return;
  try {
    await api("DELETE", "/items/" + id);
    ok("Item deleted");
    loadAudit();
    if (CUR_ITEM === id){ CUR_ITEM = null; $("detail").classList.add("hidden"); }
    const pid = CUR_PROJECT || $("project").value; if (pid) loadItems(pid);
  } catch(e){ err("delete item: " + e.message); }
}

async function archiveProject(){
  const p = curProject(); if (!p){ err("Pick a project first"); return; }
  try {
    if (p.archived_at) await api("PATCH", "/projects/" + p.id + "/unarchive");
    else await api("PATCH", "/projects/" + p.id + "/archive");
    ok(p.archived_at ? "Project unarchived" : "Project archived");
    loadAudit();
    loadProjects();
  } catch(e){ err("project archive: " + e.message); }
}

async function deleteProject(){
  const p = curProject(); if (!p){ err("Pick a project first"); return; }
  const f = await openModal("Delete project",
    [ {id:"confirm", label:"Permanently delete project “" + p.name + "” and all its items? This cannot be undone.", type:"text", placeholder:"type 'delete' to confirm"} ],
    "Delete project");
  if (!f || f.confirm !== "delete") return;
  try {
    await api("DELETE", "/projects/" + p.id);
    ok("Project deleted");
    loadAudit();
    CUR_ITEM = null; $("detail").classList.add("hidden");
    loadProjects();
  } catch(e){ err("delete project: " + e.message); }
}

$("connect").onclick = connect;
$("signin").onclick = signin;
$("signout").onclick = signout;
$("tokcopy").onclick = async () => {
  try { await navigator.clipboard.writeText(TOKEN); ok("Token copied"); }
  catch (e) { err("Copy failed: " + e.message); }
};
$("relog").onclick = () => { TOKEN = ""; localStorage.removeItem("pdm_token");
  location.reload(); };
$("newproj").onclick = async () => {
  const f = await openModal("New Project",
    [ {id:"name", label:"Name", required:true, placeholder:"e.g. Structural Frames"},
      {id:"description", label:"Description", placeholder:"optional"} ],
    "Create project");
  if (!f) return;
  try { await api("POST", "/projects", { name: f.name, description: f.description || "" }); ok("Project created");
    loadProjects(); } catch(e){ err("new project: " + e.message); }
};
$("newfolder").onclick = async () => {
  const pid = $("project").value; if (!pid){ err("Pick a project first"); return; }
  const f = await openModal("New Folder",
    [ {id:"name", label:"Name", required:true, placeholder:"e.g. Housings"} ],
    "Create folder");
  if (!f) return;
  try { await api("POST", "/projects/" + pid + "/folders", { name: f.name }); ok("Folder created");
    loadFolders(pid); } catch(e){ err("new folder: " + e.message); }
};
$("newitem").onclick = async () => {
  const pid = $("project").value; if (!pid){ err("Pick a project first"); return; }
  const f = await openModal("New Item",
    [ {id:"description", label:"Description", required:true, placeholder:"what is this part?"},
      {id:"part_number", label:"Part number", placeholder:"leave blank to auto-assign"},
      {id:"classification", label:"Classification", type:"select", options:["BE","ASM","MFG"], value:"BE"},
      {id:"folder", label:"Folder", type:"select",
       options:[{value:"", label:"(none)"}].concat(FOLDERS.map(x => ({value: x.id, label: x.name})))} ],
    "Create item");
  if (!f) return;
  try {
    const d = await api("POST", "/projects/" + pid + "/items",
      { part_number: f.part_number || null, description: f.description,
        classification: f.classification, folder_id: f.folder || null });
    ok("Item created: " + d.item.part_number);
    loadItems(pid); openItem(d.item.id);
  } catch(e){ err("new item: " + e.message); }
};
$("chips").onclick = e => {
  const c = e.target.closest(".chip"); if (!c) return;
  FILTER = c.dataset.f; syncChips(); renderItems();
};
$("viewlist").onclick = () => { VIEW = "list"; localStorage.setItem("pdm_view", VIEW); renderItems(); syncViewButtons(); };
$("viewgrid").onclick = () => { VIEW = "grid"; localStorage.setItem("pdm_view", VIEW); renderItems(); syncViewButtons(); };
$("searchgo").onclick = async () => {
  const q = $("search").value.trim(); if (!q) return;
  try {
    const d = await api("GET", "/search?q=" + encodeURIComponent(q));
    ITEMS = d.results; SEARCH_Q = q; FOLDER = null; FILTER = "all";
    renderAll();
  } catch(e){ err("search: " + e.message); }
};
// The "file" input doubles as the check-in file picker.
$("dfile").type = "file";
$("co").onclick = checkout;
$("uc").onclick = undoCheckout;
$("ci").onclick = checkin;
$("rel").onclick = release;
$("projkebab").onclick = e => {
  e.stopPropagation();
  const p = curProject();
  showKebab(e.currentTarget, p ? [
    { label: p.archived_at ? "Unarchive project" : "Archive project", act: () => archiveProject() },
    "sep",
    { label: "Delete project", danger: true, act: () => deleteProject() },
  ] : [{ label: "No project selected", act: null }]);
};
$("itemkebab").onclick = e => {
  e.stopPropagation();
  if (!CUR_ITEM) return;
  showKebab(e.currentTarget, [
    { label: CUR_ITEM_ARCHIVED ? "Unarchive" : "Archive", act: () => toggleArchiveItem(CUR_ITEM) },
    "sep",
    { label: "Delete", danger: true, act: () => delItem(CUR_ITEM) },
  ]);
};

loadBuild();
if (TOKEN) connect();
</script>
</body>
</html>
"##;
