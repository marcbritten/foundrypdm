/* Foundry PDM web workspace — talks to the PDM API with a Bearer token.
   Sections: dashboard, projects, explorer, files, parts, bom, mfg, machines,
   tasks, search, admin, item detail. Automatic refresh is opt-in. */
"use strict";
(() => {
// ── helpers ──────────────────────────────────────────────────────────────
const $ = id => document.getElementById(id);
function esc(s){ return String(s ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c])); }
function fmtTime(iso){ if(!iso) return "—"; const s=(Date.now()-Date.parse(iso))/1000;
  if (isNaN(s)) return String(iso).slice(0,10);
  if (s<60) return "just now"; if (s<3600) return Math.floor(s/60)+"m ago";
  if (s<86400) return Math.floor(s/3600)+"h ago"; if (s<7*86400) return Math.floor(s/86400)+"d ago";
  return String(iso).slice(0,10); }
function fmtDate(iso){ return iso ? String(iso).slice(0,19).replace("T"," ")+" UTC" : "—"; }
function bytes(n){ if (n==null||n==="") return "—"; n=Number(n); if (isNaN(n)) return "—";
  if (n<1024) return n+" B"; if (n<1048576) return (n/1024).toFixed(1)+" KB"; return (n/1048576).toFixed(1)+" MB"; }
function shaShort(s){ return s ? String(s).slice(0,12)+"…" : "—"; }
function itemName(it){ return (it && (it.description || it.part_number)) || "unnamed item"; }
function myId(){ return ME && ME.user ? ME.user.id : null; }
function myName(){ return ME && ME.user ? (ME.user.display_name || ME.user.email) : ""; }
function isRole(min){ const r = ME && ME.user ? ME.user.role : "viewer";
  const L = {viewer:0, editor:1, admin:2}; return (L[r]??0) >= (L[min]??0); }

// ── state ────────────────────────────────────────────────────────────────
let TOKEN = localStorage.getItem("pdm_token") || "";
let ME = null;
let ROUTE = { name:"dashboard", origin:"dashboard" };
let PROJECTS = [], PMAP = {}, USERS = [], UMAP = {};
let ALLITEMS = [], ALLITEMS_AT = 0;
let CUR_RELOAD = null;
let AUTH_BUSY = false;
const POLL_MS = 6000;
let AUTO_REFRESH = localStorage.getItem("pdm_auto_refresh") === "true";
const THUMB_CACHE = new Map();
const BLOB_MAP = new Map();
let PZ = null;

// ── API ──────────────────────────────────────────────────────────────────
async function api(method, path, body, rawBody){
  const headers = {};
  if (TOKEN) headers["Authorization"] = "Bearer " + TOKEN;
  let payload;
  if (rawBody !== undefined) payload = rawBody;
  else if (body !== undefined){ headers["Content-Type"] = "application/json"; payload = JSON.stringify(body); }
  let r;
  try { r = await fetch(path, { method, headers, body: payload }); }
  catch(e){ throw new Error("network error — is the server reachable?"); }
  const ctype = r.headers.get("content-type") || "";
  let data;
  if (ctype.includes("application/json")) data = await r.json().catch(()=>null);
  else if (ctype.startsWith("text/")) data = await r.text();
  else data = await r.arrayBuffer();
  if (!r.ok){
    let msg;
    if (r.status === 401) msg = "session expired — please sign in again";
    else if (data && typeof data === "object" && data.error)
      msg = typeof data.error === "string" ? data.error : (data.error.message || "error");
    else if (typeof data === "string" && data.trim()) msg = data;
    else msg = "HTTP " + r.status;
    const e = new Error(msg); e.status = r.status;
    if (r.status === 401 && ME && !AUTH_BUSY) location.reload();
    throw e;
  }
  return data;
}

let toastTimer = null;
function toast(msg, kind, ms){ const t = $("toast"); if (!t) return;
  t.textContent = msg; t.className = "toast " + (kind || "");
  clearTimeout(toastTimer); toastTimer = setTimeout(()=>t.classList.add("hidden"), ms || 4200); }

// ── thumbnails (auth-gated → fetch with the token, cache object URLs) ────
async function thumbUrl(src){
  if (!src) return null;
  if (THUMB_CACHE.has(src)) return THUMB_CACHE.get(src);
  try {
    const r = await fetch(src, { cache:"no-store", headers: { "Authorization": "Bearer " + TOKEN } });
    if (!r.ok) return null;
    const u = URL.createObjectURL(await r.blob());
    THUMB_CACHE.set(src, u);
    return u;
  } catch(e){ return null; }
}
async function attachThumbs(root){
  await Promise.all([...root.querySelectorAll("[data-thumb]")].map(async el => {
    const src = el.getAttribute("data-thumb");
    if (!src || el.querySelector("img")) return;
    const u = await thumbUrl(src);
    if (u) el.innerHTML = "<img src='" + u + "' alt='' loading='lazy'>";
  }));
}

// ── file kinds / badges ──────────────────────────────────────────────────
function kindSvg(id){ return "<svg class='icon sm'><use href='#" + id + "'/></svg>"; }
const CAD_EXTS = ["solidcraft","solvecraft","step","stp","igs","iges","stl","obj","3mf","glb","gltf",
  "ipt","iam","fcstd","3dm","sldprt","sldasm","slddrw","cgr","prt","x_t","f3d","sat",
  "sab","smt","smb","idw","ipj","jt","wrl","dwg"];
const GCODE_EXTS = ["nc","ngc","gcode","post","tap","mpf","clp"];
function kindInfo(mime, filename){
  const n = String(filename || "").toLowerCase(); const m = String(mime || "");
  const ext = n.slice(n.lastIndexOf(".") + 1);
  if (n.endsWith(".dxf") || m.includes("dxf")) return { label:"DXF", cls:"dxf", icon: kindSvg("ic-ftype-dxf") };
  if (n.endsWith(".pdf") || m.includes("pdf")) return { label:"PDF", cls:"pdf", icon: kindSvg("ic-ftype-pdf") };
  if (m.startsWith("image/")) return { label:"IMG", cls:"img", icon: kindSvg("ic-image") };
  if (ext === "sc" || ext === "solidcraft" || ext === "solvecraft")
    return { label:"SC", cls:"cad", icon: kindSvg("ic-ftype-cad") };
  if (ext === "onshape-bundle")
    return { label:"ONSHAPE", cls:"cad", icon: kindSvg("ic-ftype-cad") };
  if (GCODE_EXTS.includes(ext)) return { label:"GCODE", cls:"cad", icon: kindSvg("ic-gcode") };
  // CAD files badge with their real format (F3D, STEP, STL, FCSTD, …) so the
  // list shows what each file actually is.
  if (CAD_EXTS.includes(ext)) return { label: ext.toUpperCase().slice(0, 8), cls:"cad", icon: kindSvg("ic-ftype-cad") };
  if (m.startsWith("text/") || ["txt","csv","md"].includes(ext))
    return { label:"TXT", cls:"", icon: kindSvg("ic-ftype-txt") };
  if (m === "" || m === "application/octet-stream" || m.includes("solidcraft"))
    return { label:"CAD", cls:"cad", icon: kindSvg("ic-ftype-cad") };
  return { label: ext ? ext.toUpperCase().slice(0,6) : "FILE", cls:"", icon: kindSvg("ic-file") };
}
function kindBadge(mime, filename){ const k = kindInfo(mime, filename);
  return "<span class='badge " + k.cls + "' title='" + esc((mime || "") + (filename ? " · " + filename : "")) + "'>" +
    k.icon + esc(k.label) + "</span>"; }
function stateBadge(s){ const v = String(s || "wip");
  const cls = (v === "released" || v === "release") ? "released" : (v === "draft" ? "draft" : "wip");
  return "<span class='state " + cls + "'>" + esc(v.toUpperCase()) + "</span>"; }
function archBadge(a){ return a ? "<span class='arch'>archived</span>" : ""; }
function revBadge(l){ return l ? "<span class='rev'>" + esc(l) + "</span>" : ""; }
function userAv(u, off){ const nm = (u && (u.display_name || u.email)) || "?";
  const init = nm.split(/\s+/).map(p => p[0] || "").join("").slice(0,2).toUpperCase();
  return "<span class='avatar" + (off ? " off" : "") + "'>" + esc(init) + "</span>"; }
function lockName(lock){ if (!lock) return ""; const u = UMAP[lock.user_id];
  return (u && (u.display_name || u.email)) || ("user " + String(lock.user_id).slice(0,8)); }
function actorName(ev){ const u = UMAP[ev.actor_id];
  return (u && (u.display_name || u.email)) || (ev.actor_name || "system"); }

// ── modal (fields: {id,label,type,options,value,required,placeholder}) ───
function openModal(title, fields, submitLabel, extraHtml, into){
  return new Promise(resolve => {
    const wrap = document.createElement("div"); wrap.className = "modal-backdrop";
    const win = document.createElement("div"); win.className = "modal";
    const head = document.createElement("div"); head.className = "modal-head";
    const ht = document.createElement("h3"); ht.textContent = title; head.appendChild(ht);
    const x = document.createElement("button"); x.className = "modal-x"; x.textContent = "✕"; head.appendChild(x);
    // A real form: reportValidity() (below) is a form method — a plain div threw and the
    // upload never left the page. onsubmit keeps Enter in a field from navigating away.
    const body = document.createElement("form"); body.className = "modal-body";
    if (extraHtml) body.innerHTML = extraHtml;
    const inputs = {};
    for (const f of fields){
      const div = document.createElement("div"); div.className = "modal-field";
      const lab = document.createElement("label"); lab.textContent = f.label || f.id; div.appendChild(lab);
      let el;
      if (f.type === "select"){ el = document.createElement("select"); }
      else if (f.type === "textarea"){ el = document.createElement("textarea"); el.rows = 4; }
      else { el = document.createElement("input"); el.type = f.type === "password" ? "password" : "text"; }
      if (f.placeholder) el.placeholder = f.placeholder;
      if (f.required) el.required = true;
      if (f.options) for (const o of f.options){ const op = document.createElement("option");
        const isObj = typeof o === "object" && o !== null;
        op.value = isObj ? o.value : o; op.textContent = isObj ? o.label : o; el.appendChild(op); }
      if (f.value !== undefined) el.value = f.value;
      div.appendChild(el); body.appendChild(div); inputs[f.id] = el;
    }
    const foot = document.createElement("div"); foot.className = "modal-foot";
    const cancel = document.createElement("button"); cancel.textContent = "Cancel";
    const submit = document.createElement("button"); submit.textContent = submitLabel || "OK";
    submit.style.background = "#2f4a77";
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
    wrap.appendChild(win); (into || document.body).appendChild(wrap);
    document.addEventListener("keydown", onKey);
    const first = body.querySelector("input,select,textarea"); if (first) first.focus();
  });
}

// ── auth ─────────────────────────────────────────────────────────────────
function authBusy(on, label){ if (!$("signin")) return; AUTH_BUSY = on;
  $("signin").disabled = on;
  const c = $("connect"); c.disabled = on;
  c.textContent = on ? (label || "Connecting…") : "Connect"; }

async function connectFromToken(){
  const typed = $("token").value.trim();
  if (typed) TOKEN = typed;
  if (!TOKEN){ toast("Sign in, or enter an API token", "err"); return; }
  const silent = !typed && !!localStorage.getItem("pdm_token");
  if (silent) authBusy(true, "Connecting…");
  try {
    ME = await api("GET", "/me");
    localStorage.setItem("pdm_token", TOKEN);
    enterWorkspace();
  } catch(e){
    authBusy(false);
    if (silent){ TOKEN = ""; localStorage.removeItem("pdm_token");
      toast("Saved session ended (" + e.message + ") — sign in again", "err", 6000); }
    else toast("Connect failed: " + e.message, "err");
  }
}

async function signin(){
  const email = $("login_email").value.trim(), pw = $("login_password").value;
  const org = $("login_org").value.trim();
  if (!email || !pw){ toast("Enter your email and password", "err"); return; }
  authBusy(true, "Signing in…");
  try {
    const r = await fetch("/auth/login", { method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: email, password: pw, ...(org ? { org } : {}) }) });
    const d = await r.json().catch(()=>({}));
    if (!r.ok) throw new Error((d.error && d.error.message) || ("HTTP " + r.status));
    TOKEN = d.token; localStorage.setItem("pdm_token", TOKEN);
    $("login_password").value = "";
    $("tokval").textContent = TOKEN;
    $("tokshow").classList.remove("hidden");
    ME = { org_id: d.org && d.org.id, user: d.user, scopes: "full" };
    enterWorkspace();
  } catch(e){ authBusy(false); toast("Sign in failed: " + e.message, "err"); }
}

async function signout(){
  AUTH_BUSY = true;
  if (TOKEN){ try { await api("POST", "/auth/logout"); } catch(e){} }
  TOKEN = ""; ME = null; localStorage.removeItem("pdm_token");
  location.reload();
}

async function loadCommon(){
  const [pj, us] = await Promise.all([api("GET", "/projects?include_archived=true"), api("GET", "/users")]);
  PROJECTS = pj.projects || []; USERS = us.users || [];
  PMAP = {}; for (const p of PROJECTS) PMAP[p.id] = p;
  UMAP = {}; for (const u of USERS) UMAP[u.id] = u;
}

async function enterWorkspace(){
  authBusy(false);
  $("login-view").classList.add("hidden");
  $("ws").classList.remove("hidden");
  const u = ME.user;
  $("userchip").textContent = u ? (u.display_name || u.email) : "service account";
  $("sb-user").innerHTML = u
    ? userAv(u, false) + " " + esc(u.display_name || u.email) + " <span class='dim' style='margin-left:4px'>" + esc(u.role) + "</span>"
    : "service account";
  $("userkebab").hidden = !u;
  document.querySelectorAll(".sb-admin").forEach(a => a.hidden = !(u && u.role === "admin"));
  try {
    const d = await api("GET", "/version");
    const parts = [d.version ? "v" + d.version : null, d.build, d.commit].filter(Boolean);
    if (parts.length) $("buildinfo").textContent = parts.join(" · ");
  } catch(e){}
  syncNav();
  const r = routeFromHash();
  if (r){ ROUTE = r; syncNav(); render(true); }
  else go("dashboard");
  refreshApprovalsBadge();
}

// ── routing / rendering ─────────────────────────────────────────────────
// Hash routing: every go() writes #/… so back/forward, refresh and shared
// links all survive. Transient view state (loaded items, selection, expanded
// tree nodes) is dropped on navigation and refetched.
const LEGACY_NAME = { files:"items", parts:"items", tasks:"approvals" };
const NAV_NAMES = ["dashboard","projects","explorer","items","search","mfg","machines","tools",
  "post-processors","sheet-metal","approvals","activity","bom","system","settings","admin"];
function go(name, extra){
  name = LEGACY_NAME[name] || name;
  ROUTE = { name, origin: name };
  if (extra) Object.assign(ROUTE, extra);
  if (name === "detail"){
    ROUTE.origin = (extra && extra.origin) || "dashboard";
    DETAIL.tab = (extra && extra._tab) || "overview";
    if (extra && extra._tab) delete ROUTE._tab;
  }
  if (name === "explorer") ROUTE.origin = "projects";
  if (name === "tool"){
    TOOL.tab = (extra && extra._tab) || "general";
    if (extra && extra._tab) delete ROUTE._tab;
  }
  syncNav(); render(true);
  pushHash();
}
function pushHash(){
  const h = routeToHash();
  if (h && location.hash !== h) history.pushState(null, "", h);
}
function routeToHash(){
  const n = ROUTE.name;
  if (n === "explorer"){
    let h = "#/projects/" + ROUTE.pid;
    const q = [];
    if (ROUTE._ptab && ROUTE._ptab !== "items") q.push("tab=" + ROUTE._ptab);
    if (ROUTE._folder) q.push("folder=" + ROUTE._folder);
    if (q.length) h += "?" + q.join("&");
    return h;
  }
  if (n === "detail"){
    let h = "#/items/" + ROUTE.id;
    if (DETAIL.tab && DETAIL.tab !== "overview") h += "?tab=" + DETAIL.tab;
    return h;
  }
  if (n === "tool"){
    let h = "#/tools/" + ROUTE.toolid;
    if (TOOL.tab && TOOL.tab !== "general") h += "?tab=" + TOOL.tab;
    return h;
  }
  const q = [];
  if (n === "items"){
    if (ROUTE._kind && ROUTE._kind !== "all") q.push("kind=" + ROUTE._kind);
    if (ROUTE._cls && ROUTE._cls !== "all") q.push("cls=" + ROUTE._cls);
  }
  if (n === "search" && ROUTE.q) q.push("q=" + encodeURIComponent(ROUTE.q));
  if (n === "approvals" && ROUTE._atab && ROUTE._atab !== "mine") q.push("tab=" + ROUTE._atab);
  if (q.length) return "#/" + n + "?" + q.join("&");
  return "#/" + n;
}
function routeFromHash(){
  const h = (location.hash || "").replace(/^#\/?/, "");
  if (!h) return null;
  const qi = h.indexOf("?");
  const path = qi >= 0 ? h.slice(0, qi) : h;
  const q = {};
  if (qi >= 0) for (const kv of h.slice(qi + 1).split("&")){
    const i = kv.indexOf("=");
    if (i <= 0) continue;
    try { q[decodeURIComponent(kv.slice(0, i))] = decodeURIComponent(kv.slice(i + 1)); }
    catch(e){}
  }
  const seg = path.split("/").filter(Boolean);
  if (seg.length === 1){
    const n = seg[0];
    if (!NAV_NAMES.includes(n)) return null;
    const r = { name:n, origin:n };
    if (n === "search" && q.q) r.q = q.q;
    if (n === "items"){ r._kind = q.kind || "all"; r._cls = q.cls || "all"; }
    if (n === "approvals") r._atab = q.tab || "mine";
    return r;
  }
  if (seg[0] === "projects" && seg.length === 2){
    const r = { name:"explorer", origin:"projects", pid:seg[1] };
    if (q.tab && ["items","activity","bom","manufacturing","settings"].includes(q.tab)) r._ptab = q.tab;
    if (q.folder) r._folder = q.folder;
    return r;
  }
  if (seg[0] === "items" && seg.length === 2)
    return { name:"detail", origin:"items", id:seg[1], _tab: q.tab || undefined };
  if (seg[0] === "tools" && seg.length === 2)
    return { name:"tool", origin:"tools", toolid:seg[1], _tab: q.tab || undefined };
  return null;
}
function sameRoute(r){
  if (r.name !== ROUTE.name || r.pid !== ROUTE.pid || r.id !== ROUTE.id ||
      r.toolid !== ROUTE.toolid) return false;
  if (r.name === "detail") return (r._tab || "overview") === DETAIL.tab;
  if (r.name === "tool") return (r._tab || "general") === TOOL.tab;
  return (r._ptab || "items") === (ROUTE._ptab || "items") &&
    (r._kind || "all") === (ROUTE._kind || "all") &&
    (r._cls || "all") === (ROUTE._cls || "all") &&
    (r.q || "") === (ROUTE.q || "") &&
    (r._atab || "mine") === (ROUTE._atab || "mine") &&
    (r._folder || "") === (ROUTE._folder || "");
}
function onHashNav(){
  const r = routeFromHash();
  if (!r || sameRoute(r)) return;
  ROUTE = r;
  syncNav(); render(true);
}
function navTarget(){
  const map = { dashboard:"dashboard", projects:"projects", explorer:"projects",
    items:"items", files:"items", parts:"items", bom:"items", search:"search",
    mfg:"mfg", machines:"machines", tools:"tools", tool:"tools",
    "post-processors":"post-processors",
    "sheet-metal":"sheet-metal", "laser-nests":"laser-nests",
    approvals:"approvals", tasks:"approvals",
    activity:"activity", system:"system", settings:"settings", admin:"admin",
    detail: ROUTE.origin === "explorer" ? "projects" : "items" };
  return map[ROUTE.name] || "dashboard";
}
function syncNav(){
  const target = navTarget();
  document.querySelectorAll(".sb-link, .bb-link").forEach(a =>
    a.classList.toggle("on", a.dataset.nav === target));
  const names = { dashboard:"Dashboard", projects:"Projects", explorer:"Project",
    items:"Items", search:"Search", mfg:"Manufacturing", machines:"Machines & Tools",
    tools:"Tool library", "post-processors":"Post-processors", "sheet-metal":"Sheet Metal Standards",
    "laser-nests":"Laser Nests",
    approvals:"Approvals", activity:"Activity", bom:"Bills of Material",
    system:"System", settings:"Settings", admin:"Administration", detail:"Item" };
  let c = names[ROUTE.name] || "";
  if (ROUTE.name === "explorer" && PMAP[ROUTE.pid]) c = PMAP[ROUTE.pid].name;
  if (ROUTE.name === "detail" && DETAIL.id && DETAIL.data) c = itemName(DETAIL.data.item);
  if (ROUTE.name === "tool" && TOOL.data) c = TOOL.data.name;
  $("crumb").textContent = c;
}
const DETAIL = { id:null, tab:"overview", data:null, revs:[], comments:[], props:[], cmp:new Set(),
  pvSeq:null, folderCache:null };
const TOOL = { id:null, tab:"general", data:null, cutting:[], docs:[] };

async function render(skeleton){
  const v = $("view");
  if (skeleton){ v.innerHTML = "<div class='skel'><span class='spinner'></span> loading…</div>"; CUR_RELOAD = null; }
  try {
    switch (ROUTE.name){
      case "dashboard": await viewDashboard(v); break;
      case "projects": await viewProjects(v); break;
      case "explorer": await viewExplorer(v); break;
      case "items": await viewItems(v); break;
      case "mfg": await viewMfg(v); break;
      case "machines": await viewMachines(v); break;
      case "tools": await viewTools(v); break;
      case "tool": await viewToolDetail(v); break;
      case "post-processors": await viewPostProcessors(v); break;
      case "sheet-metal": await viewSheetMetal(v); break;
      case "laser-nests": await viewLaserNests(v); break;
      case "approvals": await viewApprovals(v); break;
      case "activity": await viewActivity(v); break;
      case "bom": viewSoon(v, bigIcon("ic-bom"), "Bills of Material",
        "BOM structure (parent / child relationships with quantities) is pending on the PDM backend. The project page already exposes this tab, and items group by folder and classification in the meantime."); break;
      case "search": await viewSearch(v); break;
      case "system": viewSystem(v); break;
      case "settings": await viewSettings(v); break;
      case "admin": await viewAdmin(v); break;
      case "detail": await viewDetail(v); break;
      default: viewSoon(v, bigIcon("ic-warning"), "Not found", "Pick a section from the sidebar.");
    }
  } catch(e){
    if (e.status === 401) return;
    v.innerHTML = "<div class='errbox'>" + esc(e.message) + "</div>" +
      "<div class='row'><button class='primary' id='retry'>Retry</button></div>";
    $("retry").onclick = () => render(true);
  }
  CUR_RELOAD = () => render(false);
}
function viewSoon(v, ic, title, text){
  v.innerHTML = "<div class='soon'><span class='ic'>" + ic + "</span><h3>" + esc(title) +
    "</h3><p>" + esc(text) + "</p></div>";
}

// ── shared layout components (§4.4) ─────────────────────────────────────
function bigIcon(id, size){
  const s = size || 30;
  return "<svg class='icon' style='width:" + s + "px;height:" + s + "px' aria-hidden='true'><use href='#" + id + "'/></svg>";
}
function emptyState(ic, title, text, ctaHtml){
  return "<div class='emptyState'><span class='ic'>" + bigIcon(ic, 34) + "</span>" +
    "<h3>" + esc(title) + "</h3><p>" + esc(text) + "</p>" +
    (ctaHtml || "") + "</div>";
}
function kpiTile(ic, label, value, sub, nav){
  return "<div class='kpi" + (nav ? " clickable" : "") + "'" + (nav ? " data-knav='" + nav + "'" : "") + ">" +
    "<span class='ic'>" + bigIcon(ic, 17) + "</span>" +
    "<div style='min-width:0'><div class='v'>" + value + "</div><div class='l'>" + esc(label) + "</div>" +
    (sub ? "<div class='sub'>" + sub + "</div>" : "") + "</div></div>";
}
function kpiStrip(k){
  if (!k) return "";
  const tiles = [
    kpiTile("ic-items", "Items", k.items ?? 0, null, "items"),
    kpiTile("ic-lock", "Open checkouts", (k.open_checkouts_mine ?? 0) + " of " + (k.open_checkouts ?? 0), "mine / org", "items"),
    kpiTile("ic-approval", "Pending approvals", k.pending_approvals_mine ?? 0,
      (k.pending_approvals_mine ?? 0) === (k.pending_approvals ?? 0) ? "pending, yours" : "org has " + (k.pending_approvals ?? 0), "approvals"),
    kpiTile("ic-revision", "Released", k.released_items ?? 0, null, "items"),
    kpiTile("ic-activity", "Jobs", (k.running_jobs ?? 0) + " running · " + (k.queued_jobs ?? 0) + " queued",
      (k.failed_jobs ? k.failed_jobs + " failed" : "no failures"), "activity"),
    kpiTile("ic-download", "Storage", bytes(k.storage_bytes ?? 0), null, null),
  ];
  return "<div class='kpiStrip' style='margin-bottom:14px'>" + tiles.join("") + "</div>";
}
function feedIcon(action){
  const m = {
    "item.checkin":"ic-lock", "item.checkout":"ic-lock", "item.undo_checkout":"ic-lock",
    "item.release":"ic-revision", "item.released":"ic-revision",
    "item.release_requested":"ic-approval", "item.lifecycle_transitioned":"ic-revision",
    "approval.requested":"ic-approval", "approval.decided":"ic-approval",
    "approval.approve":"ic-approval", "approval.reject":"ic-approval",
    "item.comment":"ic-comment", "folder.created":"ic-folder", "project.created":"ic-project",
    "project.archived":"ic-project", "project.unarchived":"ic-project", "project.deleted":"ic-project",
    "user.created":"ic-settings", "user.login":"ic-settings", "user.token_created":"ic-settings",
    "item.created":"ic-plus", "item.updated":"ic-plus", "item.archived":"ic-warning",
    "item.unarchived":"ic-warning", "item.deleted":"ic-warning",
  };
  return m[action] || "ic-activity";
}
function feedDayLabel(iso){
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "Earlier";
  const day = x => x.toDateString();
  const now = new Date();
  if (day(d) === day(now)) return "Today";
  const yest = new Date(now.getTime() - 86400000);
  if (day(d) === day(yest)) return "Yesterday";
  return d.toLocaleDateString(undefined, { year:"numeric", month:"short", day:"numeric" });
}
function feedTarget(ev){
  if (!ev || !ev.object_type) return "";
  const d = ev.detail || {};
  const label = d.part_number || d.name || d.filename || "";
  if (ev.object_type === "item" && ev.object_id)
    return label ? "<a href='#/items/" + ev.object_id + "'>" + esc(label) + "</a>" :
      "<a href='#/items/" + ev.object_id + "'>an item</a>";
  if ((ev.object_type === "approval" || ev.object_type === "approval_request") && d.item_id)
    return "<a href='#/items/" + d.item_id + "'>an approval</a>";
  if (ev.object_type === "approval" || ev.object_type === "approval_request") return "an approval";
  if (ev.object_type === "project" && ev.object_id && PMAP[ev.object_id])
    return "<a href='#/projects/" + ev.object_id + "'>" + esc(PMAP[ev.object_id].name) + "</a>";
  if (ev.object_type === "project") return "a project";
  if (label) return esc(label);
  return ev.object_type;
}
function feed(events, max){
  const list = max ? (events || []).slice(0, max) : (events || []);
  if (!list.length)
    return emptyState("ic-activity", "Nothing here yet",
      "Check-ins, releases, approvals and uploads will appear in this feed.");
  let h = "<div class='feed'>", last = "";
  for (const ev of list){
    const lbl = feedDayLabel(ev.at);
    if (lbl !== last){ h += "<div class='day'>" + esc(lbl) + "</div>"; last = lbl; }
    h += "<div class='event'>" +
      "<span class='ic'>" + kindSvg(feedIcon(ev.action)) + "</span>" +
      "<div class='body'><div class='who'>" + esc(actorName(ev)) + "</div>" +
      "<div class='what'><span class='mono' style='font-size:12px'>" + esc(ev.action) + "</span> " +
      feedTarget(ev) + "</div>" +
      "<div class='when'>" + fmtTime(ev.at) + "</div></div></div>";
  }
  return h + "</div>";
}

// ── approvals (shared by dashboard widget, approval center, detail) ─────
async function decideApproval(id, decision){
  const f = await openModal(decision === "approve" ? "Approve release" : "Reject approval",
    [{ id:"note", label:"Note (optional)", type:"textarea",
       placeholder: decision === "approve" ? "e.g. meets the release criteria" : "why is this being rejected?" }],
    decision === "approve" ? "Approve" : "Reject");
  if (!f) return;
  try {
    await api("POST", "/approvals/" + id + "/decide", { decision, note: f.note || null });
    toast(decision === "approve" ? "Approved — item released" : "Approval rejected", "ok");
    ALLITEMS = [];
    CUR_RELOAD && CUR_RELOAD();
    refreshApprovalsBadge();
  } catch(e){ toast("Decision failed: " + e.message, "err"); }
}
async function refreshApprovalsBadge(kpis){
  try {
    let n = null;
    if (kpis && kpis.pending_approvals_mine != null) n = kpis.pending_approvals_mine;
    else {
      const d = await api("GET", "/approvals?status=pending&mine=true&limit=1");
      n = d.total || 0;
    }
    const b = $("nb-approvals");
    if (!b) return;
    b.textContent = n > 0 ? String(n) : "";
    b.style.display = n > 0 ? "" : "none";
  } catch(e){}
}
async function doLifecycle(id, state){
  try {
    await api("PATCH", "/items/" + id + "/lifecycle", { state });
    toast("Lifecycle → " + state, "ok");
    ALLITEMS = [];
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Lifecycle: " + e.message, "err"); }
}

// ── shared data ─────────────────────────────────────────────────────────
async function allItems(force){
  if (!force && ALLITEMS.length && Date.now() - ALLITEMS_AT < 8000) return ALLITEMS;
  if (!PROJECTS.length) await loadCommon();
  const out = await Promise.all(PROJECTS.map(async p => {
    try {
      const d = await api("GET", "/projects/" + p.id + "/items?include_archived=true");
      return (d.items || []).map(it => Object.assign({}, it, { _project: p.name, _project_id: p.id }));
    } catch(e){ return []; }
  }));
  ALLITEMS = out.flat(); ALLITEMS_AT = Date.now();
  return ALLITEMS;
}
function sortItems(list, key, dir){
  const m = dir === "asc" ? 1 : -1;
  const val = x => key === "_name" ? itemName(x).toLowerCase()
    : (key === "modified_at" ? (Date.parse(x[key]) || 0) : String(x[key] ?? "").toLowerCase());
  return [...list].sort((a, b) => {
    const av = val(a), bv = val(b);
    if (typeof av === "number") return (av - bv) * m;
    return av < bv ? -m : av > bv ? m : 0;
  });
}

// Sheet-metal parts carry a sheet_metal spec; the ▸ marks a flat pattern
// that can be exported.
function smBadge(it){
  if (!it.sheet_metal) return "";
  const tip = ["Sheet metal part"];
  if (it.sheet_metal_material) tip.push(it.sheet_metal_material);
  if (it.sheet_metal_thickness_mm != null) tip.push(it.sheet_metal_thickness_mm + " mm");
  if (it.sheet_metal_flat_pattern) tip.push("flat pattern available");
  else tip.push("no flat pattern attached yet");
  return "<span class='badge sm' title='" + esc(tip.join(" · ")) + "'>SM" +
    (it.sheet_metal_flat_pattern ? " ▸" : "") + "</span>";
}

// 3D model type column: a sheet-metal part, a solid, or not a 3D model.
function itemType(it){
  if (it.sheet_metal) return "<span class='smtype'>Sheet metal</span>";
  const k = kindInfo(it.mime_type, it.filename);
  if (k.cls === "cad" && k.label !== "GCODE") return "Solid";
  return "—";
}

// ── shared item table ───────────────────────────────────────────────────
function itemsTable(list, showProject, selSet){
  if (!list.length) return "<div class='empty'>No items match — upload a file or create an item.</div>";
  const selall = showProject === true ? "" : "";
  const head = "<thead><tr>" +
    (selSet ? "<th style='width:28px'></th>" : "") +
    "<th class='sortable' data-s='_name'>Name</th>" +
    "<th class='sortable' data-s='part_number'>Part no</th>" +
    "<th>Type</th>" +
    (showProject ? "<th>Project</th>" : "") +
    "<th>State</th><th>Ver</th>" +
    "<th class='sortable' data-s='modified_at'>Modified</th>" +
    "<th class='sortable' data-s='owner_name'>Owner</th><th>Lock</th>" +
    "<th style='width:32px'></th></tr></thead>";
  const cols = 10 + ((selSet || showProject) ? 1 : 0);
  const rows = list.map(it => {
    const k = kindInfo(it.mime_type, it.filename);
    return "<tr class='clickable" + (it.archived_at ? " archived" : "") + "' data-id='" + it.id + "'>" +
      (selSet ? "<td><input type='checkbox' data-sel='" + it.id + "'" +
        (selSet.has(it.id) ? " checked" : "") + " onclick='event.stopPropagation()'></td>" : "") +
      "<td><span class='namecell'><span class='thumb-sm' data-thumb='/items/" + it.id + "/thumbnail'></span>" +
        "<span class='fileicon' title='" + esc(k.label) + "'>" + k.icon + "</span>" +
        "<span class='nm'>" + esc(itemName(it)) + "</span>" +
        revBadge(it.latest_revision) + kindBadge(it.mime_type, it.filename) + smBadge(it) + archBadge(it.archived_at) + "</span></td>" +
       "<td class='mono muted'>" + esc(it.part_number) + "</td>" +
       "<td class='muted'>" + itemType(it) + "</td>" +
       (showProject ? "<td class='muted'>" + esc(it._project || PMAP[it.project_id]?.name || "—") + "</td>" : "") +
      "<td>" + stateBadge(it.lifecycle_state) + "</td>" +
      "<td>" + (it.head_version_seq ?? "—") + "</td>" +
      "<td class='muted'>" + fmtTime(it.modified_at) + "</td>" +
      "<td class='muted'>" + esc(it.owner_name || "—") + "</td>" +
      "<td>" + (it.locked_by ? "<span style='color:var(--warn)' title='checked out'>" + kindSvg("ic-lock") + " " + esc(it.locked_by) + "</span>" : "") + "</td>" +
      "<td style='text-align:right'><button class='kebab-btn' data-kb='item' data-kid='" + it.id +
        "' title='Item actions'>⋮</button></td>" +
      "</tr>";
  }).join("");
  return "<table>" + head + "<tbody>" + rows + "</tbody></table>" +
    (list.length ? "" : "<div class='empty' colspan='" + cols + "'></div>");
}
function wireItemTable(container, opts){
  container.querySelectorAll("tr.clickable").forEach(tr => {
    tr.addEventListener("click", () => openDetail(tr.dataset.id));
    tr.addEventListener("contextmenu", e => {
      e.preventDefault();
      const it = (opts.findItem ? opts.findItem(tr.dataset.id) : null) ||
        (ROUTE._items || ALLITEMS).find(x => x.id === tr.dataset.id);
      if (it) showCtxMenu(e.clientX, e.clientY, it);
    });
  });
  if (opts.onSort) container.querySelectorAll("th.sortable").forEach(th =>
    th.addEventListener("click", () => opts.onSort(th.dataset.s)));
  if (opts.onSel) container.querySelectorAll("[data-sel]").forEach(cb =>
    cb.addEventListener("change", () => {
      if (cb.checked) opts.selSet.add(cb.dataset.sel); else opts.selSet.delete(cb.dataset.sel);
      opts.onSel();
    }));
  wireKebabs(container);
}
function openDetail(id){ go("detail", { id, origin: ROUTE.name === "explorer" ? "explorer" : ROUTE.name,
  _epid: ROUTE.pid || null }); }

// ── context menu ────────────────────────────────────────────────────────
function showCtxMenu(x, y, it){
  const pid = it._project_id || it.project_id || ROUTE.pid;
  const entries = [
    { label:"Open details", key:"↵", act: () => openDetail(it.id) },
  ];
  const mainId = PMAP[pid] && PMAP[pid].main_assembly_id;
  entries.push(mainId === it.id
    ? { label:"Unset main assembly", act: () => setMainAssembly(pid, null) }
    : { label:"Set as main assembly", act: () => setMainAssembly(pid, it.id) });
  if (it.head_version_seq) entries.push({ label:"Download latest", act: () => downloadLatest(it) });
  const editor = PdmEditors.forFile(it.mime_type, it.filename);
  if (it.head_version_seq && editor) entries.push({ label:"Open in " + editor.label + " (web)", act: () => openWebEditor(it.id) });
  if (it.head_version_seq && isRole("editor") && convertibleByName(it.filename))
    entries.push({ label:"Import to SolidCraft (Dutchman)", act: () => importItemHead(it) });
  if (!it.locked_by) entries.push({ label:"Check out", act: () => doCheckout(it.id) });
  else if (it.locked_by === myName()) entries.push({ label:"Undo checkout", act: () => doUndo(it.id) });
  entries.push({ label:"Upload new version", act: () => openDetail(it.id) });
  entries.push("sep");
  entries.push({ label:"Move to folder…", act: () => moveItems(pid, [it.id]) });
  entries.push({ label:"Assign owner…", act: () => assignOwner([it.id]) });
  entries.push({ label:"Release revision…", act: () => openDetail(it.id) });
  if (!it.archived_at && it.lifecycle_state === "released")
    entries.push({ label:"Mark obsolete", act: () => doLifecycle(it.id, "obsolete") });
  entries.push("sep");
  entries.push({ label: it.archived_at ? "Unarchive" : "Archive", act: () => doArchiveItem(it.id) });
  entries.push({ label:"Delete", danger:true, act: () => doDeleteItem(it.id) });
  const m = $("ctxmenu");
  m.innerHTML = entries.map((en, i) => en === "sep" ? "<div class='sep'></div>" :
    "<div class='ci" + (en.danger ? " danger" : "") + "' data-i='" + i + "'><span>" + esc(en.label) + "</span>" +
    (en.key ? "<span class='k'>" + esc(en.key) + "</span>" : "") + "</div>").join("");
  m.classList.remove("hidden");
  const r = m.getBoundingClientRect();
  m.style.left = Math.max(4, Math.min(x, innerWidth - r.width - 8)) + "px";
  m.style.top = Math.max(4, Math.min(y, innerHeight - r.height - 8)) + "px";
  m.onclick = e => { const ci = e.target.closest(".ci"); if (!ci) return;
    const en = entries[+ci.dataset.i]; hideCtx(); if (en && en.act) en.act(); };
}
function hideCtx(){ $("ctxmenu").classList.add("hidden"); }
document.addEventListener("click", e => { if (!e.target.closest("#ctxmenu")) hideCtx(); });
document.addEventListener("scroll", hideCtx, true);
window.addEventListener("resize", hideCtx);

// ── kebab (⋮) menu: archive / delete for projects & items ───────────────
function openKebabMenu(ev, id, kind){
  ev.stopPropagation();
  const p = kind === "proj" ? PMAP[id] : null;
  const it = (ALLITEMS.find(x => x.id === id)) || (ROUTE._items || []).find(x => x.id === id) || null;
  const archived = kind === "proj" ? !!(p && p.archived_at) : !!(it && it.archived_at);
  const entries = [];
  if (kind === "proj")
    entries.push({ label: "Edit", act: () => doEditProject(id) });
  if (kind === "item" && it && isRole("editor") && it.head_version_seq && convertibleByName(it.filename))
    entries.push({ label: "Import to SolidCraft (Dutchman)", act: () => importItemHead(it) });
  entries.push(
    { label: archived ? "Unarchive" : "Archive", act: () =>
        (kind === "proj" ? doArchiveProject(id) : doArchiveItem(id)) },
    "sep",
    { label: "Delete", danger: true, act: () =>
        (kind === "proj" ? doDeleteProject(id) : doDeleteItem(id)) },
  );
  const m = $("kebabmenu");
  m.innerHTML = entries.map((en, i) => en === "sep" ? "<div class='sep'></div>" :
    "<div class='ki" + (en.danger ? " danger" : "") + "' data-i='" + i + "'><span>" +
    esc(en.label) + "</span></div>").join("");
  m.classList.remove("hidden");
  const r = ev.currentTarget.getBoundingClientRect();
  let left = Math.max(6, Math.min(r.right - m.offsetWidth, innerWidth - m.offsetWidth - 6));
  let top = r.bottom + 4;
  if (top + m.offsetHeight > innerHeight - 6) top = Math.max(6, r.top - m.offsetHeight - 4);
  m.style.left = left + "px";
  m.style.top = top + "px";
  m.onclick = e => {
    const ki = e.target.closest(".ki"); if (!ki) return;
    const en = entries[+ki.dataset.i];
    hideKebab(); if (en && en.act) en.act();
  };
}
function hideKebab(){ const m = $("kebabmenu"); if (m) m.classList.add("hidden"); }
function wireKebabs(container){
  container.querySelectorAll("button.kebab-btn[data-kid]").forEach(b =>
    b.addEventListener("click", e => openKebabMenu(e, b.dataset.kid, b.dataset.kb || "item")));
}
document.addEventListener("click", e => {
  if (!e.target.closest("#kebabmenu") && !e.target.closest("button.kebab-btn")) hideKebab();
});
document.addEventListener("keydown", e => { if (e.key === "Escape") hideKebab(); });
document.addEventListener("scroll", hideKebab, true);

async function doArchiveItem(id){
  const it = (ALLITEMS.find(x => x.id === id)) || (ROUTE._items || []).find(x => x.id === id);
  const wasArchived = !!(it && it.archived_at);
  try {
    await api("PATCH", "/items/" + id + (wasArchived ? "/unarchive" : "/archive"));
    toast(wasArchived ? "Unarchived" : "Archived", "ok");
    ALLITEMS = [];
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Archive: " + e.message, "err"); }
}
async function doDeleteItem(id){
  const f = await openModal("Delete item", [
    { id:"confirm", label:"Type “delete” to confirm — this cannot be undone",
      required:true, placeholder:"delete" }
   ], "Delete");
  if (!f || (f.confirm || "").trim().toLowerCase() !== "delete") return;
  try {
    await api("DELETE", "/items/" + id);
    toast("Item deleted", "ok");
    ALLITEMS = [];
    if (ROUTE.name === "detail" && DETAIL.id === id)
      go(ROUTE.origin || "dashboard", { pid: ROUTE._epid || undefined });
    else CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Delete: " + e.message, "err"); }
}
async function doArchiveProject(id){
  const p = PMAP[id];
  const wasArchived = !!(p && p.archived_at);
  try {
    await api("PATCH", "/projects/" + id + (wasArchived ? "/unarchive" : "/archive"));
    toast(wasArchived ? "Project unarchived" : "Project archived", "ok");
    await loadCommon();
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Archive: " + e.message, "err"); }
}
function loadImg(src){ return new Promise((res, rej) => { const im = new Image();
  im.onload = () => res(im); im.onerror = () => rej(new Error("could not load image")); im.src = src; }); }
function drawJpeg(img, w, h, q){ const c = document.createElement("canvas"); c.width = w; c.height = h;
  c.getContext("2d").drawImage(img, 0, 0, w, h); return c.toDataURL("image/jpeg", q); }
// Resize to a cover-sized JPEG data URL, staying under the 400 KB server limit.
async function coverFromFile(file, maxDim){
  const target = 300000;
  const url = URL.createObjectURL(file);
  let img;
  try { img = await loadImg(url); } finally { URL.revokeObjectURL(url); }
  let w = img.naturalWidth || 1, h = img.naturalHeight || 1;
  const s = Math.min(1, (maxDim || 800) / Math.max(w, h));
  w = Math.max(1, Math.round(w * s)); h = Math.max(1, Math.round(h * s));
  for (let q = 0.85; q >= 0.3; q -= 0.13) {
    const data = drawJpeg(img, w, h, q);
    if (data.length <= target) return data;
  }
  return drawJpeg(img, Math.max(1, Math.round(w * 0.7)), Math.max(1, Math.round(h * 0.7)), 0.7);
}
async function doEditProject(id){
  const p = PMAP[id];
  let coverData = p ? (p.image_data || null) : null;
  let coverDirty = false;
  const coverHtml =
    "<div class='modal-field' style='margin-bottom:12px'>" +
      "<label>Cover image</label>" +
      "<div class='coverpick'>" +
        "<img id='covprev' class='cover-prev' alt='' " +
          (coverData ? "src='" + esc(coverData) + "'" : "style='display:none'") + ">" +
        "<div class='cover-actions'>" +
          "<input type='file' id='covfile' accept='image/*'>" +
          "<button type='button' id='covrm' class='link' " +
            (coverData ? "" : "style='display:none'") + ">Remove</button>" +
        "</div>" +
      "</div>" +
    "</div>";
  const modal = openModal("Edit project", [
    { id:"name", label:"Name", required:true, placeholder:"e.g. Structural Frames",
      value: p ? p.name : "" },
    { id:"description", label:"Description", placeholder:"optional",
      value: p ? (p.description || "") : "" }
  ], "Save changes", coverHtml);
  const prev = $("covprev"), fileIn = $("covfile"), rmBtn = $("covrm");
  const show = src => { if (src){ prev.src = src; prev.style.display = ""; } else prev.style.display = "none"; };
  if (prev){
    prev.onerror = () => show(null);
    fileIn.onchange = async () => {
      const file = fileIn.files[0]; if (!file) return;
      fileIn.disabled = true;
      try { coverData = await coverFromFile(file, 800); coverDirty = true; show(coverData);
        rmBtn.style.display = ""; }
      catch(e){ toast("Image: " + e.message, "err"); }
      finally { fileIn.disabled = false; }
    };
    rmBtn.onclick = () => { coverData = null; coverDirty = true; show(null);
      fileIn.value = ""; rmBtn.style.display = "none"; };
  }
  const f = await modal;
  if (!f) return;
  try {
    const body = { name: f.name, description: f.description || null };
    if (coverDirty) body.image_data = coverData === null ? "" : coverData;
    await api("PATCH", "/projects/" + id, body);
    toast("Project updated", "ok");
    await loadCommon();
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Edit: " + e.message, "err"); }
}
async function doDeleteProject(id){
  const p = PMAP[id];
  const f = await openModal("Delete project", [
    { id:"confirm", label:"Type “delete” to confirm — project “" +
      (p ? p.name : id) + "” and all its items will be removed",
      required:true, placeholder:"delete" }
   ], "Delete");
  if (!f || (f.confirm || "").trim().toLowerCase() !== "delete") return;
  try {
    await api("DELETE", "/projects/" + id);
    toast("Project deleted", "ok");
    ALLITEMS = [];
    await loadCommon();
    if (ROUTE.name === "explorer" && ROUTE.pid === id) go("projects");
    else CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Delete: " + e.message, "err"); }
}

// ── project main assembly ───────────────────────────────────────────────
// The project cover: the main assembly's rendered thumbnail when one is set
// (fetched live from /items/{id}/thumbnail), otherwise the chosen cover image.
// `cls` is the sizing class (explorer-cover / pcard-cover).
function projectCover(p, cls){
  if (!p) return "";
  if (p.main_assembly_id)
    return "<span class='" + cls + " pcov' data-thumb='/items/" + p.main_assembly_id + "/thumbnail'></span>";
  if (p.image_data)
    return "<img class='" + cls + "' src='" + esc(p.image_data) + "' alt='' loading='lazy'>";
  return "";
}
async function setMainAssembly(pid, itemId){
  if (!pid){ toast("Open a project first", "err"); return; }
  try {
    const d = await api("PUT", "/projects/" + pid + "/main-assembly", { item_id: itemId || null });
    const upd = d.project;
    if (PMAP[pid]) PMAP[pid] = upd;
    const ix = PROJECTS.findIndex(x => x.id === pid);
    if (ix >= 0) PROJECTS[ix] = upd;
    toast(itemId ? "Main assembly set" : "Main assembly cleared", "ok");
    ALLITEMS = []; ALLITEMS_AT = 0;
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Main assembly: " + e.message, "err"); }
}
async function chooseMainAssembly(pid){
  const p = PMAP[pid] || {};
  let items = (ROUTE._items || []).filter(i => !i.archived_at);
  if (!items.length){
    try {
      const d = await api("GET", "/projects/" + pid + "/items");
      items = (d.items || []).filter(i => !i.archived_at);
    } catch(e){ toast("Load items: " + e.message, "err"); return; }
  }
  if (!items.length){ toast("No items in this project yet", "err"); return; }
  items = items.slice().sort((a, b) =>
    ((b.classification === "ASM") - (a.classification === "ASM")) ||
    a.part_number.localeCompare(b.part_number));
  const opts = items.map(i => ({ value: i.id,
    label: "[" + i.classification + "] " + i.part_number + " · " + (i.description || "(no description)") }));
  const f = await openModal("Main assembly", [
    { id:"item", label:"Item to build the BOM from", type:"select",
      options: opts, value: p.main_assembly_id || "" }
  ], "Set as main assembly");
  if (!f) return;
  await setMainAssembly(pid, f.item || null);
}

// ── item actions (shared) ───────────────────────────────────────────────
async function doCheckout(id){
  try {
    await api("POST", "/items/" + id + "/checkout", { workstation: location.hostname });
    toast("Checked out", "ok");
    if (ROUTE.name === "detail" && DETAIL.id === id) render(false);
    else CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Checkout: " + e.message, "err"); }
}
async function doUndo(id){
  try {
    await api("DELETE", "/items/" + id + "/checkout");
    toast("Checkout undone", "ok");
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Undo: " + e.message, "err"); }
}
async function saveVersionBlob(v, fallbackName){
  const buf = await api("GET", "/versions/" + v.id + "/blob");
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([buf]));
  a.download = fallbackName || ("version-" + v.seq);
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 4000);
}
// Bulk export of flat patterns: one DXF per sheet-metal part plus a
// manifest.json that reports which parts had no pattern yet.
async function exportFlatPatterns(ids){
  if (!ids.length) return toast("Select items to export first", "err");
  try {
    const buf = await api("GET", "/sheet-metal/flat-patterns?items=" + ids.join(","));
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([buf], { type: "application/zip" }));
    a.download = "flat-patterns.zip";
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 4000);
    toast("Flat patterns exported (" + ids.length + " item(s)) — see manifest.json for missing parts", "ok");
  } catch(e){ toast("Export failed: " + e.message, "err"); }
}

// ── Laser nesting (Sparrow) ─────────────────────────────────────────────
// ── Laser nest create panel ─────────────────────────────────────────────
// A full panel inside the Laser Nests view (not a modal), laid out like a
// professional nesting tool: a "Sheets | Parts" tab pair. The Sheets tab holds
// the stock size / grain / corner / spacing / origin; the Parts tab is a
// master-detail part list (each part has quantity, mirror mode, grain, step
// angle, priority, label, bounding-box / common-edge flags) plus the common
// part parameters. `NC.active` (module state, not a route flag) survives
// timer-driven re-renders so the panel is not clobbered by auto-refresh.
function ncDefaultState(){
  return {
    cands: [], lines: [], active: false, tab: "sheets", selPart: 0, selSheet: 0,
    units: "in",
    // Sheet-size definitions, in the current unit; the worker works through
    // them in order. quantity null = as many as needed.
    sheets: [ { width: 48, length: 96, asNeeded: true, quantity: 1 } ],
    margin: 0.39, grain: "along_length", corner: "lower_left", fill: "auto",
    guillotine: false, autoOrigins: true, ssd: 0, originX: 0, originY: 0,
    spacingX: 0.2, spacingY: 0.2, addLabels: false, optPairs: false, fillAll: false,
    partUnit: "auto",
  };
}
let NC = ncDefaultState();

// Apply the org's saved nest defaults (Settings → Laser nesting) to a fresh
// panel state; leaves anything the defaults don't set untouched.
function ncApplyDefaults(d){
  if (!d) return;
  if (d.units === "in" || d.units === "mm") NC.units = d.units;
  if (d.length != null) NC.sheets[0].length = d.length;
  if (d.width != null) NC.sheets[0].width = d.width;
  if (d.margin != null) NC.margin = d.margin;
  if (d.spacingX != null) NC.spacingX = d.spacingX;
  if (d.spacingY != null) NC.spacingY = d.spacingY;
  if (d.grainDirection) NC.grain = d.grainDirection;
  if (d.nestingCorner) NC.corner = d.nestingCorner;
  if (d.guillotine != null) NC.guillotine = d.guillotine;
  if (d.addPartLabels != null) NC.addLabels = d.addPartLabels;
  if (d.partUnit) NC.partUnit = d.partUnit;
}

function ncDefaultLine(){
  return { item_id: NC.cands[0] ? NC.cands[0].id : "", quantity:1, mirror_mode:"original",
    grain:"any", step_angle:90, priority:1, label:"", use_bounding_box:false,
    common_edge:false, min_qty:0, filler_qty:0 };
}

// Candidate sheet-metal parts, with flat-pattern availability + material.
async function loadNestableParts(){
  let specs = [];
  try { const d = await api("GET", "/sheet-metal"); specs = d.sheet_metal || []; } catch(e){}
  const items = await api("GET", "/items?limit=500").then(d => d.items || []).catch(() => []);
  const byName = {};
  items.forEach(it => { byName[it.id] = it.part_number || it.id.slice(0,8); });
  const seen = {};
  const cands = specs
    .filter(s => s.item_id && byName[s.item_id] && !seen[s.item_id])
    .map(s => {
      seen[s.item_id] = 1;
      return { id: s.item_id, name: byName[s.item_id],
        hasFlat: !!s.flat_pattern_sha, material: s.material || "" };
    })
    .sort((a,b) => a.name.localeCompare(b.name));
  return cands;
}

// ── option builders ──────────────────────────────────────────────────────
function ncOpt(pairs, sel){
  return pairs.map(([v,l]) =>
    "<option value='" + v + "'" + (v===sel?" selected":"") + ">" + l + "</option>").join("");
}
const NC_SHEET_GRAIN = [["along_length","Vertical"],["along_width","Horizontal"],["any","Ignore"]];
const NC_CORNER = [["lower_left","Lower Left"],["lower_right","Lower Right"],
  ["upper_left","Upper Left"],["upper_right","Upper Right"]];
const NC_FILL = [["auto","Calculate"],["lr","Left → Right"],["rl","Right → Left"],
  ["tb","Top → Bottom"],["bt","Bottom → Top"]];
const NC_PART_GRAIN = [["any","Ignore"],["along_length","Along length"],["along_width","Across"]];
// "Original Only" is the default: a flipped copy is cut with the sheet's bottom face up, which
// is wrong for sheet with one good face (brushed, coated, film-protected, laminated, veneered).
const NC_MIRROR = [["original","Original Only (do not mirror)"],["both","Both"],["mirror","Mirror Only"]];
const NC_STEP = [[90,"90°"],[45,"45°"],[30,"30°"],[180,"180°"]];

// ── previews ─────────────────────────────────────────────────────────────
function ncSheetPreview(i){
  const sh = NC.sheets[i] || NC.sheets[0];
  const L = +sh.length || 1, W = +sh.width || 1;
  const boxW = 320, boxH = 200, pad = 14;
  const s = Math.min((boxW-2*pad)/W, (boxH-2*pad)/L);
  const w = W*s, h = L*s;
  const x = (boxW-w)/2, y = (boxH-h)/2;
  let arrow = "";
  if (NC.grain === "along_length"){
    const cx = x + 10, cy = y + h/2, a = 18;
    arrow = "<line x1='"+cx+"' y1='"+(cy-a)+"' x2='"+cx+"' y2='"+(cy+a)+"' stroke='#3a3' stroke-width='1.5'/>" +
      "<polygon points='"+cx+","+(cy+a+4)+" "+(cx-3)+","+(cy+a)+" "+(cx+3)+","+(cy+a)+"' fill='#3a3'/>" +
      "<polygon points='"+cx+","+(cy-a-4)+" "+(cx-3)+","+(cy-a)+" "+(cx+3)+","+(cy-a)+"' fill='#3a3'/>" +
      "<text x='"+(cx+6)+"' y='"+(cy+3)+"' font-size='9' fill='#8a8'>GRAIN</text>";
  } else if (NC.grain === "along_width"){
    const cy = y + 10, cx = x + w/2, a = 18;
    arrow = "<line x1='"+(cx-a)+"' y1='"+cy+"' x2='"+(cx+a)+"' y2='"+cy+"' stroke='#3a3' stroke-width='1.5'/>" +
      "<polygon points='"+(cx+a+4)+","+cy+" "+(cx+a)+","+(cy-3)+" "+(cx+a)+","+(cy+3)+"' fill='#3a3'/>" +
      "<polygon points='"+(cx-a-4)+","+cy+" "+(cx-a)+","+(cy-3)+" "+(cx-a)+","+(cy+3)+"' fill='#3a3'/>" +
      "<text x='"+(cx+6)+"' y='"+(cy-6)+"' font-size='9' fill='#8a8'>GRAIN</text>";
  }
  return "<svg class='nc-preview nc-sheetpreview' viewBox='0 0 "+boxW+" "+boxH+"'>" +
    "<rect x='0' y='0' width='"+boxW+"' height='"+boxH+"' fill='#111'/>" +
    "<rect x='"+x+"' y='"+y+"' width='"+w+"' height='"+h+"' fill='none' stroke='#3c3' stroke-width='1.5'/>" +
    "<text x='"+(x+w/2)+"' y='"+(y+h-6)+"' font-size='10' fill='#6c6' text-anchor='middle'>"+
    Math.round(W) + " × " + Math.round(L) + " " + NC.units + "</text>" + arrow + "</svg>";
}
function ncPartPreview(){
  const boxW = 320, boxH = 200;
  const i = NC.selPart, l = NC.lines[i];
  const cand = l && NC.cands.find(c => c.id === l.item_id);
  const img = (cand && cand.hasFlat)
    ? "<span data-thumb='/items/" + cand.id + "/flat-pattern/preview' class='nc-partimg'></span>"
    : "<div class='nc-preview nc-partimg' style='display:flex;align-items:center;justify-content:center;color:#666;font-size:11px'>" +
      (cand ? "No flat pattern yet" : "Select a part") + "</div>";
  return "<div class='nc-previewwrap'>" + img +
    "<div class='nc-legend'>" +
    "<span><i style='background:#3c3'></i>Profile</span>" +
    "<span><i style='background:#c33'></i>Hole</span>" +
    "<span><i style='background:#888'></i>Ignore</span></div></div>";
}

// ── Sheets tab ───────────────────────────────────────────────────────────
function ncSheetsHtml(){
  const u = NC.units;
  const list = NC.sheets.map((sh, i) =>
    "<div class='nc-sheetrow" + (i===NC.selSheet?" sel":"") + "' data-i='"+i+"'>" +
    "Sheet #"+(i+1)+" — " + sh.width + " × " + sh.length + " " + u +
    (sh.asNeeded ? " · as needed" : " · ×" + sh.quantity) +
    "</div>").join("");
  const sh = NC.sheets[NC.selSheet] || NC.sheets[0];
  return "<div class='nc-cols'>" +
    "<div class='nc-col'>" +
      "<div class='nc-boxhead'>Sheet List <span class='dim'>" + NC.sheets.length + " of " + NC.sheets.length + "</span></div>" +
      "<div class='nc-sheetlist' id='ncSheetList'>" + list + "</div>" +
      "<div class='nc-partlistbtns'><button class='small' id='ncAddSheet'>+ Add sheet size</button>" +
      "<button class='small' id='ncRmSheet'>Remove</button></div>" +
      ncSheetPreview(NC.selSheet) +
      "<div class='nc-group'>" +
        "<div class='nc-grouptitle' id='ncSheetTitle'>Sheet #" + (NC.selSheet+1) + " — " + u + "</div>" +
        "<div class='nc-row2'>" +
          "<label>Width <span class='dim'>(" + u + ")</span><input id='nc_wid' value='" + sh.width + "'></label>" +
          "<label>Length <span class='dim'>(" + u + ")</span><input id='nc_len' value='" + sh.length + "'></label>" +
        "</div>" +
        "<div class='nc-row2'>" +
          "<label class='nc-check'><input id='nc_asneeded' type='checkbox'" + (sh.asNeeded?" checked":"") + "> As many as needed</label>" +
          "<label>Quantity<input id='nc_sqty' type='number' min='1' value='" + sh.quantity + "'" + (sh.asNeeded?" disabled":"") + "></label>" +
        "</div>" +
      "</div>" +
    "</div>" +
    "<div class='nc-col'>" +
      "<div class='nc-group'>" +
        "<div class='nc-grouptitle'>Common parameters (all sheets)</div>" +
        "<div class='nc-row'><label>Units of measure<select id='nc_units'>" + ncOpt([["in","Inches"],["mm","Millimetres"]], u) + "</select></label></div>" +
        "<div class='nc-row'><label>Grain direction<select id='nc_grain'>" + ncOpt(NC_SHEET_GRAIN, NC.grain) + "</select></label></div>" +
        "<div class='nc-row'><label>Nesting corner<select id='nc_corner'>" + ncOpt(NC_CORNER, NC.corner) + "</select></label></div>" +
        "<div class='nc-row'><label>Fill direction<select id='nc_fill'>" + ncOpt(NC_FILL, NC.fill) + "</select></label></div>" +
        "<div class='nc-row'><label class='nc-check'><input id='nc_gull' type='checkbox'" + (NC.guillotine?" checked":"") + "> Guillotine cut</label></div>" +
        "<div class='nc-row'><label class='nc-check'><input id='nc_autoorig' type='checkbox'" + (NC.autoOrigins?" checked":"") + "> Automatic sheet origins</label></div>" +
        "<div class='nc-row2'>" +
          "<label>Sheet–sheet distance <span class='dim'>(" + u + ")</span><input id='nc_ssd' value='" + NC.ssd + "'></label>" +
          "<label>Sheet margin <span class='dim'>(" + u + ")</span><input id='nc_margin' value='" + NC.margin + "'></label>" +
        "</div>" +
        "<div class='nc-row2'>" +
          "<label>Origin X <span class='dim'>(" + u + ")</span><input id='nc_ox' value='" + NC.originX + "'></label>" +
          "<label>Origin Y <span class='dim'>(" + u + ")</span><input id='nc_oy' value='" + NC.originY + "'></label>" +
        "</div>" +
      "</div>" +
    "</div>" +
  "</div>";
}
function ncRerenderSheets(){
  const box = $("ncSheetTab");
  if (box){ box.innerHTML = ncSheetsHtml(); ncBindSheets(); }
}
// Typing updates only the chrome (list rows, preview, title) so the input
// keeps focus; structural changes (select/add/remove a sheet) re-render all.
function ncSheetChrome(){
  const sl = $("ncSheetList");
  if (sl){
    sl.innerHTML = NC.sheets.map((s2, i) =>
      "<div class='nc-sheetrow" + (i===NC.selSheet?" sel":"") + "' data-i='"+i+"'>" +
      "Sheet #"+(i+1)+" — " + s2.width + " × " + s2.length + " " + NC.units +
      (s2.asNeeded ? " · as needed" : " · ×" + s2.quantity) + "</div>").join("");
    sl.querySelectorAll(".nc-sheetrow").forEach(row => {
      row.onclick = () => { NC.selSheet = +row.dataset.i; ncRerenderSheets(); };
    });
  }
  const pv = document.querySelector(".nc-sheetpreview");
  if (pv) pv.outerHTML = ncSheetPreview(NC.selSheet);
  const g = $("ncSheetTitle");
  if (g) g.textContent = "Sheet #" + (NC.selSheet+1) + " — " + NC.units;
}
function ncBindSheets(){
  const set = (id, fn) => { const el = $(id); if (el){ el.onchange = e => fn(e.target); el.oninput = e => fn(e.target); } };
  const sheet = NC.sheets[NC.selSheet];
  set("nc_wid", t => { sheet.width = t.value; ncSheetChrome(); });
  set("nc_len", t => { sheet.length = t.value; ncSheetChrome(); });
  set("nc_asneeded", t => { sheet.asNeeded = t.checked;
    const q = $("nc_sqty"); if (q) q.disabled = t.checked; ncSheetChrome(); });
  set("nc_sqty", t => { sheet.quantity = Math.max(1, parseInt(t.value,10)||1); ncSheetChrome(); });
  set("nc_units", t => { NC.units = t.value; ncSheetChrome(); });
  set("nc_ox", t => NC.originX = t.value);
  set("nc_oy", t => NC.originY = t.value);
  set("nc_grain", t => { NC.grain = t.value; ncSheetChrome(); });
  set("nc_corner", t => NC.corner = t.value);
  set("nc_fill", t => NC.fill = t.value);
  set("nc_gull", t => NC.guillotine = t.checked);
  set("nc_autoorig", t => NC.autoOrigins = t.checked);
  set("nc_ssd", t => NC.ssd = t.value);
  set("nc_margin", t => NC.margin = t.value);
  const add = $("ncAddSheet");
  if (add) add.onclick = () => {
    const last = NC.sheets[NC.sheets.length-1];
    NC.sheets.push({ width: last.width, length: last.length, asNeeded: false, quantity: 1 });
    NC.selSheet = NC.sheets.length - 1;
    ncRerenderSheets();
  };
  const rm = $("ncRmSheet");
  if (rm) rm.onclick = () => {
    if (NC.sheets.length <= 1) return;
    NC.sheets.splice(NC.selSheet, 1);
    NC.selSheet = Math.max(0, NC.selSheet - 1);
    ncRerenderSheets();
  };
}

// ── Parts tab: searchable picker + master–detail lines ───────────────────
function ncPartName(id){
  const c = NC.cands.find(x => x.id === id);
  return c ? c.name : (id ? id.slice(0,8) : "—");
}
// The picker: search box + checkable rows with flat-pattern thumbnails.
function ncPickerHtml(){
  const rows = NC.cands.map(c =>
    "<label class='nc-pickrow'>" +
    "<input type='checkbox' class='nc-pick' value='" + c.id + "' data-has='" + (c.hasFlat?"1":"0") + "'>" +
    "<span class='nc-pickthumb' data-id='" + c.id + "' data-thumb='/items/" + c.id + "/flat-pattern/preview'></span>" +
    "<span class='nc-pickname'>" + esc(c.name) +
    (c.hasFlat ? "" : " <span class='badge'>needs flat pattern</span>") +
    "</span></label>").join("");
  return "<div class='nc-group'>" +
    "<div class='nc-grouptitle'>Add parts</div>" +
    "<input id='ncSearch' class='nc-search' placeholder='Search parts…' autocomplete='off'>" +
    "<div class='nc-picklist' id='ncPickList'>" + (rows || "<div class='dim'>No sheet-metal parts found.</div>") + "</div>" +
    "<div class='row' style='margin-top:8px'><button class='small' id='ncAddSelected'>Add selected</button></div>" +
    "</div>";
}
function ncPartListHtml(){
  return NC.lines.map((l, i) =>
    "<div class='nc-partrow" + (i===NC.selPart?" sel":"") + "' data-i='"+i+"'>" +
    "<b>" + esc(ncPartName(l.item_id)) + "</b> ×" + l.quantity +
    (l.mirror_mode && l.mirror_mode!=="original" ? " · " + ({both:"flips allowed",mirror:"mirror-only"}[l.mirror_mode]) : "") +
    " <span class='nc-rmline' data-i='"+i+"' title='Remove this line'>×</span>" +
    "</div>").join("");
}
function ncPartParamsHtml(){
  const i = NC.selPart; const l = NC.lines[i];
  if (!l) return "<div class='dim'>No part selected.</div>";
  return "<div class='nc-group'>" +
    "<div class='nc-grouptitle'>Parameters — " + esc(ncPartName(l.item_id)) + "</div>" +
    "<div class='nc-row2'>" +
      "<label>Quantity<input id='nc_qty' type='number' min='1' value='"+l.quantity+"'></label>" +
      "<label>Minimum qty<input id='nc_minqty' type='number' min='0' value='"+l.min_qty+"'></label>" +
    "</div>" +
    "<div class='nc-row2'>" +
      "<label>Filler qty<input id='nc_fillqty' type='number' min='0' value='"+l.filler_qty+"'></label>" +
      "<label>Step angle<select id='nc_step'>" + ncOpt(NC_STEP, l.step_angle) + "</select></label>" +
    "</div>" +
    "<div class='nc-row2'>" +
      "<label>Priority<input id='nc_prio' type='number' min='1' value='"+l.priority+"'></label>" +
      "<label>Label<input id='nc_label' value='"+esc(l.label)+"' placeholder='P"+(i+1)+"'></label>" +
    "</div>" +
    "<div class='nc-row'><label>Grain direction<select id='nc_pgrain'>" + ncOpt(NC_PART_GRAIN, l.grain) + "</select></label></div>" +
    "<div class='nc-row'><label>Mirror<select id='nc_mirror'>" + ncOpt(NC_MIRROR, l.mirror_mode) + "</select></label></div>" +
    "<div class='nc-row2'>" +
      "<label class='nc-check'><input id='nc_usebb' type='checkbox'" + (l.use_bounding_box?" checked":"") + "> Use bounding box</label>" +
      "<label class='nc-check'><input id='nc_commedge' type='checkbox'" + (l.common_edge?" checked":"") + "> Common edge</label>" +
    "</div>" +
  "</div>" +
  "<div class='nc-group'>" +
    "<div class='nc-grouptitle'>Common parameters (all parts)</div>" +
    "<div class='nc-row2'>" +
      "<label>Part–part · horizontal <span class='dim'>(" + NC.units + ")</span><input id='nc_spx' value='"+NC.spacingX+"'></label>" +
      "<label>Part–part · vertical <span class='dim'>(" + NC.units + ")</span><input id='nc_spy' value='"+NC.spacingY+"'></label>" +
    "</div>" +
    "<div class='nc-row'><label class='nc-check'><input id='nc_addlabels' type='checkbox'" + (NC.addLabels?" checked":"") + "> Add part labels</label></div>" +
    "<div class='nc-row2'>" +
      "<label class='nc-check'><input id='nc_optpairs' type='checkbox'" + (NC.optPairs?" checked":"") + "> Optimized pairs</label>" +
      "<label class='nc-check'><input id='nc_fillall' type='checkbox'" + (NC.fillAll?" checked":"") + "> Fill all sheets</label>" +
    "</div>" +
    "<div class='nc-row'><label>Part source unit<select id='nc_punit'>" + ncOpt([["auto","Auto (file units)"],["in","Inches"],["mm","Millimetres"]], NC.partUnit) + "</select></label>" +
  "</div>";
}
function ncPartsHtml(){
  return "<div class='nc-cols'>" +
    "<div class='nc-col'>" +
      ncPickerHtml() +
      "<div class='nc-boxhead'>Part List <span class='dim'>" + NC.lines.length + " line(s)</span></div>" +
      "<div class='nc-partlist' id='ncPartList'>" + (NC.lines.length ? ncPartListHtml() : "<div class='dim'>No parts added.</div>") + "</div>" +
      ncPartPreview() +
    "</div>" +
    "<div class='nc-col' id='ncPartParams'>" + ncPartParamsHtml() + "</div>" +
  "</div>";
}
function ncRebindParts(){
  // Picker: live search filter + add-selected (independent lines may repeat a part).
  const search = $("ncSearch");
  if (search) search.oninput = () => {
    const q = search.value.trim().toLowerCase();
    document.querySelectorAll("#ncPickList .nc-pickrow").forEach(row => {
      const name = (row.querySelector(".nc-pickname").textContent || "").toLowerCase();
      row.style.display = (!q || name.includes(q)) ? "" : "none";
    });
  };
  const addSel = $("ncAddSelected");
  if (addSel) addSel.onclick = () => {
    const picked = [...document.querySelectorAll(".nc-pick:checked")].map(c => c.value);
    if (!picked.length) return toast("Tick one or more parts first", "err");
    picked.forEach(id => {
      NC.lines.push({ item_id: id, quantity: 1, mirror_mode: "original", grain: "any",
        step_angle: 90, priority: 1, label: "", use_bounding_box: false,
        common_edge: false, min_qty: 0, filler_qty: 0 });
    });
    NC.selPart = NC.lines.length - 1;
    ncRerenderParts();
  };
  const list = $("ncPartList");
  if (list){
    list.querySelectorAll(".nc-partrow").forEach(row => {
      row.onclick = e => {
        if (e.target.classList.contains("nc-rmline")) return;
        NC.selPart = +row.dataset.i;
        ncRerenderParts();
      };
    });
    // Per-line remove.
    list.querySelectorAll(".nc-rmline").forEach(btn => {
      btn.onclick = e => {
        e.stopPropagation();
        const i = +btn.dataset.i;
        NC.lines.splice(i, 1);
        if (!NC.lines.length) NC.lines.push(ncDefaultLine());
        NC.selPart = Math.min(NC.selPart, NC.lines.length - 1);
        ncRerenderParts();
      };
    });
  }
  // Flat-pattern thumbnails for the picker + preview; fall back to the
  // rendered 3D thumbnail when a part has no flat pattern yet.
  const tab = document.getElementById("ncPartsTab");
  if (tab) attachThumbs(tab).then(() => {
    tab.querySelectorAll(".nc-pickthumb").forEach(el => {
      if (el.querySelector("img") || !el.dataset.id) return;
      thumbUrl("/items/" + el.dataset.id + "/thumbnail").then(u => {
        if (u && !el.querySelector("img"))
          el.innerHTML = "<img src='" + u + "' alt='' loading='lazy'>";
      });
    });
  });
  // Selected-part field bindings.
  const l = NC.lines[NC.selPart];
  if (!l) return;
  const set = (id, fn) => { const el = $(id); if (el){ el.onchange = e => fn(e.target); el.oninput = e => fn(e.target); } };
  set("nc_qty", t => l.quantity = Math.max(1, parseInt(t.value,10)||1));
  set("nc_minqty", t => l.min_qty = parseInt(t.value,10)||0);
  set("nc_fillqty", t => l.filler_qty = parseInt(t.value,10)||0);
  set("nc_step", t => l.step_angle = parseFloat(t.value)||90);
  set("nc_prio", t => l.priority = parseInt(t.value,10)||1);
  set("nc_label", t => l.label = t.value);
  set("nc_pgrain", t => l.grain = t.value);
  set("nc_mirror", t => l.mirror_mode = t.value);
  set("nc_usebb", t => l.use_bounding_box = t.checked);
  set("nc_commedge", t => l.common_edge = t.checked);
  set("nc_spx", t => NC.spacingX = t.value);
  set("nc_spy", t => NC.spacingY = t.value);
  set("nc_addlabels", t => NC.addLabels = t.checked);
  set("nc_optpairs", t => NC.optPairs = t.checked);
  set("nc_fillall", t => NC.fillAll = t.checked);
  set("nc_punit", t => NC.partUnit = t.value);
}
function ncRerenderParts(){
  const box = $("ncPartsTab");
  if (box){ box.innerHTML = ncPartsHtml(); ncRebindParts(); }
}

async function renderNestCreate(v){
  NC.cands = await loadNestableParts();
  // Pre-fill the sheet/part defaults from the org's saved settings. This only
  // runs when the panel is opened fresh (timer re-renders keep the live DOM).
  try { const d = await api("GET", "/settings/nest-defaults"); ncApplyDefaults(d.defaults); }
  catch(e){}
  // Drop any preselection carried on the route (consumed into NC.lines by the
  // entry points) and seed a default line when none were added.
  delete ROUTE._parts;
  if (!NC.lines.length && NC.cands.length) NC.lines.push(ncDefaultLine());
  NC.selPart = 0;
  v.innerHTML =
    "<div class='head'><h2>New laser nest</h2>" +
    "<div class='nc-tabs'>" +
    "<button class='nc-tab" + (NC.tab==="sheets"?" on":"") + "' id='ncTabSheetsBtn'>Sheets</button>" +
    "<button class='nc-tab" + (NC.tab==="parts"?" on":"") + "' id='ncTabPartsBtn'>Parts</button>" +
    "</div>" +
    "<div class='grow'></div>" +
    "<label class='dim nc-namewrap'>Nest name <input id='nc_name' style='width:200px' value='Laser nest " + new Date().toISOString().slice(0,10) + "'></label>" +
    "<button class='small' id='ncBack'>← Back</button></div>" +
    "<div class='card nc'>" +
    "<div id='ncSheetTab' style='" + (NC.tab==="sheets"?"":"display:none") + "'>" + ncSheetsHtml() + "</div>" +
    "<div id='ncPartsTab' style='" + (NC.tab==="parts"?"":"display:none") + "'>" + ncPartsHtml() + "</div>" +
    "<div class='nc-foot'>" +
      "<span class='dim'>Parts nest on the selected sheets; the laser receives millimetres.</span>" +
      "<div class='grow'></div>" +
      "<button class='primary' id='ncQueue'>Queue nest</button>" +
    "</div>" +
    "</div>";
  $("ncTabSheetsBtn").onclick = () => ncSwitchTab("sheets");
  $("ncTabPartsBtn").onclick = () => ncSwitchTab("parts");
  $("ncBack").onclick = () => { NC.active = false; go("laser-nests"); };
  $("ncQueue").onclick = () => queueLaserNest();
  ncBindSheets();
  ncRebindParts();
}
function ncSwitchTab(tab){
  NC.tab = tab;
  $("ncSheetTab").style.display = tab === "sheets" ? "" : "none";
  $("ncPartsTab").style.display = tab === "parts" ? "" : "none";
  $("ncTabSheetsBtn").className = "nc-tab" + (tab === "sheets" ? " on" : "");
  $("ncTabPartsBtn").className = "nc-tab" + (tab === "parts" ? " on" : "");
  if (tab === "parts") attachThumbs($("ncPartsTab"));
}

async function queueLaserNest(){
  if (!NC.lines.length) return toast("Add at least one part", "err");
  if (NC.sheets.some(sh => !isFinite(num(sh.width)) || num(sh.width) <= 0 ||
      !isFinite(num(sh.length)) || num(sh.length) <= 0))
    return toast("Sheet width and length must be positive numbers", "err");
  const mirrorMap = { original: [false, false], both: [false, true], mirror: [true, false] };
  const lines = NC.lines.map(l => {
    const mm = mirrorMap[l.mirror_mode] || [false, false];
    return {
      item_id: l.item_id,
      quantity: Math.max(1, l.quantity | 0),
      mirror: mm[0], allow_mirror: mm[1],
      grain: l.grain || "any",
      step_angle: l.step_angle || 90,
      priority: l.priority || 1,
      label: l.label || "",
      use_bounding_box: !!l.use_bounding_box,
      common_edge: !!l.common_edge,
      min_qty: l.min_qty | 0,
      filler_qty: l.filler_qty | 0,
    };
  });
  const body = {
    name: $("nc_name").value.trim() || "Laser nest",
    units: NC.units,
    stock_width_mm: num(NC.sheets[0].width),
    stock_height_mm: num(NC.sheets[0].length),
    sheets: NC.sheets.map(sh => ({
      width_mm: num(sh.width),
      length_mm: num(sh.length),
      quantity: sh.asNeeded ? null : Math.max(1, sh.quantity | 0),
    })),
    edge_margin_mm: num(NC.margin),
    spacing_x_mm: num(NC.spacingX),
    spacing_y_mm: num(NC.spacingY),
    grain_direction: NC.grain,
    nesting_corner: NC.corner,
    origin_x_mm: num(NC.originX),
    origin_y_mm: num(NC.originY),
    sheet_sheet_distance_mm: num(NC.ssd),
    guillotine: NC.guillotine,
    add_part_labels: NC.addLabels,
    fill_direction: NC.fill,
    auto_sheet_origins: NC.autoOrigins,
    fill_all_sheets: NC.fillAll,
    optimized_pairs: NC.optPairs,
    default_part_unit: NC.partUnit !== "auto" ? NC.partUnit : undefined,
    parts: lines,
  };
  try {
    const q = $("ncQueue"); q.disabled = true; q.textContent = "Queueing…";
    const pre = await api("POST", "/laser-nests/validate", body);
    if (pre && pre.ok === false){
      const msgs = (pre.problems||[]).map(p=>p.problem);
      if (pre.mixed_material) msgs.push("mixed materials");
      if (pre.mixed_thickness) msgs.push("mixed thickness");
      toast("Preflight: " + msgs.join("; "), "err", 9000);
      q.disabled = false; q.textContent = "Queue nest";
      return;
    }
    const r = await api("POST", "/laser-nests", body);
    NC = ncDefaultState();
    toast("Laser nest queued (" + r.nest.status + ")", "ok");
    go("laser-nests");
  } catch(e){ toast("Create failed: " + e.message, "err");
    const q = $("ncQueue"); if (q){ q.disabled = false; q.textContent = "Queue nest"; } }
}

// Thin wrapper so the Items bulk action still works: it jumps to the Laser
// Nests view and opens the create panel (Parts tab) with those parts pre-added.
function openLaserNestDialog(itemIds){
  if (!itemIds.length) return toast("Select one or more items first", "err");
  NC = ncDefaultState();
  NC.lines = itemIds.map(id => ({ item_id: id, quantity: 1, mirror_mode: "original",
    grain: "any", step_angle: 90, priority: 1, label: "", use_bounding_box: false,
    common_edge: false, min_qty: 0, filler_qty: 0 }));
  NC.active = true; NC.tab = "parts"; NC.selPart = 0;
  go("laser-nests");
}
function num(s, d){ const n = parseFloat(s); return isFinite(n) ? n : (d === undefined ? NaN : d); }

// Laser Nests view: a create panel (when "New nest" is active) plus history.
async function viewLaserNests(v){
  if (NC.active){
    // Already rendered this panel (e.g. a timer re-render) → keep it as-is so
    // the user's in-progress form is not wiped.
    if (v.querySelector(".nc")) return;
    v.innerHTML = "<div class='head'><h2>Laser Nests</h2><span class='spinner'></span> Loading parts…</div>";
    try { await renderNestCreate(v); }
    catch(e){ v.innerHTML = "<div class='head'><h2>Laser Nests</h2></div>" +
      emptyState("ic-warning", "Load failed", e.message); }
    return;
  }
  let d;
  try { d = await api("GET", "/laser-nests?limit=50"); }
  catch(e){ v.innerHTML = "<div class='head'><h2>Laser Nests</h2></div>" + emptyState("ic-warning", "Load failed", e.message); return; }
  const nests = d.nests || [];
  v.innerHTML =
    "<div class='head'><h2>Laser Nests</h2>" +
    "<span class='dim'>Nesting runs built from flat-pattern DXFs via the Sparrow engine.</span>" +
    "<div class='grow'></div>" +
    "<button class='primary' id='lnNew'>+ New nest</button></div>" +
    (nests.length
      ? "<div class='card ln-list'>" + nests.map(nestRow).join("") + "</div>"
      : emptyState("ic-sheet-metal", "No laser nests yet",
          "Create one from sheet-metal parts, or from the <b>Items</b> view."));
  const nn = $("lnNew");
  if (nn) nn.onclick = () => { NC.active = true; NC.lines = []; go("laser-nests"); };
  v.querySelectorAll(".ln-row").forEach(row => {
    row.onclick = () => toggleNestDetail(row);
  });
}
function statusPill(s){
  const cls = { completed:"ok", running:"run", queued:"run", failed:"err", cancelled:"dim" }[s] || "dim";
  return "<span class='ln-status " + cls + "'>" + esc(s) + "</span>";
}
function nestRow(n){
  const grain = (n.grain_direction && n.grain_direction !== "any")
    ? " · grain " + esc(n.grain_direction.replace("along_","")) : "";
  return "<div class='ln-row' data-id='" + n.id + "'>" +
    "<div class='ln-top'><b>" + esc(n.name) + "</b> " + statusPill(n.status) +
    (n.stale ? " <span class='badge' title='" + esc(n.stale_reason || "source changed") + "'>stale</span>" : "") +
    " <span class='dim'>" + Math.round(n.stock_width_mm) + "×" + Math.round(n.stock_height_mm) + " mm" +
    grain +
    (n.material ? " · " + esc(n.material) : "") + " · " + fmtTime(n.created_at) + "</span></div>" +
    (n.error ? "<div class='dim ln-err'>" + esc(n.error) + "</div>" : "") +
    "<div class='ln-detail' id='ln-" + n.id + "'></div>" +
    "</div>";
}
async function toggleNestDetail(row){
  const id = row.dataset.id;
  const box = document.getElementById("ln-" + id);
  if (box.dataset.loaded === "1"){ box.style.display = box.style.display === "none" ? "" : "none"; return; }
  box.style.display = "";
  box.innerHTML = "<span class='dim'>loading…</span>";
  try {
    const d = await api("GET", "/laser-nests/" + id);
    const det = d.detail, n = det.nest;
    const sheets = det.sheets || [];
    const spacing = (n.spacing_x_mm || n.spacing_y_mm)
      ? " · spacing " + Math.round(n.spacing_x_mm) + "×" + Math.round(n.spacing_y_mm) + " mm" : "";
    const grain = (n.grain_direction && n.grain_direction !== "any")
      ? " · grain " + n.grain_direction.replace("along_","") : "";
    const corner = (n.nesting_corner && n.nesting_corner !== "lower_left")
      ? " · " + n.nesting_corner.replace(/_/g," ") : "";
    const defs = (n.sheet_defs && n.sheet_defs.length > 1)
      ? " · sizes: " + n.sheet_defs.map(d2 => Math.round(d2.widthMm) + "×" + Math.round(d2.lengthMm) + "mm" +
          (d2.quantity == null ? " (as needed)" : " ×" + d2.quantity)).join(", ") : "";
    const partLines = (det.parts||[]).map(p => {
      const tags = [];
      if (p.mirror) tags.push("mirrored");
      if (p.allow_mirror) tags.push("flips allowed");
      if (p.part_grain && p.part_grain !== "any") tags.push(p.part_grain.replace("along_","") + " grain");
      if (p.step_angle && p.step_angle !== 90) tags.push(p.step_angle + "° steps");
      if (p.use_bounding_box) tags.push("bbox");
      if (p.label) tags.push("“" + p.label + "”");
      return "<div class='ln-partline'><b>" + esc(p.part_number) + "</b> ×" + p.quantity +
        (tags.length ? " <span class='dim'>(" + tags.join(", ") + ")</span>" : "") + "</div>";
    }).join("");
    box.innerHTML =
      "<div class='ln-meta dim'>" + (det.parts||[]).reduce((a,p)=>a+p.quantity,0) +
      " part(s) across " + det.parts.length + " line item(s) · " + sheets.length + " sheet(s)" +
      spacing + grain + corner + defs + "</div>" +
      (partLines ? "<div class='ln-partlines dim'>" + partLines + "</div>" : "") +
      (sheets.length ? "<div class='ln-sheets'>" + sheets.map(s => sheetCard(n.id, s)).join("") + "</div>" :
        "<div class='dim'>No sheets yet " + (n.status === "completed" ? "(failed run)" : "(still " + n.status + ")") + ".</div>") +
      (n.status === "completed" ? "<div class='ln-actions'><button class='small' data-act='pkg'>Download package (.zip)</button>" +
        "<button class='small' data-act='cancel' hidden>Cancel</button>" +
        "<button class='small' data-act='rerun'>Re-run</button></div>" : "") +
      (n.status === "running" || n.status === "queued" ? "<div class='ln-actions'><button class='small' data-act='cancel'>Cancel</button></div>" : "");
    box.dataset.loaded = "1";
    if (box.querySelector("[data-act=pkg]"))
      box.querySelector("[data-act=pkg]").onclick = () => downloadNestPackage(n.id);
    const rr = box.querySelector("[data-act=rerun]"); if (rr) rr.onclick = () => rerunNest(n.id);
    const cc = box.querySelector("[data-act=cancel]"); if (cc) cc.onclick = () => cancelNest(n.id);
    box.querySelectorAll("[data-dl]").forEach(b =>
      b.onclick = e => { e.stopPropagation(); downloadNestSheet(n.id, b.dataset.dl); });
    // Previews (authenticated SVG).
    for (const s of sheets){
      if (!s.preview_sha) continue;
      fetch("/laser-nests/" + n.id + "/sheets/" + s.sheet_index + "/preview",
        { headers: { "Authorization": "Bearer " + TOKEN } })
        .then(r => r.ok ? r.blob() : Promise.reject(new Error("preview failed")))
        .then(b => { const el = box.querySelector("img[data-sheet='" + s.sheet_index + "']");
          if (el) el.src = URL.createObjectURL(b); })
        .catch(() => {});
    }
  } catch(e){
    box.innerHTML = "<span class='dim'>failed to load: " + esc(e.message) + "</span>";
  }
}
function sheetCard(nestId, s){
  return "<div class='ln-sheet'><img data-sheet='" + s.sheet_index + "' alt='sheet " + s.sheet_index + "' class='ln-preview'>" +
    "<div class='ln-sheetmeta'>Sheet " + s.sheet_index + " · " + s.placed_count + " part(s)" +
    (s.strip_width_mm ? " · strip " + Math.round(s.strip_width_mm) + " mm" : "") +
    (s.utilization ? " · " + Math.round(s.utilization*100) + "% fill" : "") +
    "</div>" +
    (s.dxf_sha ? "<button class='small' data-dl='" + s.sheet_index + "'>DXF</button>" : "") +
    "</div>";
}
async function downloadNestSheet(nestId, idx){
  try {
    const buf = await api("GET", "/laser-nests/" + nestId + "/sheets/" + idx + "/dxf");
    saveBuffer(buf, "nest-sheet" + idx + ".dxf", "application/dxf");
  } catch(e){ toast("Download failed: " + e.message, "err"); }
}
async function downloadNestPackage(nestId){
  try {
    const buf = await api("GET", "/laser-nests/" + nestId + "/package");
    saveBuffer(buf, "nest-package.zip", "application/zip");
  } catch(e){ toast("Package download failed: " + e.message, "err"); }
}
function saveBuffer(buf, name, type){
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([buf], { type }));
  a.download = name; a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 4000);
}
async function cancelNest(nestId){
  try { await api("POST", "/laser-nests/" + nestId + "/cancel"); toast("Nest cancelled", "ok"); go("laser-nests"); }
  catch(e){ toast("Cancel failed: " + e.message, "err"); }
}
async function rerunNest(nestId){
  try { const r = await api("POST", "/laser-nests/" + nestId + "/rerun"); toast("Re-run queued: " + r.nest.status, "ok"); go("laser-nests"); }
  catch(e){ toast("Re-run failed: " + e.message, "err"); }
}
async function downloadLatest(it){
  try {
    const d = await api("GET", "/items/" + it.id);
    const head = d.head_version;
    if (!head){ toast("No versions to download", "err"); return; }
    await saveVersionBlob(head, head.filename || it.filename || it.part_number);
  } catch(e){ toast("Download failed: " + e.message, "err"); }
}
async function checkinItem(itemId, file, msg, filename){
  const cur = await api("GET", "/items/" + itemId);
  if (cur.lock){
    if (cur.lock.user_id !== myId())
      throw new Error("checked out by " + lockName(cur.lock) + " — undo their checkout or wait for the lease to expire");
  } else {
    await api("POST", "/items/" + itemId + "/checkout", { workstation: location.hostname });
  }
  const buf = await file.arrayBuffer();
  const up = await api("PUT", "/blobs", null, new Uint8Array(buf));
  const head = cur.head_version;
  const body = {
    sha256: up.blob.sha256, size: up.blob.size_bytes,
    base_version: head ? head.seq : null,
    commit_message: msg || "", filename: filename || file.name,
    mime_type: file.type || null,
  };
  try {
    const d = await api("POST", "/items/" + itemId + "/checkin", body);
    ALLITEMS = [];
    return { version: d.version, cur };
  } catch(e){
    if (e.status === 409){
      const h2 = (await api("GET", "/items/" + itemId)).head_version;
      const err = new Error("v" + (h2 ? h2.seq : "?") + " was committed by " +
        (h2 ? (h2.author_name || "someone") : "another session") +
        " while you were checking in. Refresh to pick up the new base, then retry.");
      err.conflict = true;
      throw err;
    }
    throw e;
  }
}

// Open immutable vault bytes in a same-origin Craft editor. Keep the base pinned
// to the version actually opened, rather than silently rebasing stale edits.
async function openWebEditor(itemId, versionId){
  try {
    const d = await api("GET", "/items/" + itemId);
    let version = versionId ? (d.versions || []).find(v => v.id === versionId) : d.head_version;
    if (!version) throw new Error("No version available to open");
    const openedVersionId = version.id;
    const filename = sourceFilename(version, d) || d.item.filename || "";
    const app = PdmEditors.forFile(version.mime_type || (d.file && d.file.mime_type), filename);
    if (!app) throw new Error("No web editor is available for this file type");
    const name = filename || (d.item.part_number + "." + app.extension);
    await PdmEditors.open({ app, name, version,
      load: async () => {
        const r = await fetch("/versions/" + openedVersionId + "/blob", { headers: { Authorization:"Bearer " + TOKEN } });
        if (!r.ok) throw new Error("HTTP " + r.status);
        return r.arrayBuffer();
      },
      checkin: isRole("editor") ? async (buffer, message) => {
        const cur = await api("GET", "/items/" + itemId);
        if (!cur.head_version || cur.head_version.id !== version.id)
          throw new Error("The vault head has changed since this file was opened. Download your edits and compare with the latest version.");
        if (cur.lock && cur.lock.user_id !== myId()) throw new Error("The item is checked out by " + lockName(cur.lock));
        if (!cur.lock) await api("POST", "/items/" + itemId + "/checkout", { workstation:location.hostname });
        const up = await api("PUT", "/blobs", null, new Uint8Array(buffer));
        const result = await api("POST", "/items/" + itemId + "/checkin", {
          sha256:up.blob.sha256, base_version:version.seq, commit_message:message,
          filename:name, mime_type:app.mime,
        });
        version = result.version;
        ALLITEMS = []; ALLITEMS_AT = 0;
        return version;
      } : null,
      onSaved: () => { if (CUR_RELOAD) CUR_RELOAD(); },
    });
  } catch(e){ toast("Web editor: " + e.message, "err"); }
}

// ── bulk / folder / owner / upload ──────────────────────────────────────
async function moveItems(pid, ids){
  const folders = ROUTE._folders ||
    await api("GET", "/projects/" + pid + "/folders").then(d => d.folders).catch(() => []);
  const f = await openModal("Move " + ids.length + " item(s) to folder", [
    { id:"folder", label:"Folder", type:"select",
      options:[{value:"",label:"(project root)"}].concat(folders.map(x=>({value:x.id,label:x.name}))) }
  ], "Move");
  if (!f) return;
  try {
    for (const id of ids) await api("PUT", "/items/" + id, { folder_id: f.folder || null });
    toast("Moved " + ids.length + " item(s)", "ok");
    ALLITEMS = [];
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Move failed: " + e.message, "err"); }
}
async function assignOwner(ids){
  const f = await openModal("Assign owner", [
    { id:"owner", label:"Owner", type:"select",
      options: USERS.map(u=>({value:u.id,label:u.display_name||u.email})) }
  ], "Assign");
  if (!f) return;
  try {
    for (const id of ids) await api("PUT", "/items/" + id, { owner_id: f.owner });
    toast("Owner updated for " + ids.length + " item(s)", "ok");
    ALLITEMS = [];
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Assign failed: " + e.message, "err"); }
}
async function checkoutBulk(ids){
  try {
    for (const id of ids) await api("POST", "/items/" + id + "/checkout", { workstation: location.hostname });
    toast("Checked out " + ids.length + " item(s)", "ok");
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Checkout: " + e.message, "err"); }
}
async function uploadModal(pid, files){
  const folders = ROUTE._folders || [];
  const extra = files && files.length
    ? "<div class='modal-note' style='margin-bottom:10px'>" +
      [...files].map(f => "• " + esc(f.name) + " (" + bytes(f.size) + ")").join("<br>") + "</div>"
    : null;
  // Classification options: the org's configured categories (Settings ▸ Part-number
  // prefixes) first, then the legacy classes that aren't configured yet. The part-number
  // prefix is resolved server-side from the chosen category.
  const configured = (await api("GET", "/settings/part-number-prefixes")
    .catch(() => ({ prefixes: [] }))).prefixes || [];
  const clsOpts = configured.map(p => ({ value:p.category, label:p.category + " (" + p.prefix + ")" }));
  for (const legacy of ["BE","ASM","MFG","SC"])
    if (!configured.some(p => p.category.toLowerCase() === legacy))
      clsOpts.push({ value:legacy, label:legacy + " (no prefix configured)" });
  return openModal("Upload " + (files ? files.length + " file(s)" : "file(s)"), [
    { id:"desc", label:"Item description (one item per file; blank = filename)", placeholder:"e.g. Drain tray" },
    { id:"pn", label:"Part number (blank = auto-assign from the category prefix)" },
    { id:"cls", label:"Category", type:"select", options: clsOpts.length ? clsOpts : ["BE"], value: clsOpts[0] ? clsOpts[0].value : "BE" },
    { id:"folder", label:"Folder", type:"select",
      options:[{value:"",label:"(project root)"}].concat(folders.map(x=>({value:x.id,label:x.name}))) },
    { id:"msg", label:"Commit message", placeholder:"e.g. initial upload" },
  ], "Upload", extra);
}
async function doUpload(pid, f, files){
  if (!files.length){ toast("No files to upload", "err"); return; }
  const proj = PMAP[pid];
  let n = 0;
  try {
    for (const file of files){
      const desc = f.desc || file.name.replace(/\.[^.]+$/, "");
      const pn = f.pn ? (files.length > 1 ? f.pn + "-" + (n + 1) : f.pn) : null;
      const d = await api("POST", "/projects/" + pid + "/items", {
        description: desc, part_number: pn, classification: f.cls,
        folder_id: f.folder || null });
      const r = await checkinItem(d.item.id, file, f.msg || "upload");
      n++;
      toast("Uploaded " + n + "/" + files.length + " — " + d.item.part_number + " v" + r.version.seq, "ok", 5000);
    }
  } catch(e){
    toast("Upload stopped at file " + (n + 1) + " (" + (proj ? proj.name : pid) + "): " + e.message, "err", 7000);
  }
  ALLITEMS = [];
  if ((ROUTE.name === "explorer" && ROUTE.pid === pid) ||
      (ROUTE.name === "detail" && DETAIL.data)) render(false);
  else CUR_RELOAD && CUR_RELOAD();
}
function pickFiles(pid){
  const inp = document.createElement("input");
  inp.type = "file"; inp.multiple = true;
  inp.onchange = async () => {
    const files = [...inp.files];
    if (!files.length) return;
    const f = await uploadModal(pid, files);
    if (f) doUpload(pid, f, files);
  };
  inp.click();
}

// ── dashboard ───────────────────────────────────────────────────────────
function docCards(items){
  if (!items.length)
    return emptyState("ic-download", "Nothing modified yet",
      "Upload a file into a project and it shows up here with its rendered preview.");
  return items.map(it => {
    const k = kindInfo(it.mime_type, it.filename);
    return "<div class='dcard' data-id='" + it.id + "'>" +
      "<span class='thumb-lg' data-thumb='/items/" + it.id + "/thumbnail'></span>" +
      "<div class='nm'>" + esc(itemName(it)) + " " + revBadge(it.latest_revision) + "</div>" +
      "<div class='mt'>" + esc(it._project || "") + " · " + k.icon + " " + esc(k.label) +
      " · " + fmtTime(it.modified_at) + "</div></div>";
  }).join("");
}
function projCards(list){
  if (!list.length)
    return emptyState("ic-project", "No projects yet",
      "A project groups folders and items under one part-number namespace.",
      "<div><button class='cta' data-go='projects'>Create your first project</button></div>");
  return list.map(p => "<div class='pcard" + (p.archived_at ? " archived" : "") + "' data-pid='" + p.id + "'>" +
    projectCover(p, "pcard-cover") +
    "<div class='t'>" + kindSvg("ic-project") + " " + esc(p.name) + archBadge(p.archived_at) + "</div>" +
    "<div class='m'>" + esc(p.description || "") + "</div>" +
    "<div class='m'>created " + fmtTime(p.created_at) + "</div>" +
    "<button class='kebab-btn' data-kb='proj' data-kid='" + p.id + "' title='Project actions'>⋮</button>" +
    "</div>").join("");
}
function collabChips(){
  if (!USERS.length) return "<span class='dim'>loading…</span>";
  return USERS.map(u => "<span class='collab'>" + userAv(u, !u.active) +
    "<span>" + esc(u.display_name || u.email) + "</span>" +
    "<span class='dim'>" + esc(u.role) + "</span></span>").join("");
}
function apprMiniTable(list){
  if (!list.length) return "<div class='empty'>Nothing pending — items that route to you appear here.</div>";
  return "<table><thead><tr><th>Item</th><th>Requested by</th><th>Since</th><th></th></tr></thead><tbody>" +
    list.map(a => "<tr class='clickable' data-aid='" + a.id + "'>" +
      "<td class='mono'>" + esc(a.item_part_number || "no item") + "</td>" +
      "<td class='muted'>" + esc(a.requested_by_name || "—") + "</td>" +
      "<td class='muted'>" + fmtTime(a.created_at) + "</td>" +
      "<td style='white-space:nowrap'>" +
      (isRole("editor") ?
        "<button class='small' data-ap='" + a.id + "'>Approve</button> " +
        "<button class='small' data-rj='" + a.id + "'>Reject</button>" : "") +
      "</td></tr>").join("") +
    "</tbody></table>";
}
function jobsTable(list){
  if (!list.length) return "<div class='empty'>No background jobs.</div>";
  const stat = s => s === "done" ? "done" : (s === "failed" ? "failed" :
    (s === "pending" ? "pending" : "active"));
  return "<table><thead><tr><th>Job</th><th>Status</th><th>Attempts</th><th>Since</th><th>Error</th></tr></thead><tbody>" +
    list.map(j => "<tr><td>" + esc(j.kind) + "</td>" +
      "<td><span class='jobstat " + stat(j.status) + "'>" + esc(j.status) + "</span></td>" +
      "<td class='muted'>" + j.attempts + "/" + j.max_attempts + "</td>" +
      "<td class='muted'>" + fmtTime(j.created_at) + "</td>" +
      "<td class='dim mono' style='max-width:170px;overflow:hidden;text-overflow:ellipsis' title='" + esc(j.last_error || "") + "'>" +
      esc(j.last_error || "") + "</td></tr>").join("") +
    "</tbody></table>";
}
async function viewDashboard(v){
  if (!PROJECTS.length || !USERS.length) await loadCommon();
  const home = await api("GET", "/home").catch(() => null);
  const k = home && home.kpis ? home.kpis : null;
  const firstRun = k && !k.items && !PROJECTS.length;
  const recent = await api("GET", "/items?limit=5&sort=modified_at&dir=desc")
    .then(d => (d.items || []).map(it => Object.assign({}, it,
      { _project: PMAP[it.project_id] ? PMAP[it.project_id].name : "—" })))
    .catch(() => []);
  const mfgRel = await api("GET", "/items?limit=5&classification=MFG&state=released&sort=modified_at&dir=desc")
    .then(d => (d.items || []).map(it => Object.assign({}, it,
      { _project: PMAP[it.project_id] ? PMAP[it.project_id].name : "—" })))
    .catch(() => []);
  v.innerHTML =
    "<div class='head'><h2>Dashboard</h2>" +
    "<span class='dim muted' id='refreshstatus'>" + refreshModeLabel() + "</span></div>" +
    (firstRun ?
      "<div class='card' style='margin-bottom:14px'>" +
      "<h2 class='sec'>Get started</h2>" +
      "<p class='dim' style='margin:0 0 10px'>Three steps and the workspace is live:</p>" +
      "<div class='row' style='margin-bottom:6px'><b>1.</b> Create a project " +
      "<button class='small' id='onb1'>Open projects</button></div>" +
      "<div class='row' style='margin-bottom:6px'><b>2.</b> Upload a file into it " +
      "<button class='small' id='onb2'>How?</button></div>" +
      "<div class='row'><b>3.</b> Invite a teammate " +
      "<button class='small' id='onb3'>Administration</button></div>" +
      "</div>" : "") +
    kpiStrip(k) +
    "<div class='grid2'>" +
      "<div><h2 class='sec'>Activity</h2>" +
        feed(home ? home.recent_audit : [], 8) +
        (home && home.recent_audit && home.recent_audit.length ?
          "<div class='row' style='margin-top:6px'><button id='dallact' class='small'>All activity →</button></div>" : "") +
      "</div>" +
      "<div>" +
        "<h2 class='sec'>Recently modified documents</h2>" +
        "<div class='cards'>" + docCards(recent) + "</div>" +
        "<h2 class='sec'>Projects</h2>" +
        "<div class='cards'>" +
          projCards(PROJECTS.filter(p => !p.archived_at).slice(0, 4)) + "</div>" +
        "<h2 class='sec'>My approvals</h2>" +
        apprMiniTable(home && home.my_approvals ? home.my_approvals : []) +
        "<h2 class='sec'>Active jobs</h2>" +
        jobsTable(home && home.active_jobs ? home.active_jobs : []) +
        "<h2 class='sec'>Active collaborators</h2>" +
        "<div class='chips'>" + collabChips() + "</div>" +
        "<h2 class='sec'>Released manufacturing packages</h2>" +
        (mfgRel.length ? itemsTable(mfgRel, true) :
          "<div class='empty'>No released manufacturing packages yet.</div>") +
      "</div>" +
    "</div>";
  v.querySelectorAll(".dcard[data-id]").forEach(c => c.addEventListener("click", () => openDetail(c.dataset.id)));
  v.querySelectorAll(".pcard[data-pid]").forEach(c => c.addEventListener("click", () => go("explorer", { pid: c.dataset.pid })));
  v.querySelectorAll(".kpi.clickable[data-knav]").forEach(c =>
    c.addEventListener("click", () => go(c.dataset.knav)));
  const g = v.querySelector("[data-go='projects']");
  if (g) g.onclick = () => go("projects");
  const o1 = $("onb1"), o2 = $("onb2"), o3 = $("onb3");
  if (o1) o1.onclick = () => go("projects");
  if (o2) o2.onclick = () => go("projects");
  if (o3) o3.onclick = () => go("admin");
  const da = $("dallact");
  if (da) da.onclick = () => go("activity");
  v.querySelectorAll("[data-ap]").forEach(b =>
    b.addEventListener("click", e => { e.stopPropagation(); decideApproval(b.dataset.ap, "approve"); }));
  v.querySelectorAll("[data-rj]").forEach(b =>
    b.addEventListener("click", e => { e.stopPropagation(); decideApproval(b.dataset.rj, "reject"); }));
  v.querySelectorAll("tr.clickable[data-aid]").forEach(tr =>
    tr.addEventListener("click", () => {
      const a = (home && home.my_approvals || []).find(x => x.id === tr.dataset.aid);
      if (a && a.item_id) openDetail(a.item_id);
    }));
  wireItemTable(v, {});
  wireKebabs(v);
  await attachThumbs(v);
  refreshApprovalsBadge(k);
}

// ── projects ────────────────────────────────────────────────────────────
async function viewProjects(v){
  if (!PROJECTS.length) await loadCommon();
  const counts = {};
  try {
    const d = await api("GET", "/items?limit=1000&include_archived=true");
    for (const it of d.items || []) counts[it.project_id] = (counts[it.project_id] || 0) + 1;
  } catch(e){}
  const sorted = [...PROJECTS].sort((a, b) => (Date.parse(b.created_at) || 0) - (Date.parse(a.created_at) || 0));
  v.innerHTML =
    "<div class='head'><h2>Projects</h2><div class='grow'></div>" +
    "<button id='newproj' class='primary'>＋ New project</button></div>" +
    (sorted.length ? "<div class='cards'>" +
    sorted.map(p => "<div class='pcard" + (p.archived_at ? " archived" : "") + "' data-pid='" + p.id + "'>" +
       projectCover(p, "pcard-cover") +
       "<div class='t'>" + kindSvg("ic-project") + " " + esc(p.name) + archBadge(p.archived_at) + "</div>" +
      "<div class='m'>" + esc(p.description || "no description") + "</div>" +
      "<div class='m'>" + (counts[p.id] || 0) + " items · created " + fmtTime(p.created_at) + "</div>" +
      "<button class='kebab-btn' data-kb='proj' data-kid='" + p.id + "' title='Project actions'>⋮</button>" +
      "</div>").join("") +
    "</div>" :
    emptyState("ic-project", "No projects yet",
      "A project is the top-level container for folders and items — create one to start uploading.",
      "<div><button class='cta' id='newproj2'>＋ New project</button></div>"));
  v.querySelectorAll(".pcard[data-pid]").forEach(c => c.addEventListener("click", () => go("explorer", { pid: c.dataset.pid })));
  wireKebabs(v);
  attachThumbs(v);
  const startNew = async () => {
    const f = await openModal("New project", [
      { id:"name", label:"Name", required:true, placeholder:"e.g. Structural Frames" },
      { id:"description", label:"Description", placeholder:"optional" }
    ], "Create project");
    if (!f) return;
    try {
      const d = await api("POST", "/projects", { name: f.name, description: f.description || null });
      toast("Project “" + d.project.name + "” created", "ok");
      await loadCommon();
      go("explorer", { pid: d.project.id });
    } catch(e){ toast("Create failed: " + e.message, "err"); }
  };
  $("newproj").onclick = startNew;
  const n2 = $("newproj2");
  if (n2) n2.onclick = startNew;
}

// ── items (org-wide, server-filtered; replaces files + parts) ───────────
async function viewItems(v){
  if (!PROJECTS.length || !USERS.length) await loadCommon();
  if (!ROUTE._kind) ROUTE._kind = "all";
  if (!ROUTE._cls) ROUTE._cls = "all";
  if (ROUTE._sm === undefined) ROUTE._sm = "all";
  if (ROUTE._q === undefined) ROUTE._q = "";
  if (!ROUTE._sort) ROUTE._sort = "modified_at";
  if (!ROUTE._dir) ROUTE._dir = "desc";
  if (!ROUTE._page) ROUTE._page = 1;
  if (!(ROUTE._sel instanceof Set)) ROUTE._sel = new Set();
  const params = new URLSearchParams({
    limit: "100", page: String(ROUTE._page),
    sort: ROUTE._sort, dir: ROUTE._dir });
  if (ROUTE._kind !== "all") params.set("kind", ROUTE._kind);
  if (ROUTE._cls !== "all") params.set("classification", ROUTE._cls);
  if (ROUTE._sm === "on") params.set("sheet_metal", "true");
  if (ROUTE._sm === "off") params.set("sheet_metal", "false");
  if (ROUTE._q) params.set("q", ROUTE._q);
  const d = await api("GET", "/items?" + params.toString());
  const loaded = (d.items || []).map(it => Object.assign({}, it, {
    _project_id: it.project_id,
    _project: PMAP[it.project_id] ? PMAP[it.project_id].name : "—" }));
  const total = d.total != null ? d.total : loaded.length;
  const pages = Math.max(1, Math.ceil(total / (d.limit || 100)));
  const chip = (key, attr, val, label) =>
    "<span class='chip" + (ROUTE[key] === val ? " on" : "") + "' data-" + attr + "='" + val + "'>" + label + "</span>";
  v.innerHTML =
    "<div class='head'><h2>Items</h2>" +
    "<span class='dim muted'>" + total + " item(s) across all projects · filters run on the server</span></div>" +
    "<div class='chips' style='margin-bottom:6px'>" +
      chip("_kind","k","all","All kinds") +
      ["cad","img","pdf","dxf","txt"].map(k => chip("_kind","k",k,k.toUpperCase())).join("") + "</div>" +
    "<div class='chips' style='margin-bottom:10px'>" +
      chip("_cls","c","all","All classes") +
      ["BE","ASM","MFG","SC"].map(c => chip("_cls","c",c,c)).join("") + "</div>" +
    "<div class='chips' style='margin-bottom:6px'>" +
      chip("_sm","sm","all","All parts") +
      chip("_sm","sm","on","SHEET METAL") +
      chip("_sm","sm","off","Not sheet metal") + "</div>" +
    "<div class='row' style='margin-bottom:8px'>" +
      "<input id='iq' placeholder='filter: name, part no, description…' value='" + esc(ROUTE._q) + "' style='width:280px'>" +
      "<select id='isort'>" +
        "<option value='modified_at'>Last modified</option>" +
        "<option value='part_number'>Part number</option>" +
        "<option value='created_at'>Created</option>" +
        "<option value='classification'>Classification</option>" +
      "</select>" +
      "<select id='idir'>" +
        "<option value='desc'>newest first</option>" +
        "<option value='asc'>oldest first</option>" +
      "</select>" +
      "<div class='grow'></div>" +
      (pages > 1 ?
        "<button id='iprev' class='small'" + (ROUTE._page <= 1 ? " disabled" : "") + ">← Prev</button> " +
        "<span class='dim'>page " + ROUTE._page + " / " + pages + "</span> " +
        "<button id='inext' class='small'" + (ROUTE._page >= pages ? " disabled" : "") + ">Next →</button>"
        : "") +
    "</div>" +
    "<div id='itwrap'></div>";
  const wrap = $("itwrap");
  const bulkBar = () => {
    const n = ROUTE._sel.size;
    if (!n) return "";
    return "<div class='bulkbar'><b>" + n + " selected</b>" +
      "<button class='small' id='bsmflat'>Export flat patterns</button>" +
      "<button class='small primary' id='blast'>Create laser nest</button>" +
      "<div class='grow'></div>" +
      "<button class='small' id='bsmclear'>Clear</button></div>";
  };
  const fill = async (list) => {
    wrap.innerHTML = bulkBar() + (list.length ? itemsTable(list, true, ROUTE._sel) :
      emptyState("ic-items", "No items match",
        "Upload a file into a project, or widen the kind / class / text filters.",
        "<div><button class='cta' id='iproj'>Open projects</button></div>"));
    const ip = $("iproj"); if (ip) ip.onclick = () => go("projects");
    const bf = $("bsmflat"); if (bf) bf.onclick = () => exportFlatPatterns([...ROUTE._sel]);
    const bl = $("blast"); if (bl) bl.onclick = () => openLaserNestDialog([...ROUTE._sel]);
    const bc = $("bsmclear"); if (bc) bc.onclick = () => { ROUTE._sel.clear(); fill(shown()); };
    await attachThumbs(wrap);
    wireItemTable(wrap, {
      onSort: k => {
        if (k === "_name" || k === "owner_name"){
          ROUTE._lsort = ROUTE._lsort === k ? null : k;
          ROUTE._ldir = ROUTE._ldir === "asc" ? "desc" : "asc";
        } else {
          ROUTE._lsort = null;
          if (ROUTE._sort === k) ROUTE._dir = ROUTE._dir === "asc" ? "desc" : "asc";
          else { ROUTE._sort = k; ROUTE._dir = "desc"; }
          ROUTE._page = 1;
          render(false);
          return;
        }
        fill();
      },
      onSel: () => fill(shown()),
      selSet: ROUTE._sel,
    });
  };
  const shown = () => {
    let list = loaded;
    if (ROUTE._lsort) list = sortItems(list, ROUTE._lsort, ROUTE._ldir || "asc");
    return list;
  };
  await fill(shown());
  $("isort").value = ROUTE._sort;
  $("idir").value = ROUTE._dir;
  $("iq").addEventListener("keydown", e => {
    if (e.key === "Enter"){ ROUTE._q = e.target.value.trim(); ROUTE._page = 1; render(false); }
  });
  $("isort").onchange = e => { ROUTE._sort = e.target.value; ROUTE._page = 1; render(false); };
  $("idir").onchange = e => { ROUTE._dir = e.target.value; ROUTE._page = 1; render(false); };
  v.querySelectorAll(".chip[data-k]").forEach(c => c.addEventListener("click", () => {
    ROUTE._kind = c.dataset.k; ROUTE._page = 1; render(false);
  }));
  v.querySelectorAll(".chip[data-c]").forEach(c => c.addEventListener("click", () => {
    ROUTE._cls = c.dataset.c; ROUTE._page = 1; render(false);
  }));
  v.querySelectorAll(".chip[data-sm]").forEach(c => c.addEventListener("click", () => {
    ROUTE._sm = c.dataset.sm; ROUTE._page = 1; render(false);
  }));
  const ip = $("iprev"), inx = $("inext");
  if (ip) ip.onclick = () => { ROUTE._page = Math.max(1, ROUTE._page - 1); render(false); };
  if (inx) inx.onclick = () => { ROUTE._page = Math.min(pages, ROUTE._page + 1); render(false); };
}

// ── machines & tools registry (MFG master data) ─────────────────────────
async function viewMachines(v){
  if (!PROJECTS.length) await loadCommon();
  const [md, td, ncd] = await Promise.all([
    api("GET", "/machines").catch(() => ({ machines: [] })),
    api("GET", "/tools").catch(() => ({ tools: [] })),
    api("GET", "/nc-programs").catch(() => ({ nc_programs: [] }))
  ]);
  const machines = md.machines || [], tools = td.tools || [];
  const inUse = id => (ncd.nc_programs || []).filter(p => p.machine_id === id).length;
  v.innerHTML =
    "<div class='head'><h2>Machines &amp; Tools</h2>" +
    "<span class='dim muted'>shop-floor master data referenced by NC programs and production jobs</span>" +
    "<div class='grow'></div>" +
    "<button id='newmachine' class='small'>＋ Machine</button>" +
    "<button id='newtool' class='primary small'>＋ Tool</button></div>" +
    "<div class='card'><h2 class='sec'>Machines (" + machines.length + ")</h2>" +
      (machines.length ?
        "<table><thead><tr><th>Name</th><th>Kind</th><th>Manufacturer / model</th>" +
        "<th>Axes</th><th>In use</th><th>Maintenance</th></tr></thead><tbody>" +
        machines.map(m => "<tr>" +
          "<td><b>" + esc(m.name) + "</b></td>" +
          "<td class='muted'>" + esc(m.kind || "—") + "</td>" +
          "<td class='muted'>" + esc(((m.manufacturer || "") + " " + (m.model || "")).trim() || "—") + "</td>" +
          "<td class='muted'>" + esc((m.axes || []).join(", ")) + "</td>" +
          "<td class='muted'>" + (inUse(m.id) || "—") + "</td>" +
          "<td class='muted'>" + esc(m.maintenance_status || "—") + "</td>" +
          "</tr>").join("") + "</tbody></table>" :
        emptyState("ic-machine", "No machines yet",
          "Register the CNC mills, lathes, lasers and printers that production jobs run on.")) +
    "</div>" +
    "<div class='card' style='margin-top:12px'><h2 class='sec'>Tool library (" + tools.length + ")</h2>" +
      (tools.length ?
        "<table><thead><tr><th>Name</th><th>Kind</th><th>Inventory</th><th>Materials</th></tr></thead><tbody>" +
        tools.map(t => "<tr>" +
          "<td><b>" + esc(t.name) + "</b></td>" +
          "<td class='muted'>" + esc(t.kind || "—") + "</td>" +
          "<td class='muted'>" + (t.inventory != null ? t.inventory : "—") + "</td>" +
          "<td class='muted dim'>" + esc((t.material_compatibility || []).join(", ")) + "</td>" +
          "</tr>").join("") + "</tbody></table>" :
        emptyState("ic-tool", "No tools yet",
          "Add the end mills, drills and other tooling that CAM programs select from.")) +
    "</div>";
  $("newmachine").onclick = async () => {
    const f = await openModal("New machine", [
      { id:"name", label:"Name", required:true, placeholder:"e.g. Shop mill 1" },
      { id:"kind", label:"Kind", type:"select",
        options:["cnc_mill","cnc_lathe","cnc_laser","router","printer","other"], value:"cnc_mill" },
      { id:"manufacturer", label:"Manufacturer", placeholder:"optional" },
      { id:"model", label:"Model", placeholder:"optional" },
      { id:"maintenance", label:"Maintenance status", type:"select",
        options:["ok","due","overdue"], value:"ok" },
    ], "Add machine");
    if (!f) return;
    try {
      await api("POST", "/machines", {
        name: f.name, kind: f.kind,
        manufacturer: f.manufacturer || null, model: f.model || null,
        maintenance_status: f.maintenance || null });
      toast("Machine added", "ok");
      render(false);
    } catch(e){ toast("Add machine: " + e.message, "err"); }
  };
  $("newtool").onclick = async () => {
    const f = await openModal("New tool", [
      { id:"name", label:"Name", required:true, placeholder:"e.g. 10 mm 4-flute end mill" },
      { id:"kind", label:"Kind", type:"select", options:TOOL_KINDS, value:"end_mill" },
      { id:"inventory", label:"Inventory count", placeholder:"0" },
      { id:"materials", label:"Compatible materials (comma-separated)", placeholder:"e.g. Aluminum, Steel" },
    ], "Add tool");
    if (!f) return;
    try {
      await api("POST", "/tools", {
        name: f.name, kind: f.kind,
        inventory: f.inventory === "" ? null : Number(f.inventory),
        material_compatibility: f.materials
          ? f.materials.split(",").map(s => s.trim()).filter(Boolean) : null });
      toast("Tool added", "ok");
      render(false);
    } catch(e){ toast("Add tool: " + e.message, "err"); }
  };
}

// ── tool library (General / Cutter / Cutting data / Post processor) ─────
const TOOL_KINDS = [
  { value:"end_mill", label:"End mill" }, { value:"drill", label:"Drill" },
  { value:"tap", label:"Tap" }, { value:"reamer", label:"Reamer" },
  { value:"face_mill", label:"Face mill" }, { value:"chamfer", label:"Chamfer" },
  { value:"turning", label:"Turning" }, { value:"holder", label:"Holder" },
  { value:"insert", label:"Insert" }];
function kindLabel(k){ const f = TOOL_KINDS.find(x => x.value === k);
  return f ? f.label : (k || "—"); }
// Geometry fields offered per tool kind; values stored in dimensions JSONB,
// in the tool's units (mm or in).
const TOOL_GEOMETRY = {
  end_mill: [["diameter","Diameter"],["flute_length","Cutting length"],
    ["overall_length","Overall length"],["corner_radius","Corner radius"],
    ["shank_diameter","Shank diameter"]],
  drill: [["diameter","Diameter"],["overall_length","Overall length"],
    ["point_angle","Point angle (deg)"],["shank_diameter","Shank diameter"]],
  tap: [["diameter","Major diameter"],["thread_pitch","Thread pitch"],
    ["overall_length","Overall length"],["chamfer_angle","Chamfer angle (deg)"]],
  reamer: [["diameter","Diameter"],["flute_length","Cutting length"],
    ["overall_length","Overall length"]],
  face_mill: [["diameter","Diameter"],["cutter_diameter","Cutter diameter"],
    ["overall_length","Overall length"]],
  chamfer: [["diameter","Diameter"],["chamfer_angle","Chamfer angle (deg)"],
    ["overall_length","Overall length"]],
  turning: [["width","Insert width"],["length","Insert length"],
    ["thickness","Insert thickness"],["nose_radius","Nose radius"]],
  holder: [["shank_diameter","Shank diameter"],["collet_size","Collet size"],
    ["overall_length","Overall length"]],
  insert: [["width","Insert width"],["length","Insert length"],
    ["thickness","Insert thickness"],["nose_radius","Nose radius"]],
};
const TOOL_MATERIALS = ["Solid carbide","Coated carbide","HSS","HSS (coated)","Carbide (indexable)",
  "Cermet","Ceramic","Diamond (PCD)","CBN","High-speed steel","Other"];
const FINISHES = ["roughing","semi-finishing","finishing","deep slotting","general"];

async function viewTools(v){
  const d = await api("GET", "/tools").catch(() => ({ tools: [] }));
  const tools = d.tools || [];
  v.innerHTML =
    "<div class='head'><h2>Tool library</h2>" +
    "<span class='dim muted'>cutter geometry, cutting data and machine tool numbers</span>" +
    "<div class='grow'></div>" +
    "<button id='newtool' class='primary'>＋ New tool</button></div>" +
    "<div class='card'>" +
    (tools.length ?
      "<table><thead><tr><th>Name</th><th>Type</th><th>Vendor</th><th>Part no.</th>" +
      "<th>Units</th><th>Flutes</th><th>Tool no.</th><th>Inventory</th></tr></thead><tbody>" +
      tools.map(t => "<tr class='toolrow' data-id='" + t.id + "' style='cursor:pointer'>" +
        "<td><b>" + esc(t.name) + "</b>" +
        (t.description ? "<div class='dim'>" + esc(t.description).slice(0, 90) + "</div>" : "") + "</td>" +
        "<td class='muted'>" + esc(kindLabel(t.kind)) + "</td>" +
        "<td class='muted'>" + esc(t.vendor || "—") + "</td>" +
        "<td class='muted mono'>" + esc(t.part_number || "—") + "</td>" +
        "<td class='muted'>" + esc(t.units || "mm") + "</td>" +
        "<td class='muted'>" + (t.flutes != null ? t.flutes : "—") + "</td>" +
        "<td class='muted mono'>" + (t.tool_number != null ? "T" + t.tool_number : "—") + "</td>" +
        "<td class='muted'>" + (t.inventory != null ? t.inventory : "—") + "</td>" +
        "</tr>").join("") + "</tbody></table>" :
      emptyState("ic-tool", "No tools yet",
        "Create tools with their cutter geometry, cutting data and machine tool numbers.")) +
    "</div>";
  v.querySelectorAll(".toolrow").forEach(r =>
    r.addEventListener("click", () => go("tool", { toolid: r.dataset.id, _tab:"general" })));
  $("newtool").onclick = async () => {
    const f = await openModal("New tool", [
      { id:"name", label:"Name", required:true,
        placeholder:"e.g. 10 - Ø0.38645\" R0.03543\" (3/8 High Feed O2R 2FL)" },
      { id:"kind", label:"Type of tool", type:"select", options:TOOL_KINDS },
      { id:"units", label:"Units", type:"select",
        options:[{ value:"mm", label:"Millimetre (mm)" }, { value:"in", label:"Inch (in)" }] },
      { id:"vendor", label:"Vendor", placeholder:"optional" },
      { id:"part_number", label:"Part number", placeholder:"optional" },
      { id:"description", label:"Description", type:"textarea", placeholder:"optional" },
    ], "Create tool");
    if (!f) return;
    try {
      const r = await api("POST", "/tools", {
        name: f.name, kind: f.kind, units: f.units || "mm",
        vendor: f.vendor || null, part_number: f.part_number || null,
        description: f.description || null });
      go("tool", { toolid: r.tool.id, _tab:"general" });
    } catch(e){ toast("Create tool: " + e.message, "err"); }
  };
}

async function viewToolDetail(v){
  if (ROUTE._tab){ TOOL.tab = ROUTE._tab; delete ROUTE._tab; }
  const id = ROUTE.toolid;
  const d = await api("GET", "/tools/" + id);
  TOOL.id = id; TOOL.data = d.tool; TOOL.cutting = d.cutting_data || []; TOOL.docs = d.documents || [];
  const t = TOOL.data;
  const tabCount = k => k === "cutting" ? TOOL.cutting.length : k === "general" ? TOOL.docs.length : 0;
  v.innerHTML =
    "<div class='head'><button id='tback' class='small'>← Tool library</button>" +
    "<h2>" + esc(t.name) + "</h2>" +
    "<span class='dim muted'>" + esc(kindLabel(t.kind)) + " · " + esc(t.units || "mm") +
    (t.tool_number != null ? " · T" + t.tool_number : "") + "</span>" +
    "<div class='grow'></div>" +
    "<button id='tsave' class='primary small'>Save</button>" +
    "<button id='tdel' class='small'>Delete</button></div>" +
    "<div class='tabs'>" +
    [["general","General"],["cutter","Cutter"],["cutting","Cutting data"],["post","Post processor"]]
      .map(x => "<div class='tab" + (TOOL.tab === x[0] ? " on" : "") + "' data-t='" + x[0] + "'>" +
        x[1] + (tabCount(x[0]) ? " <span class='count'>" + tabCount(x[0]) + "</span>" : "") +
        "</div>").join("") +
    "</div><div id='ttab'></div>";
  $("tback").onclick = () => go("tools");
  v.querySelectorAll(".tab").forEach(x => x.addEventListener("click", () => {
    TOOL.tab = x.dataset.t;
    v.querySelectorAll(".tab").forEach(y => y.classList.toggle("on", y === x));
    pushHash(); renderToolTab();
  }));
  $("tsave").onclick = saveTool;
  $("tdel").onclick = async () => {
    if (!confirm("Delete tool \"" + t.name + "\"? Its cutting data and documents are removed too.")) return;
    try { await api("DELETE", "/tools/" + id); toast("Tool deleted", "ok"); go("tools"); }
    catch(e){ toast("Delete tool: " + e.message, "err"); }
  };
  renderToolTab();
}
function renderToolTab(){
  const v = $("ttab");
  if (!v) return;
  if (TOOL.tab === "cutter") renderToolCutter(v);
  else if (TOOL.tab === "cutting") renderToolCutting(v);
  else if (TOOL.tab === "post") renderToolPost(v);
  else renderToolGeneral(v);
}
function tf(label, inputHtml){
  return "<div class='tfield'><label>" + label + "</label>" + inputHtml + "</div>";
}
function renderToolGeneral(v){
  const t = TOOL.data;
  v.innerHTML =
    "<div class='card'>" +
    tf("Description", "<textarea id='g-desc' rows='3'>" + esc(t.description || "") + "</textarea>") +
    "<div class='tgrid'>" +
    tf("Vendor", "<input id='g-vendor' value='" + esc(t.vendor || "") + "' placeholder='optional'>") +
    tf("Part number", "<input id='g-pn' class='mono' value='" + esc(t.part_number || "") + "' placeholder='optional'>") +
    tf("Link", "<input id='g-link' value='" + esc(t.link || "") + "' placeholder='https://…'>") +
    "</div>" +
    "<div style='margin-top:14px'><button id='tsave2' class='primary small'>Save general</button></div></div>" +
    "<div class='card' style='margin-top:12px'><h2 class='sec'>Documents (" + TOOL.docs.length + ")</h2>" +
    (TOOL.docs.length ?
      "<table><thead><tr><th>Name</th><th>Size</th><th>Added</th><th></th></tr></thead><tbody>" +
      TOOL.docs.map(doc => "<tr>" +
        "<td><b>" + kindBadge(doc.content_type, doc.filename) + " " + esc(doc.filename) + "</b></td>" +
        "<td class='muted'>" + bytes(doc.size_bytes) + "</td>" +
        "<td class='muted'>" + fmtTime(doc.created_at) + "</td>" +
        "<td style='text-align:right;white-space:nowrap'>" +
        "<button class='small' data-dl='" + doc.id + "'>Download</button> " +
        "<button class='small' data-dx='" + doc.id + "'>Remove</button></td></tr>").join("") +
      "</tbody></table>" :
      emptyState("ic-file", "No documents attached",
        "Attach datasheets, drawings or vendor files to this tool.")) +
    "<div style='margin-top:10px'><input type='file' id='g-file' class='hidden'>" +
    "<button id='gup' class='small primary'>⬆ Upload document</button></div></div>";
  $("tsave2").onclick = saveTool;
  $("gup").onclick = () => $("g-file").click();
  $("g-file").onchange = e => { uploadToolDoc(e.target.files[0]); e.target.value = ""; };
  v.querySelectorAll("[data-dl]").forEach(b =>
    b.onclick = () => downloadToolDoc(TOOL.docs.find(x => x.id === b.dataset.dl)));
  v.querySelectorAll("[data-dx]").forEach(b =>
    b.onclick = async () => {
      try { await api("DELETE", "/tools/" + TOOL.id + "/documents/" + b.dataset.dx);
        TOOL.docs = TOOL.docs.filter(x => x.id !== b.dataset.dx);
        toast("Document removed", "ok"); renderToolTab();
      } catch(e){ toast("Remove document: " + e.message, "err"); }
    });
}
function renderToolCutter(v){
  const t = TOOL.data;
  const dims = t.dimensions || {};
  const geoms = TOOL_GEOMETRY[t.kind] || [];
  v.innerHTML =
    "<div class='card'>" +
    "<div class='tgrid'>" +
    tf("Type of tool", "<select id='c-kind'>" +
      TOOL_KINDS.map(k => "<option value='" + k.value + "'" +
        (k.value === t.kind ? " selected" : "") + ">" + k.label + "</option>").join("") + "</select>") +
    tf("Units", "<select id='c-units'>" +
      ["mm","in"].map(u => "<option value='" + u + "'" + (u === (t.units || "mm") ? " selected" : "") + ">" +
        (u === "mm" ? "Millimetre (mm)" : "Inch (in)") + "</option>").join("") + "</select>") +
    tf("Rotation", "<select id='c-rotation'>" +
      ["cw","ccw"].map(r => "<option value='" + r + "'" + (r === (t.rotation || "cw") ? " selected" : "") + ">" +
        (r === "cw" ? "Clockwise (CW)" : "Counter-clockwise (CCW)") + "</option>").join("") + "</select>") +
    tf("Flutes / cutting edges", "<input id='c-flutes' type='number' min='0' step='1' value='" +
      (t.flutes != null ? t.flutes : "") + "'>") +
    "</div>" +
    "<div class='tsec'><h3>Material</h3>" +
    tf("Cutter material", "<select id='c-material'>" +
      "<option value=''></option>" +
      (t.tool_material && !TOOL_MATERIALS.includes(t.tool_material) ?
        "<option value='" + esc(t.tool_material) + "' selected>" + esc(t.tool_material) + "</option>" : "") +
      TOOL_MATERIALS.map(m => "<option" + (m === t.tool_material ? " selected" : "") + ">" + m + "</option>").join("") +
      "</select>") +
    "</div>" +
    (geoms.length ?
      "<div class='tsec'><h3>Geometry (" + esc(kindLabel(t.kind)) + ", in " + esc(t.units || "mm") + ")</h3>" +
      "<div class='tgrid'>" +
      geoms.map(g => "<div class='tfield'><label>" + g[1] + "</label>" +
        "<input id='geom-" + g[0] + "' type='number' step='any' value='" +
        (dims[g[0]] != null ? esc(String(dims[g[0]])) : "") + "'></div>").join("") +
      "</div></div>" : "") +
    "<div style='margin-top:14px'><button id='tsave3' class='primary small'>Save cutter</button></div></div>";
  $("c-kind").onchange = () => renderToolTab();
  $("tsave3").onclick = saveTool;
}
function renderToolCutting(v){
  const u = TOOL.data.units || "mm";
  v.innerHTML =
    "<div class='card'><h2 class='sec'>Cutting data (" + TOOL.cutting.length + ")</h2>" +
    "<div class='dim muted' style='margin-bottom:10px'>One row per material / finish required. Feed values are in " +
    esc(u) + "/min, feed-per-tooth in " + esc(u) + ", spindle in rpm.</div>" +
    (TOOL.cutting.length ?
      "<table><thead><tr><th>Label</th><th>Material</th><th>Finish</th><th>Spindle (rpm)</th>" +
      "<th>Cutting feed</th><th>Feed/tooth</th><th></th></tr></thead><tbody>" +
      TOOL.cutting.map(c => "<tr>" +
        "<td><b>" + esc(c.label || "—") + "</b></td>" +
        "<td class='muted'>" + esc(c.material || "—") + "</td>" +
        "<td class='muted'>" + esc(c.finish || "—") + "</td>" +
        "<td class='mono muted'>" + (c.spindle_speed != null ? c.spindle_speed : "—") + "</td>" +
        "<td class='mono muted'>" + (c.cutting_feedrate != null ? c.cutting_feedrate + " " + u + "/min" : "—") + "</td>" +
        "<td class='mono muted'>" + (c.feed_per_tooth != null ? c.feed_per_tooth + " " + u : "—") + "</td>" +
        "<td style='text-align:right;white-space:nowrap'>" +
        "<button class='small' data-ce='" + c.id + "'>Edit</button> " +
        "<button class='small' data-cd='" + c.id + "'>Delete</button></td></tr>").join("") +
      "</tbody></table>" :
      emptyState("ic-tool", "No cutting data yet",
        "Add speeds and feedrates for the materials and finishes this tool is used on.")) +
    "<div style='margin-top:10px'><button id='cadd' class='small primary'>＋ Add cutting data</button></div></div>";
  $("cadd").onclick = () => addOrEditCutting(null);
  v.querySelectorAll("[data-ce]").forEach(b =>
    b.onclick = () => addOrEditCutting(TOOL.cutting.find(x => x.id === b.dataset.ce)));
  v.querySelectorAll("[data-cd]").forEach(b =>
    b.onclick = async () => {
      if (!confirm("Delete this cutting-data row?")) return;
      try { await api("DELETE", "/tools/" + TOOL.id + "/cutting-data/" + b.dataset.cd);
        TOOL.cutting = TOOL.cutting.filter(x => x.id !== b.dataset.cd);
        toast("Cutting data deleted", "ok"); renderToolTab();
      } catch(e){ toast("Delete cutting data: " + e.message, "err"); }
    });
}
function cuttingDataFields(existing, u){
  const g = k => existing && existing[k] != null ? String(existing[k]) : "";
  return [
    { id:"label", label:"Label", placeholder:"e.g. Steel — roughing" },
    { id:"material", label:"Material", placeholder:"e.g. Aluminum 6061" },
    { id:"finish", label:"Finish required", type:"select",
      options:FINISHES, value:existing ? (existing.finish || "roughing") : "roughing" },
    { id:"spindle_speed", label:"Spindle speed (rpm)", placeholder:"e.g. 9000", value:g("spindle_speed") },
    { id:"surface_speed", label:"Surface speed", placeholder:"optional", value:g("surface_speed") },
    { id:"ramp_spindle_speed", label:"Ramp spindle speed (rpm)", placeholder:"optional", value:g("ramp_spindle_speed") },
    { id:"cutting_feedrate", label:"Cutting feedrate (" + u + "/min)", placeholder:"e.g. 41.5", value:g("cutting_feedrate") },
    { id:"feed_per_tooth", label:"Feed per tooth (" + u + ")", placeholder:"e.g. 0.007", value:g("feed_per_tooth") },
    { id:"lead_in_feedrate", label:"Lead-in feedrate (" + u + "/min)", placeholder:"optional", value:g("lead_in_feedrate") },
    { id:"lead_out_feedrate", label:"Lead-out feedrate (" + u + "/min)", placeholder:"optional", value:g("lead_out_feedrate") },
    { id:"transition_feedrate", label:"Transition feedrate (" + u + "/min)", placeholder:"optional", value:g("transition_feedrate") },
    { id:"ramp_feedrate", label:"Ramp feedrate (" + u + "/min)", placeholder:"optional", value:g("ramp_feedrate") },
    { id:"ramp_angle", label:"Ramp angle (deg)", placeholder:"optional", value:g("ramp_angle") },
    { id:"plunge_feedrate", label:"Plunge feedrate (" + u + "/min)", placeholder:"optional", value:g("plunge_feedrate") },
    { id:"plunge_feed_per_rev", label:"Plunge feed per revolution (" + u + ")", placeholder:"optional", value:g("plunge_feed_per_rev") },
  ];
}
function numOrNull(s){ s = String(s ?? "").trim(); if (s === "") return null;
  const v = Number(s); return isNaN(v) ? null : v; }
async function addOrEditCutting(existing){
  const u = TOOL.data.units || "mm";
  const f = await openModal(existing ? "Edit cutting data" : "Add cutting data",
    cuttingDataFields(existing, u), existing ? "Save" : "Add");
  if (!f) return;
  const body = {
    label: f.label || null, material: f.material || null, finish: f.finish,
    spindle_speed: numOrNull(f.spindle_speed), surface_speed: numOrNull(f.surface_speed),
    ramp_spindle_speed: numOrNull(f.ramp_spindle_speed),
    cutting_feedrate: numOrNull(f.cutting_feedrate), feed_per_tooth: numOrNull(f.feed_per_tooth),
    lead_in_feedrate: numOrNull(f.lead_in_feedrate),
    lead_out_feedrate: numOrNull(f.lead_out_feedrate),
    transition_feedrate: numOrNull(f.transition_feedrate),
    ramp_feedrate: numOrNull(f.ramp_feedrate), ramp_angle: numOrNull(f.ramp_angle),
    plunge_feedrate: numOrNull(f.plunge_feedrate),
    plunge_feed_per_rev: numOrNull(f.plunge_feed_per_rev),
  };
  try {
    if (existing){
      const r = await api("PUT", "/tools/" + TOOL.id + "/cutting-data/" + existing.id, body);
      const i = TOOL.cutting.findIndex(x => x.id === existing.id);
      if (i >= 0) TOOL.cutting[i] = r.cutting_data;
      toast("Cutting data saved", "ok");
    } else {
      const r = await api("POST", "/tools/" + TOOL.id + "/cutting-data", body);
      TOOL.cutting.push(r.cutting_data);
      toast("Cutting data added", "ok");
    }
    renderToolTab();
  } catch(e){ toast("Cutting data: " + e.message, "err"); }
}
function renderToolPost(v){
  const t = TOOL.data;
  v.innerHTML =
    "<div class='card'><h2 class='sec'>Post processor</h2>" +
    "<div class='dim muted' style='margin-bottom:10px'>The tool number this cutter is loaded at on the machine, " +
    "as written in the tool table by the post processor.</div>" +
    "<div class='tgrid'>" +
    tf("Tool number in machine", "<input id='p-tnum' type='number' min='0' step='1' value='" +
      (t.tool_number != null ? t.tool_number : "") + "' placeholder='e.g. 10'>") +
    "</div>" +
    (t.tool_number != null ?
      "<div class='dim' style='margin-top:8px'>Currently T" + t.tool_number + ".</div>" : "") +
    "<div style='margin-top:14px'><button id='tsave4' class='primary small'>Save post processor</button></div></div>";
  $("tsave4").onclick = saveTool;
}
function collectToolEdits(){
  const txt = id => { const el = $(id); return el ? (el.value.trim() || null) : undefined; };
  const num = id => { const el = $(id); if (!el) return undefined;
    const s = el.value.trim(); if (s === "") return null;
    const v = Number(s); return isNaN(v) ? null : v; };
  const edits = {};
  const set = (k, val) => { if (val !== undefined) edits[k] = val; };
  set("description", txt("g-desc"));
  set("vendor", txt("g-vendor"));
  set("part_number", txt("g-pn"));
  set("link", txt("g-link"));
  set("kind", txt("c-kind"));
  set("units", txt("c-units"));
  set("rotation", txt("c-rotation"));
  set("flutes", num("c-flutes"));
  set("tool_material", txt("c-material"));
  set("tool_number", num("p-tnum"));
  if ($("c-kind")) {
    const dims = TOOL.data.dimensions || {};
    for (const [k] of TOOL_GEOMETRY[TOOL.data.kind] || []) {
      const el = $("geom-" + k);
      if (!el) continue;
      const s = el.value.trim();
      if (s === "") delete dims[k];
      else { const val = Number(s); dims[k] = isNaN(val) ? s : val; }
    }
    for (const k of Object.keys(dims)) {
      if (!(TOOL_GEOMETRY[TOOL.data.kind] || []).some(x => x[0] === k)) delete dims[k];
    }
    set("dimensions", Object.keys(dims).length ? dims : null);
  }
  return edits;
}
async function saveTool(){
  try {
    const edits = collectToolEdits();
    if (!Object.keys(edits).length){ toast("Nothing to save", "ok"); return; }
    TOOL.data = (await api("PUT", "/tools/" + TOOL.id, edits)).tool;
    toast("Tool saved", "ok");
    render(false);
  } catch(e){ toast("Save tool: " + e.message, "err"); }
}
async function uploadToolDoc(file){
  if (!file) return;
  try {
    const r = await api("PUT", "/blobs", undefined, file);
    const d = await api("POST", "/tools/" + TOOL.id + "/documents", {
      filename: file.name, blob_id: r.blob.id, content_type: file.type || null });
    TOOL.docs.push(d.document);
    toast("Document attached", "ok");
    renderToolTab();
  } catch(e){ toast("Upload document: " + e.message, "err"); }
}
async function downloadToolDoc(doc){
  if (!doc) return;
  try {
    const r = await fetch("/tools/" + TOOL.id + "/documents/" + doc.id + "/download",
      { headers: { "Authorization": "Bearer " + TOKEN } });
    if (!r.ok) throw new Error("HTTP " + r.status);
    const u = URL.createObjectURL(await r.blob());
    const a = document.createElement("a");
    a.href = u; a.download = doc.filename; document.body.appendChild(a);
    a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(u), 10000);
  } catch(e){ toast("Download: " + e.message, "err"); }
}

// ── post-processor registry ─────────────────────────────────────────────
async function viewPostProcessors(v){
  const d = await api("GET", "/post-processors").catch(() => ({ post_processors: [] }));
  const list = d.post_processors || [];
  v.innerHTML =
    "<div class='head'><h2>Post-processors</h2>" +
    "<span class='dim muted'>G-code dialect generators pinned per machine kind</span>" +
    "<div class='grow'></div>" +
    "<button id='newpp' class='primary'>＋ Post-processor</button></div>" +
    (list.length ?
      "<table><thead><tr><th>Name</th><th>Dialect</th><th>Machine kind</th><th>Version</th><th>Added</th></tr></thead><tbody>" +
      list.map(p => "<tr>" +
        "<td><b>" + esc(p.name) + "</b></td>" +
        "<td class='mono muted'>" + esc(p.dialect || "—") + "</td>" +
        "<td class='muted'>" + esc(p.machine_kind || "—") + "</td>" +
        "<td class='muted'>" + esc(p.version || "—") + "</td>" +
        "<td class='muted'>" + fmtDate(p.created_at) + "</td>" +
        "</tr>").join("") + "</tbody></table>" :
      emptyState("ic-tool", "No post-processors yet",
        "Add the G-code post-processors NC programs are generated with."));
  $("newpp").onclick = async () => {
    const f = await openModal("New post-processor", [
      { id:"name", label:"Name", required:true, placeholder:"e.g. Haas VF-2 G-code" },
      { id:"dialect", label:"Dialect", type:"select",
        options:["gcode","rs274ngc","fanuc","heidenhain","other"], value:"gcode" },
      { id:"machine_kind", label:"Machine kind", type:"select",
        options:["cnc_mill","cnc_lathe","cnc_laser","router","printer","other"], value:"cnc_mill" },
      { id:"version", label:"Version", placeholder:"optional" },
    ], "Add post-processor");
    if (!f) return;
    try {
      await api("POST", "/post-processors", {
        name: f.name, dialect: f.dialect,
        machine_kind: f.machine_kind, version: f.version || null });
      toast("Post-processor added", "ok");
      render(false);
    } catch(e){ toast("Add post-processor: " + e.message, "err"); }
  };
}

// ── manufacturing hub (P0-1 console against existing routes) ────────────
const MFG_STEPS = ["needs_cad","in_cam","ready_for_machining","being_machined",
  "awaiting_inspection","completed"];
const MFG_STEP_LABEL = { needs_cad:"needs CAD", in_cam:"in CAM",
  ready_for_machining:"ready to machine", being_machined:"machining",
  awaiting_inspection:"inspection", completed:"done" };
function mfgStepper(status){
  if (status === "rejected")
    return "<div class='stepper'><span class='s failed'>rejected</span></div>";
  const idx = MFG_STEPS.indexOf(status);
  if (idx < 0) return "<span class='dim'>" + esc(status || "—") + "</span>";
  return "<div class='stepper' style='flex-wrap:wrap'>" + MFG_STEPS.map((s, i) =>
    "<span class='s " + (i < idx ? "done" : i === idx ? "active" : "queued") + "'>" +
    MFG_STEP_LABEL[s] + "</span>").join("<span class='dim'>›</span>") + "</div>";
}
async function viewMfg(v){
  if (!PROJECTS.length) await loadCommon();
  const [dash, ncd, jbd, prd, ind, mcfg, md] = await Promise.all([
    api("GET", "/manufacturing/dashboard").catch(() => ({ items: [], buckets: {} })),
    api("GET", "/nc-programs").catch(() => ({ nc_programs: [] })),
    api("GET", "/production-jobs").catch(() => ({ production_jobs: [] })),
    api("GET", "/print-jobs").catch(() => ({ print_jobs: [] })),
    api("GET", "/inspections").catch(() => ({ inspections: [] })),
    api("GET", "/items?limit=1000&include_archived=true").catch(() => ({ items: [] })),
    api("GET", "/machines").catch(() => ({ machines: [] }))
  ]);
  const itemMap = {};
  for (const it of mcfg.items || []) itemMap[it.id] = it;
  const machMap = {};
  for (const m of md.machines || []) machMap[m.id] = m.name;
  const nameOf = id => {
    const it = itemMap[id];
    if (it) return "<a href='#/items/" + id + "'>" + esc(itemName(it)) + "</a>";
    return "item " + String(id || "").slice(0, 8);
  };
  const ncs = ncd.nc_programs || [], jobs = jbd.production_jobs || [],
        prints = prd.print_jobs || [], inSp = ind.inspections || [];
  v.innerHTML =
    "<div class='head'><h2>Manufacturing</h2>" +
    "<span class='dim muted'>" + (dash.items || []).length + " item(s) in the mfg pipeline · " +
    ncs.length + " NC program(s) · " + jobs.length + " production job(s)</span></div>" +
    "<div class='card'><h2 class='sec'>Item pipeline</h2>" +
    ((dash.items || []).length ?
      "<table><thead><tr><th>Item</th><th>Pipeline</th><th>NC</th><th>Jobs</th></tr></thead><tbody>" +
      dash.items.map(r => "<tr>" +
        "<td>" + nameOf(r.item_id) + "</td>" +
        "<td>" + mfgStepper(r.status) + "</td>" +
        "<td class='muted'>" + r.nc_programs + "</td>" +
        "<td class='muted'>" + r.production_jobs + "</td>" +
        "</tr>").join("") + "</tbody></table>" :
      emptyState("ic-manufacturing", "No items in the mfg pipeline",
        "Give items the MFG classification and build their setups, CAM projects and NC programs.")) +
    "</div>" +
    "<div class='card' style='margin-top:12px'><h2 class='sec'>NC programs</h2>" +
    (ncs.length ?
      "<table><thead><tr><th>Item</th><th>Machine</th><th>Released</th><th>Source</th><th>Created</th><th></th></tr></thead><tbody>" +
      ncs.map(p => "<tr>" +
        "<td>" + nameOf(p.item_id) + "</td>" +
        "<td class='muted'>" + esc(machMap[p.machine_id] || "—") + "</td>" +
        "<td>" + (p.released ? "<span class='state released'>released</span>" :
          "<span class='dim'>draft</span>") +
          (p.outdated ? " <span class='state wip' title='the source design changed after this program was created'>outdated</span>" : "") +
        "</td>" +
        "<td class='mono muted'>" + (p.gcode_sha256 ? shaShort(p.gcode_sha256) : "—") + "</td>" +
        "<td class='muted'>" + fmtTime(p.created_at) + "</td>" +
        "<td>" + (!p.released ? "<button class='small' data-ncr='" + p.id + "'>Release</button>" : "") + "</td>" +
        "</tr>").join("") + "</tbody></table>" :
      "<div class='empty'>No NC programs yet.</div>") +
    "</div>" +
    "<div class='card' style='margin-top:12px'><h2 class='sec'>Production jobs</h2>" +
    (jobs.length ?
      "<table><thead><tr><th>Item</th><th>Machine</th><th>Operator</th><th>Status</th><th>Finished</th><th></th></tr></thead><tbody>" +
      jobs.map(j => "<tr>" +
        "<td>" + (j.item_id ? nameOf(j.item_id) : "—") + "</td>" +
        "<td class='muted'>" + esc(machMap[j.machine_id] || "—") + "</td>" +
        "<td class='muted'>" + esc(j.operator || "—") + "</td>" +
        "<td><span class='jobstat " + j.status + "'>" + esc(j.status) + "</span></td>" +
        "<td class='muted'>" + fmtTime(j.completed_at || j.created_at) + "</td>" +
        "<td style='white-space:nowrap'>" +
          (j.status === "pending" ? "<button class='small' data-ps='" + j.id + "' data-to='in_progress'>Start</button> " : "") +
          (j.status === "in_progress" ?
            "<button class='small' data-ps='" + j.id + "' data-to='done'>Done</button> " +
            "<button class='small' data-ps='" + j.id + "' data-to='rework'>Rework</button>" : "") +
        "</td>" +
        "</tr>").join("") + "</tbody></table>" :
      "<div class='empty'>No production jobs yet.</div>") +
    "</div>" +
    "<div class='grid2' style='margin-top:12px'>" +
      "<div class='card'><h2 class='sec'>Print jobs</h2>" +
      (prints.length ?
        "<table><thead><tr><th>Item</th><th>Material</th><th>Status</th><th></th></tr></thead><tbody>" +
        prints.map(p => "<tr>" +
          "<td>" + nameOf(p.item_id) + "</td>" +
          "<td class='muted'>" + esc(p.material || "—") + "</td>" +
          "<td><span class='jobstat " + p.status + "'>" + esc(p.status) + "</span></td>" +
          "<td style='white-space:nowrap'>" +
            (p.status === "queued" ? "<button class='small' data-pjs='" + p.id + "' data-to='printing'>Start</button> " : "") +
            ((p.status === "queued" || p.status === "printing") ?
              "<button class='small' data-pjs='" + p.id + "' data-to='done'>Done</button>" : "") +
          "</td>" +
          "</tr>").join("") + "</tbody></table>" :
        "<div class='empty'>No print jobs yet.</div>") +
      "</div>" +
      "<div class='card'><h2 class='sec'>Inspections</h2>" +
      (inSp.length ?
        "<table><thead><tr><th>Result</th><th>Job</th><th>Notes</th><th>When</th></tr></thead><tbody>" +
        inSp.map(i => "<tr>" +
          "<td>" + (i.passed == null ? "<span class='dim'>pending</span>" :
            i.passed ? "<span class='state released'>passed</span>" :
            "<span class='state wip'>failed</span>") + "</td>" +
          "<td class='mono muted'>" + String(i.production_job_id || "").slice(0, 8) + "</td>" +
          "<td class='dim muted'>" + esc(i.notes || "") + "</td>" +
          "<td class='muted'>" + fmtTime(i.created_at) + "</td>" +
          "</tr>").join("") + "</tbody></table>" :
        "<div class='empty'>No inspection records yet.</div>") +
      "</div>" +
    "</div>";
  v.querySelectorAll("[data-ncr]").forEach(b => b.addEventListener("click", async () => {
    try {
      await api("POST", "/nc-programs/" + b.dataset.ncr + "/release");
      toast("NC program released", "ok");
      render(false);
    } catch(e){ toast("Release: " + e.message, "err"); }
  }));
  v.querySelectorAll("[data-ps]").forEach(b => b.addEventListener("click", async () => {
    try {
      await api("POST", "/production-jobs/" + b.dataset.ps + "/status",
        { status: b.dataset.to });
      toast("Job " + b.dataset.to, "ok");
      render(false);
    } catch(e){ toast("Job status: " + e.message, "err"); }
  }));
  v.querySelectorAll("[data-pjs]").forEach(b => b.addEventListener("click", async () => {
    try {
      await api("POST", "/print-jobs/" + b.dataset.pjs + "/status",
        { status: b.dataset.to });
      toast("Print job " + b.dataset.to, "ok");
      render(false);
    } catch(e){ toast("Print job: " + e.message, "err"); }
  }));
}

// ── sheet-metal standards library (MFG reference data) ──────────────────
function smNum(x){ const n = Number(x); return (x === "" || isNaN(n)) ? "—" : n; }
function smUnit(){ return localStorage.getItem("pdm_sm_unit") === "saes" ? "saes" : "metric"; }
function smDisp(){ return localStorage.getItem("pdm_display_unit") === "in" ? "in" : "mm"; }
function smLen(x, disp){
  if (disp === "in"){
    const i = Number(x) / 25.4;
    return (x === "" || x === null || x === undefined || isNaN(i)) ? "—" : i.toFixed(3);
  }
  return smNum(x);
}
function smThickLabel(t, disp){
  if (disp === "in"){
    const inch = t.thickness_in !== undefined ? Number(t.thickness_in) : Number(t.thickness_mm) / 25.4;
    return inch.toFixed(3) + "&Prime;" +
      (t.thickness_gauge ? " <b>" + esc(t.thickness_gauge) + "</b>" : "") +
      " <span class='muted dim'>· " + smNum(t.thickness_mm) + " mm</span>";
  }
  return smNum(t.thickness_mm) + " mm" +
    (t.thickness_gauge ? " <span class='muted dim'>· " + esc(t.thickness_gauge) + "</span>" : "");
}
function smRow(t, disp){
  return "<tr><td>" + smThickLabel(t, disp) + "</td>" +
    "<td>" + smLen(t.min_bend_radius_mm, disp) + "</td>" +
    "<td>" + smNum(t.k_factor) + "</td>" +
    "<td>" + smLen(t.bend_deduction_mm, disp) + "</td>" +
    "<td>" + smLen(t.bend_allowance_mm, disp) + "</td>" +
    "<td class='muted dim'>" + esc(t.notes || "") + "</td></tr>";
}
function smTable(m, disp){
  const u = disp === "in" ? "in" : "mm";
  const head = "<thead><tr><th>" + (disp === "in" ? "Thickness (in · US gauge)" : "Thickness") + "</th><th>Min bend radius (" + u + ")</th>" +
    "<th>K-factor</th><th>Bend deduction 90° (" + u + ")</th><th>Bend allowance 90° (" + u + ")</th><th>Notes</th></tr></thead>";
  const rows = (m.thicknesses || []).map(t => smRow(t, disp)).join("");
  return "<table>" + head + "<tbody>" + rows + "</tbody></table>";
}
function smGroupLabel(g, m){
  const den = m.density_g_cm3 ? " <span class='muted dim'>· " + smNum(m.density_g_cm3) + " g/cm³</span>" : "";
  return "<h2 class='sec'>" + esc(m.material) + den +
    (g ? " <span class='muted dim'>(group: " + esc(g) + ")</span>" : "") + "</h2>";
}
async function viewSheetMetal(v){
  const saes = smUnit() === "saes";
  const disp = smDisp();
  const d = await api("GET", "/sheet-materials?unit=" + (saes ? "saes" : "metric"));
  const mats = d.materials || [];
  const groups = [];
  for (const m of mats) if (!groups.includes(m.group)) groups.push(m.group);
  v.innerHTML =
    "<div class='head'><h2>Sheet Metal Standards</h2>" +
    "<span class='dim muted'>" + mats.length + " material(s) · reusable bend-property reference, " +
    "seeded from the common-material catalog — per-item sheet-metal specs reference these</span>" +
    "<div class='grow'></div>" +
    "<span class='dim'>Filter:</span> " +
    "<div class='seg' id='sm-unit'>" +
    "<button data-unit='metric' class='" + (saes ? "" : "on") + "'>Metric</button>" +
    "<button data-unit='saes' class='" + (saes ? "on" : "") + "'>SAE / Imperial</button>" +
    "</div>" +
    "<span class='dim'>Display:</span> " +
    "<div class='seg' id='sm-disp'>" +
    "<button data-disp='mm' class='" + (disp === "mm" ? "on" : "") + "'>mm</button>" +
    "<button data-disp='in' class='" + (disp === "in" ? "on" : "") + "'>inch</button>" +
    "</div>" +
    "<button id='sm-add' class='primary'>＋ Add standard</button></div>" +
    groups.map(g =>
      "<h2 class='sec' style='margin-top:16px'>" + esc(g) + "</h2>" +
      mats.filter(m => m.group === g).map(m =>
        "<div style='margin:8px 0 14px'><div class='row'><b>" + esc(m.material) + "</b>" +
        (m.density_g_cm3 ? "<span class='badge'>ρ " + smNum(m.density_g_cm3) + " g/cm³</span>" : "") +
        "<span class='dim muted'>" + (m.thicknesses || []).length + " thickness(es)</span></div>" +
        smTable(m, disp) + "</div>").join("")
    ).join("");
  $("sm-unit").onclick = async e => {
    const b = e.target.closest("[data-unit]"); if (!b) return;
    localStorage.setItem("pdm_sm_unit", b.dataset.unit);
    for (const x of $("sm-unit").children) x.classList.toggle("on", x === b);
    await render(false);
  };
  $("sm-disp").onclick = async e => {
    const b = e.target.closest("[data-disp]"); if (!b) return;
    localStorage.setItem("pdm_display_unit", b.dataset.disp);
    for (const x of $("sm-disp").children) x.classList.toggle("on", x === b);
    await render(false);
  };
  $("sm-add").onclick = async () => {
    const f = await openModal("Add sheet-metal standard", [
      { id:"material_group", label:"Material group", required:true, placeholder:"e.g. Aluminum / Steel / Stainless" },
      { id:"material", label:"Material", required:true, placeholder:"e.g. Aluminum 5052-H32" },
      { id:"thickness_mm", label:"Thickness (mm)", required:true, placeholder:"e.g. 1.5" },
      { id:"thickness_gauge", label:"US gauge (optional)", placeholder:"e.g. 16 ga" },
      { id:"density_g_cm3", label:"Density g/cm³ (optional)", placeholder:"e.g. 2.66" },
      { id:"min_bend_radius_mm", label:"Min bend radius mm (optional)", placeholder:"e.g. 0.75" },
      { id:"k_factor", label:"K-factor (optional)", placeholder:"e.g. 0.33" },
      { id:"bend_deduction_mm", label:"Bend deduction 90° mm (optional)", placeholder:"e.g. 2.1" },
      { id:"bend_allowance_mm", label:"Bend allowance 90° mm (optional)", placeholder:"e.g. 3.1" },
      { id:"notes", label:"Notes (optional)", type:"textarea", placeholder:"e.g. typical 90° air bend, V-die opening ~8t" }
    ], "Add standard");
    if (!f) return;
    const num = x => x.trim() === "" ? null : Number(x);
    const body = {
      material_group: f.material_group.trim(),
      material: f.material.trim(),
      thickness_mm: num(f.thickness_mm),
      thickness_gauge: f.thickness_gauge.trim() || null,
      density_g_cm3: num(f.density_g_cm3),
      min_bend_radius_mm: num(f.min_bend_radius_mm),
      k_factor: num(f.k_factor),
      bend_deduction_mm: num(f.bend_deduction_mm),
      bend_allowance_mm: num(f.bend_allowance_mm),
      notes: f.notes.trim() || null
    };
    if (body.thickness_mm === null || isNaN(body.thickness_mm)) return toast("Thickness (mm) must be a number", "err");
    try {
      await api("POST", "/sheet-materials", body);
      toast("Standard added: " + body.material + " " + body.thickness_mm + " mm", "ok");
      render(false);
    } catch(e){ toast("Add failed: " + e.message, "err"); }
  };
}
// ── approval center (P0-2): inbox, decisions, routing ───────────────────
function apprFullTable(list){
  const canDecide = a => a.status === "pending" && isRole("editor");
  return "<table><thead><tr><th>Item</th><th>Requester</th><th>Assignees</th>" +
    "<th>Status</th><th>Since</th><th></th></tr></thead><tbody>" +
    list.map(a => {
      const requester = a.requested_by_name ||
        (UMAP[a.requested_by] ? (UMAP[a.requested_by].display_name || UMAP[a.requested_by].email) : "—");
      const assignees = (a.assignees || [])
        .map(uid => UMAP[uid] ? (UMAP[uid].display_name || UMAP[uid].email) : "user")
        .join(", ") || "—";
      const statusCls = a.status === "approved" ? "released" :
        (a.status === "rejected" ? "wip" : "draft");
      const mine = myId() && (a.assignees || []).includes(myId());
      return "<tr class='clickable' data-aid='" + a.id + "'>" +
        "<td>" + (a.item_part_number ?
          "<span class='mono'>" + esc(a.item_part_number) + "</span>" :
          "<span class='dim'>no item</span>") + "</td>" +
        "<td class='muted'>" + esc(requester) + "</td>" +
        "<td class='muted'>" + esc(assignees) +
          (mine && a.status === "pending" ? " <span class='dim' style='font-size:11px'>(you)</span>" : "") + "</td>" +
        "<td><span class='state " + statusCls + "'>" + esc(a.status).toUpperCase() + "</span></td>" +
        "<td class='muted'>" + fmtTime(a.created_at) + "</td>" +
        "<td style='white-space:nowrap'>" +
          (canDecide(a) ?
            "<button class='small' data-ap='" + a.id + "'>Approve</button> " +
            "<button class='small' data-rj='" + a.id + "'>Reject</button>" : "") +
        "</td></tr>";
    }).join("") + "</tbody></table>";
}
async function viewApprovals(v){
  if (!ROUTE._atab) ROUTE._atab = "mine";
  const tabs = [["mine","Needs my approval"],["requested","I requested"],["all","All"]];
  v.innerHTML =
    "<div class='head'><h2>Approvals</h2>" +
    "<span class='dim muted'>release requests routed to editors and admins</span></div>" +
    "<div class='tabs'>" + tabs.map(t =>
      "<div class='tab" + (ROUTE._atab === t[0] ? " on" : "") + "' data-at='" + t[0] + "'>" +
      t[1] + "</div>").join("") + "</div>" +
    "<div id='awrap'><div class='skel'><span class='spinner'></span> loading…</div></div>";
  v.querySelectorAll(".tab[data-at]").forEach(t => t.addEventListener("click", () => {
    ROUTE._atab = t.dataset.at;
    v.querySelectorAll(".tab").forEach(x => x.classList.toggle("on", x === t));
    pushHash();
    loadApprovals();
  }));
  await loadApprovals();
  async function loadApprovals(){
    const box = $("awrap");
    if (!box) return;
    const params = new URLSearchParams({ limit: "100" });
    if (ROUTE._atab === "mine"){ params.set("status", "pending"); params.set("mine", "true"); }
    else if (ROUTE._atab === "requested") params.set("status", "pending");
    let d;
    try { d = await api("GET", "/approvals?" + params.toString()); }
    catch(e){
      box.innerHTML = emptyState("ic-approval", "Could not load approvals", e.message,
        "<div><button class='cta' id='aretry'>Retry</button></div>");
      const r = $("aretry"); if (r) r.onclick = () => loadApprovals();
      return;
    }
    let list = d.approvals || [];
    if (ROUTE._atab === "requested") list = list.filter(a => a.requested_by === myId());
    box.innerHTML = list.length ? apprFullTable(list) :
      emptyState("ic-approval",
        ROUTE._atab === "mine" ? "Nothing needs your approval" :
        ROUTE._atab === "requested" ? "You haven't requested any approvals" :
        "No approvals yet",
        "Releasing an item with the approval gate on routes it here for sign-off.",
        "<div><button class='cta' data-go='items'>Browse items</button></div>");
    const g = box.querySelector("[data-go]");
    if (g) g.onclick = () => go("items");
    box.querySelectorAll("[data-ap]").forEach(b =>
      b.addEventListener("click", e => { e.stopPropagation(); decideApproval(b.dataset.ap, "approve"); }));
    box.querySelectorAll("[data-rj]").forEach(b =>
      b.addEventListener("click", e => { e.stopPropagation(); decideApproval(b.dataset.rj, "reject"); }));
    box.querySelectorAll("tr.clickable[data-aid]").forEach(tr =>
      tr.addEventListener("click", () => {
        const a = list.find(x => x.id === tr.dataset.aid);
        if (a && a.item_id) openDetail(a.item_id);
      }));
  }
}

// ── org-wide activity feed ──────────────────────────────────────────────
async function viewActivity(v){
  if (!USERS.length || !PROJECTS.length) await loadCommon();
  const limit = Math.min(200, ROUTE._alimit || 100);
  const d = await api("GET", "/audit?limit=" + limit).catch(() => ({ events: [] }));
  const evs = d.events || [];
  v.innerHTML =
    "<div class='head'><h2>Activity</h2>" +
    "<span class='dim muted'>org-wide audit trail — newest first</span></div>" +
    feed(evs) +
    (evs.length && limit < 200 ?
      "<div class='row' style='justify-content:center;margin-top:8px'>" +
      "<button id='amore' class='small'>Load older events</button></div>" : "");
  const m = $("amore");
  if (m) m.onclick = () => { ROUTE._alimit = 200; render(false); };
}
async function viewSearch(v){
  if (!PROJECTS.length) await loadCommon();
  const q = (ROUTE.q || $("globsearch").value || "");
  v.innerHTML =
    "<div class='head'><h2>Search</h2></div>" +
    "<div class='row'><input id='sq' style='flex:1;max-width:520px' " +
    "placeholder='part number, description or filename…' value='" + esc(q) + "'>" +
    "<button id='sbtn' class='primary'>Search</button></div>" +
    "<div id='sres'>" + (q ? "<div class='skel'><span class='spinner'></span> searching…</div>" :
      "<div class='empty'>Type a query — searches across all projects.</div>") + "</div>";
  const run = async () => {
    const t = $("sq").value.trim();
    if (!t) return;
    try {
      const d = await api("GET", "/search?q=" + encodeURIComponent(t));
      const items = (d.results || []).map(it => Object.assign({}, it,
        { _project: PMAP[it.project_id] ? PMAP[it.project_id].name : "?", _project_id: it.project_id }));
      $("sres").innerHTML = items.length ? itemsTable(items, true) :
        emptyState("ic-search", "No results", "Nothing matches “" + t + "” across part numbers, descriptions and filenames.");
      await attachThumbs($("sres"));
      wireItemTable($("sres"), {});
    } catch(e){ $("sres").innerHTML = "<div class='errbox'>" + esc(e.message) + "</div>"; }
  };
  $("sbtn").onclick = run;
  $("sq").addEventListener("keydown", e => { if (e.key === "Enter") run(); });
  if (q){ $("sq").focus(); run(); }
}

// ── themes ──────────────────────────────────────────────────────────────
const THEMES = [
  { id:"dark", name:"Dark", desc:"Midnight (default)", swatches:["#0f141b","#182130","#5aa9ff","#d8e1ec"] },
  { id:"light", name:"Light", desc:"Bright workspace", swatches:["#f3f5f8","#ffffff","#1f6fd6","#1d2733"] },
  { id:"slate", name:"Slate", desc:"Cool grey, cyan accent", swatches:["#1a1d23","#22262e","#88c0d0","#eceff4"] },
  { id:"contrast", name:"High contrast", desc:"Maximum readability", swatches:["#000","#0a0a0a","#ffff00","#fff"] }
];
function currentTheme(){
  return localStorage.getItem("pdm_theme") || "dark";
}
function applyTheme(id){
  if (!THEMES.some(t => t.id === id)) id = "dark";
  document.documentElement.setAttribute("data-theme", id);
  localStorage.setItem("pdm_theme", id);
}
function themePickerHTML(){
  const cur = currentTheme();
  return "<div class='theme-grid'>" + THEMES.map(t =>
    "<button type='button' class='theme-card" + (t.id === cur ? " on" : "") + "' data-theme-id='" + t.id + "'>" +
    "<div class='theme-swatches'>" + t.swatches.map(c => "<span style='background:" + c + "'></span>").join("") + "</div>" +
    "<strong>" + esc(t.name) + "</strong><span class='dim'>" + esc(t.desc) + "</span></button>"
  ).join("") + "</div>";
}
function wireThemePicker(root){
  (root || document).querySelectorAll("[data-theme-id]").forEach(b => {
    b.onclick = () => { applyTheme(b.dataset.themeId); toast("Theme: " + (THEMES.find(t => t.id === b.dataset.themeId) || {}).name, "ok"); if (ROUTE.name === "admin" || ROUTE.name === "settings") render(false); };
  });
}

const GPU_POLICIES = [
  { id:"auto", name:"Auto", desc:"GPU if present, else CPU" },
  { id:"hardware", name:"GPU", desc:"Require the machine GPU" },
  { id:"software", name:"GPU software", desc:"Driver on CPU (llvmpipe)" },
  { id:"cpu", name:"CPU", desc:"SolidCraft CPU rasterizer" }
];
function gpuPickerHTML(cur, canEdit){
  return "<div class='theme-grid'>" + GPU_POLICIES.map(t =>
    "<button type='button' class='theme-card" + (t.id === cur ? " on" : "") + "' data-gpu-id='" + t.id + "'" +
    (canEdit ? "" : " disabled") + ">" +
    "<strong>" + esc(t.name) + "</strong><span class='dim'>" + esc(t.desc) + "</span></button>"
  ).join("") + "</div>";
}
function wireGpuPicker(root, canEdit){
  if (!canEdit) return;
  (root || document).querySelectorAll("[data-gpu-id]").forEach(b => {
    b.onclick = async () => {
      try {
        await api("PUT", "/settings/gpu-policy", { gpu_policy: b.dataset.gpuId });
        toast("Preview renderer: " + ((GPU_POLICIES.find(t => t.id === b.dataset.gpuId) || {}).name || b.dataset.gpuId), "ok");
        render(false);
      } catch(e){ toast("GPU policy: " + e.message, "err"); }
    };
  });
}

// ── administration ──────────────────────────────────────────────────────
function usersTable(list){
  if (!list.length) return "<div class='empty'>No users.</div>";
  return "<table><thead><tr><th>User</th><th>Email</th><th>Role</th><th>Status</th><th>Since</th><th></th></tr></thead><tbody>" +
    list.map(u => "<tr>" +
      "<td><span style='display:inline-flex;gap:8px;align-items:center'>" + userAv(u, !u.active) +
        "<span>" + esc(u.display_name || "—") + "</span></span></td>" +
      "<td class='mono'>" + esc(u.email) + "</td>" +
      "<td>" + esc(u.role) + "</td>" +
      "<td class='muted'>" + (u.active ? "Active" : "Disabled") + "</td>" +
       "<td class='muted'>" + String(u.created_at || "").slice(0, 10) + "</td>" +
        "<td style='white-space:nowrap'><button class='small' data-edit='" + u.id + "'>edit</button> " +
        "<button class='small' data-pw='" + u.id + "'>password</button> " +
        "<button class='small' data-tok='" + u.id + "'>＋ token</button> " +
        "<button class='small' data-active='" + u.id + "'>" + (u.active ? "Disable" : "Enable") + "</button></td>" +
       "</tr>").join("") + "</tbody></table>";
}
function auditTable(list){
  if (!list.length) return "<div class='empty'>No audit events.</div>";
  return "<table><thead><tr><th>When</th><th>Actor</th><th>Action</th><th>Object</th><th>Detail</th></tr></thead><tbody>" +
    list.map(ev => {
      const detail = ev.detail ? esc(JSON.stringify(ev.detail)).slice(0, 90) : "";
      return "<tr><td class='muted'>" + fmtDate(ev.at) + "</td>" +
        "<td class='muted'>" + esc(actorName(ev)) + "</td>" +
        "<td class='mono'>" + esc(ev.action) + "</td>" +
        "<td class='dim'>" + esc(ev.object_type || "") + "</td>" +
        "<td class='dim' style='max-width:340px;overflow:hidden;text-overflow:ellipsis'>" + detail + "</td></tr>";
    }).join("") + "</tbody></table>";
}
function genPassword(){
  const c = "abcdefghjkmnpqrstuvwxyzABCDEFGHJKMNPQRSTUVWXYZ23456789";
  let s = "";
  for (let i = 0; i < 12; i++) s += c[Math.floor(Math.random() * c.length)];
  return s;
}
async function addUser(){
  const f = await openModal("Add user", [
    { id:"email", label:"Email", required:true, placeholder:"name@company.com", type:"text" },
    { id:"display_name", label:"Display name", placeholder:"e.g. Jane" },
    { id:"role", label:"Role", type:"select",
      options:[{value:"editor",label:"Editor"},{value:"viewer",label:"Viewer"},{value:"admin",label:"Admin"}], value:"editor" },
    { id:"password", label:"Password (min 4 characters)", required:true, type:"password" }
  ], "Add user");
  if (!f) return;
  try {
    await api("POST", "/users", {
      email: f.email, display_name: f.display_name || null, role: f.role, password: f.password
    });
    toast("User “" + f.email + "” added", "ok");
    render(false);
  } catch(e){ toast("Add user: " + e.message, "err"); }
}
async function inviteUser(){
  const f = await openModal("Invite user", [
    { id:"email", label:"Email", required:true, placeholder:"name@company.com", type:"text" },
    { id:"display_name", label:"Display name", placeholder:"e.g. Jane" },
    { id:"role", label:"Role", type:"select",
      options:[{value:"editor",label:"Editor"},{value:"viewer",label:"Viewer"},{value:"admin",label:"Admin"}], value:"editor" }
  ], "Send invite");
  if (!f) return;
  try {
    await api("POST", "/users/invite", {
      email: f.email, display_name: f.display_name || null, role: f.role
    });
    toast("Invite emailed to " + f.email, "ok");
    render(false);
  } catch(e){ toast("Invite: " + e.message, "err"); }
}
async function editUser(u){
  const f = await openModal("Edit user — " + u.email, [
    { id:"display_name", label:"Display name", value:u.display_name || "" },
    { id:"role", label:"Role", type:"select",
      options:[{value:"editor",label:"Editor"},{value:"viewer",label:"Viewer"},{value:"admin",label:"Admin"}], value:u.role },
    { id:"active", label:"Account status", type:"select",
      options:[{value:"true",label:"Active"},{value:"false",label:"Disabled"}], value:String(u.active) }
  ], "Save");
  if (!f) return;
  try {
    const d = await api("PUT", "/users/" + u.id, {
      display_name: f.display_name, role: f.role, active: f.active === "true"
    });
    if (u.id === myId()){
      ME.user = d.user;
      if (!d.user.active){ await signout(); return; }
    }
    toast("User “" + u.email + "” updated", "ok");
    render(false);
  } catch(e){ toast("Edit user: " + e.message, "err"); }
}
async function toggleUserActive(u){
  const active = !u.active;
  if (!active && !confirm("Disable account for " + u.email + "? Sign-in and existing API tokens will be blocked until the account is enabled again." +
    (u.id === myId() ? " This is your account; you will be signed out." : ""))) return;
  try {
    await api("PUT", "/users/" + u.id, { active });
    toast("Account “" + u.email + "” " + (active ? "enabled" : "disabled"), "ok");
    if (!active && u.id === myId()) { await signout(); return; }
    render(false);
  } catch(e){ toast("Account status: " + e.message, "err"); }
}
async function giveToken(u){
  const f = await openModal("Create API token — " + u.email, [
    { id:"name", label:"Token name", value:"web" }
  ], "Create");
  if (!f) return;
  try {
    const d = await api("POST", "/users/" + u.id + "/tokens", { name: f.name || "web" });
    openModal("API token for " + u.email, [
      { id:"tok", label:"Share this token — shown only once", type:"textarea", value: d.token }
    ], "Done");
  } catch(e){ toast("Token failed: " + e.message, "err"); }
}

// ── account: user menu + password change / reset ────────────────────────
function showUserMenu(ev){
  ev.stopPropagation();
  if (!ME.user) return;
  const entries = [
    { label: "Change password", act: () => changePassword() },
    "sep",
    { label: "Sign out", danger: true, act: () => signout() },
  ];
  const m = $("kebabmenu");
  m.innerHTML = entries.map((en, i) => en === "sep" ? "<div class='sep'></div>" :
    "<div class='ki" + (en.danger ? " danger" : "") + "' data-i='" + i + "'><span>" +
    esc(en.label) + "</span></div>").join("");
  m.classList.remove("hidden");
  const r = ev.currentTarget.getBoundingClientRect();
  let left = Math.max(6, Math.min(r.right - m.offsetWidth, innerWidth - m.offsetWidth - 6));
  let top = r.bottom + 4;
  if (top + m.offsetHeight > innerHeight - 6) top = Math.max(6, r.top - m.offsetHeight - 4);
  m.style.left = left + "px";
  m.style.top = top + "px";
  m.onclick = e => {
    const ki = e.target.closest(".ki"); if (!ki) return;
    const en = entries[+ki.dataset.i];
    hideKebab(); if (en && en.act) en.act();
  };
}

async function changePassword(){
  if (!ME.user) return;
  const f = await openModal("Change password — " + myName(), [
    { id:"current", label:"Current password", required:true, type:"password" },
    { id:"new", label:"New password (min 4 characters)", required:true, type:"password" },
    { id:"confirm", label:"Confirm new password", required:true, type:"password" }
  ], "Change password");
  if (!f) return;
  if (f.new !== f.confirm){ toast("New passwords do not match", "err"); return; }
  // Raw fetch: api() treats *any* 401 as "session expired" and reloads the page,
  // which would discard this form when only the current password is wrong.
  try {
    const r = await fetch("/me/password", {
      method: "POST",
      headers: { "Authorization": "Bearer " + TOKEN, "Content-Type": "application/json" },
      body: JSON.stringify({ current: f.current, new: f.new })
    });
    if (!r.ok){
      let msg = "HTTP " + r.status;
      const d = await r.json().catch(() => null);
      if (d && d.error) msg = typeof d.error === "string" ? d.error : (d.error.message || msg);
      toast("Change password: " + msg, "err");
      return;
    }
    toast("Password changed", "ok");
  } catch(e){ toast("Change password: " + e.message, "err"); }
}

// Admin: set a brand-new password for any user (no current password needed).
async function resetPassword(u){
  const pw = genPassword();
  const f = await openModal("Reset password — " + (u.display_name || u.email), [
    { id:"new", label:"New password (min 4 characters)", required:true, type:"password", value: pw },
    { id:"confirm", label:"Confirm new password", required:true, type:"password", value: pw }
  ], "Reset password");
  if (!f) return;
  if (f.new !== f.confirm){ toast("Passwords do not match", "err"); return; }
  try {
    await api("POST", "/users/" + u.id + "/password", { password: f.new });
    toast("Password reset for " + (u.display_name || u.email), "ok");
  } catch(e){ toast("Reset password: " + e.message, "err"); }
}
// ── system (server + bundled SolidCraft CLI + infrastructure) ────────────
function fmtUptime(sec){
  if (sec == null) return "—";
  sec = Math.floor(sec);
  const d = Math.floor(sec / 86400); sec -= d * 86400;
  const h = Math.floor(sec / 3600); sec -= h * 3600;
  const m = Math.floor(sec / 60); sec -= m * 60;
  const p = [];
  if (d) p.push(d + "d"); if (h) p.push(h + "h"); if (m) p.push(m + "m");
  p.push(sec + "s");
  return p.join(" ");
}
function viewSystem(v){
  v.innerHTML =
    "<div class='head'><h2>System</h2>" +
    "<span class='dim muted'>PDM server · bundled SolidCraft CLI · infrastructure</span>" +
    "<div class='grow'></div><button id='sys-refresh' class='primary'>⟳ Refresh</button></div>" +
    "<div class='skel'><span class='spinner'></span> loading…</div>";
  const load = async () => {
    try {
      const [d, cad, scweb, dm] = await Promise.all([
        api("GET", "/system-info"),
        api("GET", "/settings/cad-cli").catch(() => ({})),
        api("GET", "/settings/solidcraft-web").catch(() => ({})),
        api("GET", "/settings/dutchman").catch(() => ({}))
      ]);
      const row = (k, val) =>
        "<tr><td class='dim'>" + k + "</td><td class='mono'>" +
        (val == null ? "—" : esc(String(val))) + "</td></tr>";
      const pdm = d.pdm || {}, cli = d.solidcraft_cli || {}, pg = d.postgres || {},
            st = d.storage || {}, c = d.counts || {}, gpu = d.gpu || {};
      const survey = gpu.survey || {};
      const chosen = survey.chosen || {};
      const lands = survey.lands_on || survey.renderer || chosen.name || null;
      const cliLine = cli.present
        ? (cli.version ? esc(String(cli.version)) : "bundled")
        : "<span class='dim'>not bundled — worker CAD jobs are skipped</span>";
      v.innerHTML =
        "<div class='head'><h2>System</h2>" +
        "<span class='dim muted'>PDM server · bundled SolidCraft CLI · infrastructure</span>" +
        "<div class='grow'></div><button id='sys-refresh' class='primary'>⟳ Refresh</button></div>" +
        "<div class='grid2'>" +
        "<div class='card'><h2 class='sec'>PDM server</h2><table>" +
          row("Build", pdm.build) +
          "<tr><td class='dim'>Update</td><td class='mono'>" +
            (pdm.update_available
              ? "<a href='" + esc(pdm.releases_url || "#") + "' target='_blank' rel='noopener'>available " +
                esc(String(pdm.latest != null ? pdm.latest : "?")) + "</a>"
              : (pdm.latest != null ? "up to date" : "—")) +
          "</td></tr>" +
          row("Package version", pdm.version) +
          row("Git commit", pdm.commit) + row("Built at", pdm.built_at) +
          row("Uptime", fmtUptime(d.uptime_seconds)) + "</table></div>" +
        "<div class='card'><h2 class='sec'>SolidCraft CAD CLI</h2><table>" +
          "<tr><td class='dim'>Version</td><td class='mono'>" + cliLine + "</td></tr>" +
          row("Binary path", cli.path) +
          row("GitHub latest", cad.latest || "—") +
          "</table>" +
          (cli.error ? "<p class='dim' style='color:#d98'>" + esc(cli.error) + "</p>" : "") +
          "<p class='dim'><a href='https://github.com/tangency-labs/solidcraft/releases/latest' target='_blank' rel='noreferrer'>tangency-labs/solidcraft</a></p>" +
          "<div class='row'><button id='cli-upd' class='primary'>" +
            (cad.update_available ? "Update CLI" : "Check / reinstall CLI") + "</button></div>" +
          "</div>" +
        "<div class='card'><h2 class='sec'>Dutchman (feature-tree converter)</h2><table>" +
          "<tr><td class='dim'>Installed</td><td class='mono'>" +
            (dm.installed
              ? esc(String(dm.installed)) + (dm.info && dm.info.tag ? " · " + esc(String(dm.info.channel)) : "")
              : "<span class='dim'>" + (dm.path ? "not installed — pulled on demand" : "—") + "</span>") +
          "</td></tr>" +
          row("Binary path", dm.path) +
          "</table>" +
          (dm.error ? "<p class='dim' style='color:#d98'>" + esc(dm.error) + "</p>" : "") +
          "<p class='dim'>Decodes native CAD (Fusion, Inventor, SolidWorks, FreeCAD, CATIA, Rhino, " +
            "ACIS, STEP, Onshape bundles) into SolidCraft designs with editable feature history. " +
            (dm.auto_update
              ? "Tracks the selected release channel automatically (boot + hourly; PDM_DUTCHMAN_UPDATE=0 opts out)."
              : "Auto-update is off (PDM_DUTCHMAN_UPDATE=0).") +
          " <a href='https://github.com/dutchman-cad/dutchman/releases' target='_blank' rel='noreferrer'>dutchman-cad/dutchman releases</a></p>" +
          "<div class='row'><label for='dm-channel'>Release channel</label>" +
          "<select id='dm-channel'>" +
            ((dm.channels || []).map(c =>
              "<option value='" + esc(c.value) + "'" +
              (dm.settings && dm.settings.channel === c.value ? " selected" : "") + ">" + esc(c.label) + "</option>")
            .join("")) +
          "</select></div>" +
          "<div class='row'><button id='dm-upd' class='primary'>" +
            (dm.update_available ? "Download the referenced release" : "Check / reinstall Dutchman") +
            "</button></div>" +
          "<h2 class='sec' style='margin-top:12px'>Conversion settings (this org)</h2>" +
          "<div class='row'><label for='dm-preset'>Preset</label>" +
          "<select id='dm-preset'>" + ["fast","standard","full"].map(p =>
            "<option value='" + p + "'" + ((dm.settings||{}).preset === p ? " selected" : "") + ">" + p + "</option>").join("") +
          "</select>" +
          "<label for='dm-parts'>Part positions</label>" +
          "<select id='dm-parts'>" + ["smart","last-shown","first-placed"].map(p =>
            "<option value='" + p + "'" + ((dm.settings||{}).part_positions === p ? " selected" : "") + ">" + p + "</option>").join("") +
          "</select></div>" +
          "<div class='row'><label for='dm-verify'><input type='checkbox' id='dm-verify'" +
            (((dm.settings||{}).verify !== false) ? " checked" : "") + "> Verify history in SolidCraft</label>" +
          "<label for='dm-timeout'>Timeout (s, 0 = 1800)</label>" +
          "<input id='dm-timeout' type='number' min='0' max='7200' step='60' style='width:90px' value='" +
            ((dm.settings||{}).timeout_secs || 0) + "'></div>" +
          "<div class='row'><label for='dm-onshape-calls'>Onshape fetch calls (0 = 200)</label>" +
          "<input id='dm-onshape-calls' type='number' min='0' max='10000' step='10' style='width:110px' value='" +
            ((dm.settings||{}).onshape_max_calls || 0) + "'></div>" +
          "<div class='row'><button id='dm-save'>Save conversion settings</button></div>" +
          "</div>" +
        "<div class='card'><h2 class='sec'>SolidCraft web editor</h2><table>" +
          "<tr><td class='dim'>Installed</td><td class='mono'>" +
            (scweb.installed ? esc(String(scweb.installed)) : "<span class='dim'>not bundled</span>") +
          "</td></tr>" +
          row("GitHub latest", scweb.latest) +
          "</table>" +
          (scweb.error ? "<p class='dim' style='color:#d98'>" + esc(scweb.error) + "</p>" : "") +
          "<p class='dim'>Serves “Edit in SolidCraft web” from the portal origin. " +
            (scweb.auto_update
              ? "Tracks the newest release automatically (every 6 h; PDM_WEB_UPDATE=0 opts out)."
              : "Auto-update is off (PDM_WEB_UPDATE=0).") +
          " Open editors pick up a new build on the next open.</p>" +
          "<div class='row'><button id='scweb-upd' class='primary'>" +
            (scweb.update_available ? "Update web editor" : "Check / reinstall web editor") + "</button></div>" +
          "</div>" +
        "<div class='card'><h2 class='sec'>Preview renderer</h2><table>" +
          row("Policy", gpu.policy) +
          row("Lands on", lands) +
          row("Adapter", chosen.name || chosen.adapter || chosen.info || null) +
          row("Backends", Array.isArray(survey.backends) ? survey.backends.join(", ") : survey.backends) +
          "</table>" +
          (gpu.error ? "<p class='dim' style='color:#d98'>" + esc(gpu.error) + "</p>" : "") + "</div>" +
        "<div class='card'><h2 class='sec'>Infrastructure</h2><table>" +
          row("PostgreSQL", pg.version ? pg.version + (pg.ok === false ? " (offline)" : "") : "—") +
          row("Object storage", st.kind || "—") +
          row("Storage endpoint", st.endpoint || "—") +
          row("Bucket", st.bucket || "—") + "</table></div>" +
        "<div class='card'><h2 class='sec'>Counts</h2><table>" +
          row("Organizations", c.organizations) + row("Users", c.users) +
          row("Projects (this org)", c.projects) + row("Items (this org)", c.items) +
          row("Files (this org)", c.files) + row("Revisions (this org)", c.revisions) + "</table></div>" +
        "</div>";
      $("sys-refresh").onclick = load;
      const up = $("cli-upd");
      if (up) up.onclick = async () => {
        up.disabled = true; up.textContent = "Downloading…";
        try {
          const r = await api("POST", "/settings/cad-cli/update", {});
          toast("CLI " + (r.installed || "updated"), "ok");
          load();
        } catch(e){ toast("CLI update: " + e.message, "err"); up.disabled = false; up.textContent = "Update CLI"; }
      };
      const wup = $("scweb-upd");
      if (wup) wup.onclick = async () => {
        wup.disabled = true; wup.textContent = "Downloading…";
        try {
          const r = await api("POST", "/settings/solidcraft-web/update", {});
          toast("SolidCraft web " + (r.installed || "updated"), "ok");
          load();
        } catch(e){ toast("Web editor update: " + e.message, "err"); wup.disabled = false; wup.textContent = "Update web editor"; }
      };
      const dup = $("dm-upd");
      if (dup) dup.onclick = async () => {
        dup.disabled = true; dup.textContent = "Downloading…";
        try {
          const channel = $("dm-channel") ? $("dm-channel").value : "latest";
          await api("PUT", "/settings/dutchman", { channel });
          const r = await api("POST", "/settings/dutchman/update", {});
          toast("Dutchman " + (r.installed || "updated") + " (" + channel + ")", "ok");
          load();
        } catch(e){ toast("Dutchman update: " + e.message, "err"); dup.disabled = false; dup.textContent = "Update Dutchman"; }
      };
      const dsave = $("dm-save");
      if (dsave) dsave.onclick = async () => {
        dsave.disabled = true;
        try {
          await api("PUT", "/settings/dutchman", {
            preset: $("dm-preset").value,
            part_positions: $("dm-parts").value,
            verify: $("dm-verify").checked,
            timeout_secs: Number($("dm-timeout").value || 0),
            onshape_max_calls: Number($("dm-onshape-calls").value || 0)
          });
          toast("Dutchman settings saved", "ok");
          load();
        } catch(e){ toast("Dutchman settings: " + e.message, "err"); dsave.disabled = false; }
      };
    } catch(e){
      v.innerHTML = "<div class='errbox'>" + esc(e.message) + "</div>" +
        "<div class='row'><button class='primary' id='sys-retry'>Retry</button></div>";
      const r = $("sys-retry"); if (r) r.onclick = () => viewSystem(v);
    }
  };
  $("sys-refresh").onclick = load;
  load();
}

// ── settings (org part-number prefixes) ─────────────────────────────────
async function viewSettings(v){
  const canEdit = isRole("admin");
  const [d, gp, mail, osh, proj, preview, nestDef] = await Promise.all([
    api("GET", "/settings/part-number-prefixes").catch(() => ({ prefixes: [] })),
    api("GET", "/settings/gpu-policy").catch(() => ({ gpu_policy: "auto" })),
    api("GET", "/settings/mail").catch(() => ({})),
    api("GET", "/settings/onshape").catch(() => ({})),
    api("GET", "/projects").catch(() => ({ projects: [] })),
    api("GET", "/settings/preview-style").catch(() => ({preview_style:"transparent"})),
    api("GET", "/settings/nest-defaults").catch(() => ({ defaults: null }))
  ]);
  const nd = nestDef.defaults || {};
  const prefixes = d.prefixes || [];
  const gpuPolicy = gp.gpu_policy || "auto";
  v.innerHTML =
    "<div class='head'><h2>Settings</h2>" +
    "<span class='dim muted'>organization settings · shared by the web workspace and the desktop app</span>" +
    "</div>" +
    "<h2 class='sec'>Appearance</h2>" +
    "<div class='card'><p class='dim'>Colour theme for this browser. Stored locally.</p>" +
    themePickerHTML() + "</div>" +
    "<h2 class='sec'>Preview renderer</h2>" +
    "<div class='card'><p class='dim'>Transparent product renders use the CPU product-image renderer, with no grid or axes. " +
    "Viewport snapshots retain the original background and use the GPU policy below. Regenerate an existing preview to apply a new style.</p>" +
    "<div class='row'><label for='preview-style'>Preview style</label><select id='preview-style'" + (canEdit ? "" : " disabled") + ">" +
    "<option value='transparent'" + (preview.preview_style !== "viewport" ? " selected" : "") + ">Transparent product render</option>" +
    "<option value='viewport'" + (preview.preview_style === "viewport" ? " selected" : "") + ">Viewport snapshot (background, grid and axes)</option></select>" +
    (canEdit ? "<button id='preview-style-save'>Save preview style</button>" : "") + "</div>" +
    gpuPickerHTML(gpuPolicy, canEdit) + "</div>" +
    "<h2 class='sec'>Mail server</h2>" +
    "<div class='card'><p class='dim'>SMTP used when you invite a user. Leave password blank to keep the current one.</p>" +
    "<div class='grid2'>" +
      "<div class='modal-field'><label>Host</label><input id='mail-host' value='" + esc(mail.host || "") + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Port</label><input id='mail-port' value='" + esc(String(mail.port || 587)) + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Username</label><input id='mail-user' value='" + esc(mail.username || "") + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Password</label><input id='mail-pass' type='password' placeholder='" + (mail.password_set ? "unchanged" : "") + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>From address</label><input id='mail-from' placeholder='Foundry PDM &lt;pdm@example.com&gt;' value='" + esc(mail.from || "") + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Encryption</label><select id='mail-tls'" + (canEdit?"":" disabled") + ">" +
        ["starttls","tls","none"].map(t => "<option value='" + t + "'" + (mail.tls === t ? " selected" : "") + ">" +
          (t === "starttls" ? "STARTTLS (587)" : t === "tls" ? "TLS (465)" : "None") + "</option>").join("") +
      "</select></div>" +
      "<div class='modal-field' style='grid-column:1/-1'><label>Sign-in URL (optional)</label><input id='mail-url' placeholder='https://pdm.example.com' value='" + esc(mail.login_url || "") + "'" + (canEdit?"":" disabled") + "></div>" +
    "</div>" +
    (canEdit ? "<div class='row' style='margin-top:10px'><button id='mail-save' class='primary'>Save mail</button><button id='mail-test'>Send test</button>" +
      "<span class='dim'>" + (mail.configured ? "configured" : "not configured") + "</span></div>" : "") +
    "</div>" +
    "<h2 class='sec'>Onshape</h2>" +
    "<div class='card'><p class='dim'>Register an Onshape OAuth app, then an <b>Element right panel</b> extension (Part Studio and Assembly) whose Action URL is:</p>" +
    "<p class='mono' style='word-break:break-all'>" + esc(location.origin) + "/static/onshape.html?documentId={$documentId}&amp;workspaceId={$workspaceOrVersionId}&amp;elementId={$elementId}&amp;server=https://cad.onshape.com</p>" +
    "<p class='dim'>Subscribe the company to the app. CAD users open the panel in Onshape and click Send — PDM exports STEP on the server (Onshape does not allow API calls from the browser).</p>" +
    "<div class='grid2'>" +
      "<div class='modal-field'><label>Base URL</label><input id='os-base' value='" + esc(osh.base_url || "https://cad.onshape.com") + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Access key</label><input id='os-key' value='" + esc(osh.access_key || "") + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field' style='grid-column:1/-1'><label>Secret</label><input id='os-secret' type='password' placeholder='" + (osh.secret_set ? "unchanged" : "") + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field' style='grid-column:1/-1'><label>Default PDM project</label><select id='os-proj'" + (canEdit?"":" disabled") + ">" +
        "<option value=''>—</option>" +
        (proj.projects||[]).map(p => "<option value='"+p.id+"'" + (String(osh.default_project_id||"")===String(p.id)?" selected":"") + ">"+esc(p.name)+"</option>").join("") +
      "</select></div>" +
      "<div class='modal-field' style='grid-column:1/-1'><label><input id='os-folder-sync' type='checkbox'" + (osh.folder_sync_enabled ? " checked" : "") + (canEdit ? "" : " disabled") + "> Enable Onshape folder sync</label></div>" +
    "</div>" +
    (canEdit ? "<div class='row' style='margin-top:10px'><button id='os-save' class='primary'>Save Onshape</button>" +
      "<span class='dim'>" + (osh.configured ? "configured" : "not configured") + "</span></div>" : "") +
    "</div>" +
    "<h2 class='sec'>Laser nesting defaults</h2>" +
    "<div class='card'><p class='dim'>Pre-fills the New laser nest panel. Blank fields keep the panel's built-in default. " +
    "Units of measure applies to the sheet/part dimensional fields.</p>" +
    "<div class='grid2'>" +
      "<div class='modal-field'><label>Units of measure</label><select id='nd-units'" + (canEdit?"":" disabled") + ">" +
        ["in","mm"].map(u => "<option value='"+u+"' " + (nd.units===u?"selected":"") + ">" + (u==="in"?"Inches":"Millimetres") + "</option>").join("") + "</select></div>" +
      "<div class='modal-field'><label>Part source unit</label><select id='nd-punit'" + (canEdit?"":" disabled") + ">" +
        ["auto","in","mm"].map(u => "<option value='"+u+"' " + (nd.partUnit===u?"selected":"") + ">" + (u==="auto"?"Auto (file units)":u==="in"?"Inches":"Millimetres") + "</option>").join("") + "</select></div>" +
      "<div class='modal-field'><label>Sheet width</label><input id='nd-width' value='" + esc(String(nd.width ?? "")) + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Sheet length</label><input id='nd-length' value='" + esc(String(nd.length ?? "")) + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Sheet margin</label><input id='nd-margin' value='" + esc(String(nd.margin ?? "")) + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Part spacing (H)</label><input id='nd-spx' value='" + esc(String(nd.spacingX ?? "")) + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Part spacing (V)</label><input id='nd-spy' value='" + esc(String(nd.spacingY ?? "")) + "'" + (canEdit?"":" disabled") + "></div>" +
      "<div class='modal-field'><label>Grain direction</label><select id='nd-grain'" + (canEdit?"":" disabled") + ">" +
        [["along_length","Vertical"],["along_width","Horizontal"],["any","Ignore"]].map(([u,l]) =>
          "<option value='"+u+"' " + (nd.grainDirection===u?"selected":"") + ">" + l + "</option>").join("") + "</select></div>" +
      "<div class='modal-field'><label>Nesting corner</label><select id='nd-corner'" + (canEdit?"":" disabled") + ">" +
        [["lower_left","Lower Left"],["lower_right","Lower Right"],["upper_left","Upper Left"],["upper_right","Upper Right"]].map(([u,l]) =>
          "<option value='"+u+"' " + (nd.nestingCorner===u?"selected":"") + ">" + l + "</option>").join("") + "</select></div>" +
      "<div class='modal-field'><label><input id='nd-gull' type='checkbox'" + (nd.guillotine?" checked":"") + (canEdit?"":" disabled") + "> Guillotine cut</label></div>" +
      "<div class='modal-field'><label><input id='nd-labels' type='checkbox'" + (nd.addPartLabels?" checked":"") + (canEdit?"":" disabled") + "> Add part labels</label></div>" +
    "</div>" +
    (canEdit ? "<div class='row' style='margin-top:10px'><button id='nd-save' class='primary'>Save nesting defaults</button></div>" : "") +
    "</div>" +
    "<div class='head'><h2 class='sec' style='margin:0'>Part-number prefixes</h2>" +
    "<div class='grow'></div>" +
    (canEdit ? "<button id='pn-add' class='primary'>＋ Add prefix</button>" : "") +
    "</div>" +
    "<div class='card'><p class='dim'>When a part number is left blank on upload or create, the server auto-assigns one as " +
    "<b class='mono'>PREFIX-000001</b>, <b class='mono'>PREFIX-000002</b>, … The prefix is looked up from the item's " +
    "<b>category</b> below. Categories with no configured prefix fall back to the category name itself.</p>" +
    prefixTable(prefixes, canEdit) +
    "<p class='dim muted' style='margin-top:10px'>Existing items keep their part numbers; changing a prefix only affects newly auto-assigned ones.</p></div>" +
    "<h2 class='sec'>My account</h2>" +
    "<div class='card'><p class='dim'>Password for your account (" + esc(myName()) + "). " +
    "Org admins can also reset any user's password from Administration.</p>" +
    "<div class='row'><button id='changepw'>Change password…</button></div></div>";
  const addBtn = $("pn-add");
  if (addBtn) addBtn.onclick = () => addPrefix();
  const cpBtn = $("changepw");
  if (cpBtn) cpBtn.onclick = () => changePassword();
  wireThemePicker(v);
  wireGpuPicker(v, canEdit);
  if ($("preview-style-save")) $("preview-style-save").onclick = async () => {
    try { await api("PUT", "/settings/preview-style", {preview_style:$("preview-style").value}); toast("Preview style saved", "ok"); }
    catch(e){ toast("Preview style: " + e.message, "err"); }
  };
  const mailSave = $("mail-save");
  if (mailSave) mailSave.onclick = async () => {
    const port = Number($("mail-port").value);
    try {
      await api("PUT", "/settings/mail", {
        host: $("mail-host").value.trim(),
        port: Number.isFinite(port) && port > 0 ? port : 587,
        username: $("mail-user").value.trim(),
        password: $("mail-pass").value,
        from: $("mail-from").value.trim(),
        tls: $("mail-tls").value,
        login_url: $("mail-url").value.trim()
      });
      toast("Mail server saved", "ok");
      render(false);
    } catch(e){ toast("Mail: " + e.message, "err"); }
  };
  const mailTest = $("mail-test");
  if (mailTest) mailTest.onclick = async () => {
    try {
      const d = await api("POST", "/settings/mail/test", {});
      toast("Test message sent to " + (d.to || "you"), "ok");
    } catch(e){ toast("Mail test: " + e.message, "err"); }
  };
  const osSave = $("os-save");
  if (osSave) osSave.onclick = async () => {
    try {
      await api("PUT", "/settings/onshape", {
        base_url: $("os-base").value.trim(),
        access_key: $("os-key").value.trim(),
        secret: $("os-secret").value,
        default_project_id: $("os-proj").value || null,
        folder_sync_enabled: $("os-folder-sync").checked
      });
      toast("Onshape API saved", "ok");
      render(false);
    } catch(e){ toast("Onshape: " + e.message, "err"); }
  };
  const ndSave = $("nd-save");
  if (ndSave) ndSave.onclick = async () => {
    const numOr = id => { const n = num($(id).value); return isFinite(n) ? n : null; };
    try {
      await api("PUT", "/settings/nest-defaults", {
        units: $("nd-units").value,
        part_unit: $("nd-punit").value,
        width: numOr("nd-width"),
        length: numOr("nd-length"),
        margin: numOr("nd-margin"),
        spacingX: numOr("nd-spx"),
        spacingY: numOr("nd-spy"),
        grainDirection: $("nd-grain").value,
        nestingCorner: $("nd-corner").value,
        guillotine: $("nd-gull").checked,
        addPartLabels: $("nd-labels").checked,
      });
      toast("Nesting defaults saved", "ok");
      render(false);
    } catch(e){ toast("Nesting defaults: " + e.message, "err"); }
  };
}
function prefixTable(prefixes, canEdit){
  if (!prefixes.length)
    return "<div class='empty'>No prefixes configured yet." + (canEdit ? " Add one above." : "") + "</div>";
  const rows = prefixes.map(p =>
    "<tr><td class='mono'>" + esc(p.prefix) + "</td>" +
    "<td>" + esc(p.category) + "</td>" +
    (canEdit ? "<td style='white-space:nowrap'>" +
      "<button class='small' data-edit='" + esc(p.category) + "'>Edit</button> " +
      "<button class='small danger' data-del='" + esc(p.category) + "'>Delete</button></td>" : "") +
    "</tr>").join("");
  const onEdit = canEdit
    ? (v) => v.querySelectorAll("[data-edit]").forEach(b =>
        b.addEventListener("click", () => editPrefix(b.dataset.edit)))
    : () => {};
  const onDel = canEdit
    ? (v) => v.querySelectorAll("[data-del]").forEach(b =>
        b.addEventListener("click", () => deletePrefix(b.dataset.del)))
    : () => {};
  const wrap = document.createElement("div");
  wrap.innerHTML = "<table><thead><tr><th>Prefix</th><th>Category</th>" +
    (canEdit ? "<th></th>" : "") + "</tr></thead><tbody>" + rows + "</tbody></table>";
  onEdit(wrap); onDel(wrap);
  return wrap.innerHTML;
}
async function addPrefix(){
  const f = await openModal("Add part-number prefix", [
    { id:"category", label:"Category", required:true, placeholder:"e.g. Drain Tray" },
    { id:"prefix", label:"Prefix", required:true, placeholder:"e.g. DT" }
  ], "Add prefix");
  if (!f) return;
  try {
    await api("POST", "/settings/part-number-prefixes",
      { category: f.category.trim(), prefix: f.prefix.trim() });
    toast("Prefix added", "ok");
    render(false);
  } catch(e){ toast("Add prefix: " + e.message, "err"); }
}
async function editPrefix(category){
  const d = await api("GET", "/settings/part-number-prefixes");
  const p = (d.prefixes || []).find(x => x.category === category);
  const f = await openModal("Edit prefix — " + category, [
    { id:"prefix", label:"New prefix", required:true, value: p ? p.prefix : "" }
  ], "Save");
  if (!f) return;
  try {
    await api("POST", "/settings/part-number-prefixes",
      { category: category, prefix: f.prefix.trim() });
    toast("Prefix updated", "ok");
    render(false);
  } catch(e){ toast("Edit prefix: " + e.message, "err"); }
}
async function deletePrefix(category){
  if (!confirm("Delete the prefix for \"" + category + "\"? New items in this category will fall back to the category name.")) return;
  try {
    await api("DELETE", "/settings/part-number-prefixes/" + encodeURIComponent(category));
    toast("Prefix deleted", "ok");
    render(false);
  } catch(e){ toast("Delete prefix: " + e.message, "err"); }
}

async function viewAdmin(v){
  if (!isRole("admin")){
    v.innerHTML = "<div class='card' style='max-width:560px'><h2>Administration</h2>" +
      "<p class='dim'>The Administration section is available to org admins. You are signed in as " +
      "<b>" + esc(myName()) + "</b> with role <b>" + esc(ME && ME.user ? ME.user.role : "?") + "</b>.</p></div>";
    return;
  }
  const [us, aud, vs] = await Promise.all([
    api("GET", "/users"),
    api("GET", "/audit?limit=100").catch(() => ({ events: [] })),
    api("GET", "/version").catch(() => ({}))
  ]);
  USERS = us.users || [];
  UMAP = {};
  for (const u of USERS) UMAP[u.id] = u;
  v.innerHTML =
    "<div class='head'><h2>Administration</h2><div class='grow'></div>" +
    "<button id='adduser' class='primary'>＋ Add user</button>" +
    "<button id='invbtn'>＋ Invite user</button></div>" +
    "<h2 class='sec'>Appearance</h2>" +
    "<div class='card'><p class='dim'>Colour theme for the web workspace. Stored in this browser.</p>" +
    themePickerHTML() + "</div>" +
    "<h2 class='sec'>Users</h2>" + usersTable(us.users || []) +
    "<h2 class='sec'>Audit log</h2>" +
    "<div style='max-height:420px;overflow:auto'>" + auditTable(aud.events || []) + "</div>" +
    "<h2 class='sec'>My client settings (JSON)</h2>" +
    "<div class='card'><textarea id='setjson' rows='6' class='mono' style='width:100%'>{}</textarea>" +
    "<div class='row'><button id='setsave' class='primary'>Save settings</button>" +
    "<span id='setstat' class='dim'></span></div></div>" +
    "<h2 class='sec'>Server build</h2>" +
    "<div class='card dim mono'>" + esc(JSON.stringify(vs)) +
    "<div style='margin-top:10px'><button class='small' id='syslink'>System status →</button></div></div>";
  $("adduser").onclick = addUser;
  $("invbtn").onclick = inviteUser;
  $("syslink").onclick = () => go("system");
  wireThemePicker(v);
  v.querySelectorAll("[data-tok]").forEach(b => b.addEventListener("click", () => giveToken(UMAP[b.dataset.tok])));
  v.querySelectorAll("[data-pw]").forEach(b => b.addEventListener("click", () => resetPassword(UMAP[b.dataset.pw])));
  v.querySelectorAll("[data-edit]").forEach(b => b.addEventListener("click", () => editUser(UMAP[b.dataset.edit])));
  v.querySelectorAll("[data-active]").forEach(b => b.addEventListener("click", () => toggleUserActive(UMAP[b.dataset.active])));
  $("setsave").onclick = async () => {
    try {
      const payload = JSON.parse($("setjson").value || "{}");
      await api("PUT", "/me/settings", { payload });
      $("setstat").textContent = "saved " + new Date().toLocaleTimeString();
    } catch(e){ $("setstat").textContent = "JSON error: " + e.message; }
  };
  api("GET", "/me/settings")
    .then(d => { $("setjson").value = JSON.stringify(d.payload != null ? d.payload : {}, null, 2); })
    .catch(() => { $("setjson").value = "{}"; });
}

// ── project explorer ────────────────────────────────────────────────────
function bulkBarHTML(){
  const n = ROUTE._sel ? ROUTE._sel.size : 0;
  if (!n) return "";
  return "<div class='bulkbar' id='bulkbar'><b>" + n + " selected</b>" +
    "<button class='small' id='bmove'>Move to folder</button>" +
    "<button class='small' id='bowner'>Assign owner</button>" +
    "<button class='small' id='bco'>Check out</button>" +
    "<button class='small' id='bflat'>Export flat patterns</button>" +
    "<div class='grow'></div>" +
    "<button class='small' id='bclear'>Clear</button></div>";
}
function wireBulk(){
  const ids = [...ROUTE._sel];
  const bm = $("bmove"), bo = $("bowner"), bc = $("bco"), bf = $("bflat"), bl = $("bclear");
  if (bm) bm.onclick = () => moveItems(ROUTE.pid, ids);
  if (bo) bo.onclick = () => assignOwner(ids);
  if (bc) bc.onclick = () => checkoutBulk(ids);
  if (bf) bf.onclick = () => exportFlatPatterns(ids);
  if (bl) bl.onclick = () => { ROUTE._sel.clear(); refreshExpTable(); };
}
async function refreshExpTable(){
  const wrap = $("exptablewrap");
  if (!wrap) return;
  const q = ROUTE._q.toLowerCase();
  let list = (ROUTE._items || []).filter(it => !q ||
    (it.part_number || "").toLowerCase().includes(q) ||
    (it.description || "").toLowerCase().includes(q) ||
    (it.filename || "").toLowerCase().includes(q));
  if (ROUTE._folder) list = list.filter(it => it.folder_id === ROUTE._folder);
  list = sortItems(list, ROUTE._sort, ROUTE._dir);
  wrap.innerHTML = bulkBarHTML() + itemsTable(list, false, ROUTE._sel);
  await attachThumbs(wrap);
  wireItemTable(wrap, {
    onSort: k => {
      if (ROUTE._sort === k) ROUTE._dir = ROUTE._dir === "asc" ? "desc" : "asc";
      else { ROUTE._sort = k; ROUTE._dir = "desc"; }
      render(false);
    },
    onSel: () => refreshExpTable(),
    selSet: ROUTE._sel,
  });
  wireBulk();
}
function treeHTML(){
  const folders = ROUTE._folders || [], items = ROUTE._items || [];
  const byPar = {};
  for (const f of folders){
    const k = f.parent_id || "root";
    (byPar[k] = byPar[k] || []).push(f);
  }
  for (const k in byPar) byPar[k].sort((a, b) => a.name.localeCompare(b.name));
  const count = fid => items.filter(it => it.folder_id === fid).length;
  let h = "";
  const rootItems = items.filter(i => !i.folder_id);
  h += "<div class='tnode" + (!ROUTE._folder ? " on" : "") + "' data-f='' style='padding-left:8px'>" +
    "<span class='caret'></span><span class='ficon'>" + kindSvg("ic-project") + "</span><span>Project root</span>" +
    "<span class='cnt'>" + rootItems.length + "</span></div>";
  const node = (f, depth) => {
    const open = ROUTE._expanded.has(f.id);
    const kids = byPar[f.id] || [];
    h += "<div class='tnode" + (ROUTE._folder === f.id ? " on" : "") + "' data-f='" + f.id +
      "' style='padding-left:" + (10 + depth * 14) + "px'>" +
      "<span class='caret" + (kids.length ? " tog" : "") + "' data-c='" + f.id + "'>" +
      (open && kids.length ? "▾" : "▸") + "</span>" +
      "<span class='ficon'>" + kindSvg("ic-folder") + "</span><span>" + esc(f.name) + "</span>" +
      "<span class='cnt'>" + count(f.id) + "</span></div>";
    if (open){
      for (const it of items.filter(i => i.folder_id === f.id))
        h += "<div class='tnode' data-i='" + it.id + "' style='padding-left:" + (10 + (depth + 1) * 14) + "px'>" +
          "<span class='caret'></span><span class='ficon'>" +
          kindInfo(it.mime_type, it.filename).icon + "</span>" +
          "<span title='" + esc(itemName(it)) + "'>" + esc(itemName(it)) + "</span></div>";
      for (const k2 of kids) node(k2, depth + 1);
    }
  };
  for (const f of byPar["root"] || []) node(f, 0);
  return h;
}
function wireExpTree(){
  const tree = $("exptree");
  if (!tree) return;
  tree.querySelectorAll(".tnode[data-f]").forEach(n =>
    n.addEventListener("click", () => {
      const f = n.dataset.f || null;
      ROUTE._folder = ROUTE._folder === f ? null : f;
      render(false);
    }));
  tree.querySelectorAll(".caret.tog").forEach(c =>
    c.addEventListener("click", e => {
      e.stopPropagation();
      const id = c.dataset.c;
      if (ROUTE._expanded.has(id)) ROUTE._expanded.delete(id);
      else ROUTE._expanded.add(id);
      render(false);
    }));
  tree.querySelectorAll(".tnode[data-i]").forEach(n =>
    n.addEventListener("click", () => openDetail(n.dataset.i)));
}
// Project home (§4.1.2): one header + tabbed body. The explorer is the
// Items tab; Activity/BOM/Manufacturing/Settings round it out.
async function viewExplorer(v){
  const pid = ROUTE.pid;
  if (!pid){ go("projects"); return; }
  if (!PMAP[pid]) await loadCommon();
  const proj = PMAP[pid];
  if (!ROUTE._ptab) ROUTE._ptab = "items";
  const d = await api("GET", "/projects/" + pid + "/items");
  const fd = await api("GET", "/projects/" + pid + "/folders").catch(() => ({ folders: [] }));
  ROUTE._items = d.items || [];
  ROUTE._folders = fd.folders || [];
  if (!ROUTE._q) ROUTE._q = "";
  if (!ROUTE._sort) ROUTE._sort = "modified_at";
  if (!ROUTE._dir) ROUTE._dir = "desc";
  if (!(ROUTE._sel instanceof Set)) ROUTE._sel = new Set();
  if (!(ROUTE._expanded instanceof Set)) ROUTE._expanded = new Set(ROUTE._folders.map(f => f.id));
  const ptabs = [["items","Items"],["activity","Activity"],["bom","Bills of material"],
    ["manufacturing","Manufacturing"],["settings","Settings"]];
  v.innerHTML =
     "<div class='head'>" +
    "<button id='backproj' class='small'>← Projects</button>" +
     projectCover(proj, "explorer-cover") +
    "<h2>" + esc(proj ? proj.name : "Project") + archBadge(proj && proj.archived_at) + "</h2>" +
    "<span class='dim'>" + esc(proj ? (proj.description || "") : "") + "</span>" +
    "<span class='dim muted'>" + ROUTE._items.length + " items · created " +
    fmtTime(proj ? proj.created_at : null) + "</span>" +
    "<div class='grow'></div>" +
     "<button id='newfolder' class='small'>＋ Folder</button>" +
     "<button id='newitem' class='small'>＋ Item</button>" +
     (isRole("editor") ? "<button id='newscpart' class='small' title='Model a new part in the SolidCraft web CAD app, then Save to PDM stores it as a new item (blank part number = auto-assign)'>✎ New part in SolidCraft web</button>" : "") +
     "<button id='uploadbtn' class='primary small'>⬆ Upload</button>" +
    "<button id='projkebab' class='kebab-btn vis' title='Project actions'>⋮</button>" +
    "</div>" +
    "<div class='tabs'>" + ptabs.map(t =>
      "<div class='tab" + (ROUTE._ptab === t[0] ? " on" : "") + "' data-ptab='" + t[0] + "'>" +
      t[1] + "</div>").join("") +
    "</div>" +
    "<div id='ptab'></div>";
  v.querySelectorAll(".tab[data-ptab]").forEach(t => t.addEventListener("click", () => {
    ROUTE._ptab = t.dataset.ptab;
    v.querySelectorAll(".tab").forEach(x => x.classList.toggle("on", x === t));
    pushHash();
    renderProjectTab(v);
  }));
  $("backproj").onclick = () => go("projects");
  $("projkebab").addEventListener("click", e => openKebabMenu(e, pid, "proj"));
  $("newfolder").onclick = async () => {
    const f = await openModal("New folder", [
      { id:"name", label:"Name", required:true, placeholder:"e.g. Housings" },
      { id:"parent", label:"Parent folder (for nesting)", type:"select",
        options:[{value:"",label:"(project root)"}]
          .concat((ROUTE._folders || []).map(x => ({ value:x.id, label:x.name }))) }
    ], "Create folder");
    if (!f) return;
    try {
      await api("POST", "/projects/" + pid + "/folders", { name: f.name, parent_id: f.parent || null });
      toast("Folder created", "ok");
      render(false);
    } catch(e){ toast("Folder failed: " + e.message, "err"); }
  };
  $("newitem").onclick = async () => {
    const f = await openModal("New item", [
      { id:"desc", label:"Description", required:true, placeholder:"what is this part?" },
      { id:"pn", label:"Part number (blank = auto-assign)" },
      { id:"cls", label:"Classification", type:"select", options:["BE","ASM","MFG","SC"], value:"BE" },
      { id:"folder", label:"Folder", type:"select",
        options:[{value:"",label:"(project root)"}]
          .concat((ROUTE._folders || []).map(x => ({ value:x.id, label:x.name }))) }
    ], "Create item");
    if (!f) return;
    try {
      const d2 = await api("POST", "/projects/" + pid + "/items",
        { part_number: f.pn || null, description: f.desc,
          classification: f.cls, folder_id: f.folder || null });
      toast("Item created: " + d2.item.part_number, "ok");
      render(false);
    } catch(e){ toast("Create failed: " + e.message, "err"); }
  };
  const nsp = $("newscpart");
  if (nsp) nsp.onclick = () => newPartInSolidcraftWeb(pid);
  $("uploadbtn").onclick = () => pickFiles(pid);
  attachThumbs(v);
  await renderProjectTab(v);
}
async function renderProjectTab(v){
  const box = $("ptab");
  if (!box) return;
  if (ROUTE._ptab === "items") await projectItemsTab(box);
  else if (ROUTE._ptab === "activity") await projectActivityTab(box);
  else if (ROUTE._ptab === "bom"){
    const p = PMAP[ROUTE.pid] || {};
    const mainItem = p.main_assembly_id ? (ROUTE._items || []).find(i => i.id === p.main_assembly_id) : null;
    const rootHtml = mainItem
      ? "<div class='card' style='max-width:640px;margin-bottom:14px'>" +
        "<h2 class='sec'>BOM root</h2>" +
        "<div class='row' style='align-items:center'>" +
          "<span class='thumb-sm pcov' data-thumb='/items/" + mainItem.id + "/thumbnail'></span>" +
          "<div style='min-width:0'><div class='mono'>" + esc(mainItem.part_number) + "</div>" +
          "<div class='dim'>" + esc(mainItem.description || "(no description)") + "</div></div>" +
          "<div class='grow'></div>" +
          "<button class='small' data-openma='" + mainItem.id + "'>Open item</button>" +
        "</div>" +
        "<p class='dim' style='font-size:12.5px;margin-top:10px'>The bill of materials is built from this main assembly.</p>" +
      "</div>"
      : "";
    box.innerHTML = rootHtml + emptyState("ic-bom", "Bills of material — coming soon",
      "Parent/child structure with quantities, rollups and where-used lands with the BOM backend increment. " +
      (mainItem ? "Full structure will build from the main assembly above." :
        "Set an item as the main assembly (right-click an item, or Settings) to build the BOM from it — and to use it as the project image."));
    const ob = box.querySelector("[data-openma]");
    if (ob) ob.onclick = () => openDetail(ob.dataset.openma);
    attachThumbs(box);
  }
  else if (ROUTE._ptab === "manufacturing") await projectMfgTab(box);
  else if (ROUTE._ptab === "settings") projectSettingsTab(box);
}
async function projectItemsTab(box){
  box.innerHTML =
    "<div class='explorer' id='droproot'>" +
      "<div class='tree' id='exptree'>" + treeHTML() + "</div>" +
      "<div>" +
        "<div class='row' style='margin-bottom:8px'>" +
          "<input id='exq' placeholder='Filter by part no, description, filename…' value='" + esc(ROUTE._q) + "' style='min-width:200px;flex:1'>" +
          "<select id='exsort'>" +
            "<option value='modified_at'>Last modified</option>" +
            "<option value='_name'>Name</option>" +
            "<option value='part_number'>Part number</option>" +
            "<option value='owner_name'>Owner</option>" +
          "</select>" +
          "<select id='exdir'>" +
            "<option value='desc'>newest first</option>" +
            "<option value='asc'>oldest first</option>" +
          "</select>" +
        "</div>" +
        "<div id='exptablewrap'></div>" +
      "</div>" +
      "<div class='dropzone hidden' id='dz'>⬆ Drop files to upload into " +
      esc(PMAP[ROUTE.pid] ? PMAP[ROUTE.pid].name : "this project") + "</div>" +
    "</div>";
  $("exsort").value = ROUTE._sort;
  $("exdir").value = ROUTE._dir;
  $("exsort").onchange = e => { ROUTE._sort = e.target.value; render(false); };
  $("exdir").onchange = e => { ROUTE._dir = e.target.value; render(false); };
  $("exq").addEventListener("input", e => { ROUTE._q = e.target.value; refreshExpTable(); });
  await refreshExpTable();
  await attachThumbs(box);
  wireExpTree();
  // drag & drop
  const dr = $("droproot");
  let dragN = 0;
  dr.addEventListener("dragenter", e => { e.preventDefault(); dragN++; $("dz").classList.remove("hidden"); });
  dr.addEventListener("dragover", e => e.preventDefault());
  dr.addEventListener("dragleave", e => {
    e.preventDefault();
    if (--dragN <= 0){ dragN = 0; $("dz").classList.add("hidden"); }
  });
  dr.addEventListener("drop", async e => {
    e.preventDefault(); dragN = 0; $("dz").classList.add("hidden");
    const files = [...e.dataTransfer.files];
    if (!files.length) return;
    const f = await uploadModal(ROUTE.pid, files);
    if (f) doUpload(ROUTE.pid, f, files);
  });
}
async function projectActivityTab(box){
  if (!USERS.length) await loadCommon();
  const d = await api("GET", "/audit?limit=200").catch(() => ({ events: [] }));
  const pid = ROUTE.pid;
  const itemIds = new Set((ROUTE._items || []).map(i => i.id));
  const evs = (d.events || []).filter(ev =>
    (ev.object_type === "project" && ev.object_id === pid) ||
    (ev.object_type === "item" && itemIds.has(ev.object_id)));
  box.innerHTML = evs.length ? feed(evs) :
    emptyState("ic-activity", "No project activity yet",
      "Check-ins, releases and folder changes in this project will appear here.");
}
async function projectMfgTab(box){
  const mfg = (ROUTE._items || []).filter(it => it.classification === "MFG");
  box.innerHTML = mfg.length ? itemsTable(mfg, false) :
    emptyState("ic-manufacturing", "No manufacturing items in this project",
      "Create items with the MFG classification, then build NC programs and production jobs in the manufacturing hub.",
      "<div><button class='cta' data-go='mfg'>Open the manufacturing hub</button></div>");
  const g = box.querySelector("[data-go]");
  if (g) g.onclick = () => go("mfg");
  await attachThumbs(box);
  wireItemTable(box, {});
}
async function projectSettingsTab(box){
  const pid = ROUTE.pid;
  const p = PMAP[pid] || {};
  const main = p.main_assembly_id
    ? (ROUTE._items || []).find(i => i.id === p.main_assembly_id) : null;
  const maSection =
    "<div style='margin-top:14px;border-top:1px solid var(--line);padding-top:14px'>" +
      "<h2 class='sec' style='margin-bottom:6px'>Main assembly</h2>" +
      "<p class='dim' style='font-size:12.5px;margin:0 0 10px'>The item the bill of materials is built from. Its rendered thumbnail is also used as this project's image.</p>" +
      (main
        ? "<div class='row' style='align-items:center'>" +
            "<span class='thumb-sm pcov' data-thumb='/items/" + main.id + "/thumbnail'></span>" +
            "<div style='min-width:0'><div class='mono'>" + esc(main.part_number) + "</div>" +
            "<div class='dim'>" + esc(main.description || "(no description)") + "</div></div>" +
            "<div class='grow'></div>" +
            "<button class='small' id='ma-change'>Change</button> " +
            "<button class='small' id='ma-clear'>Clear</button>" +
          "</div>"
        : "<div class='row'><button class='cta' id='ma-set'>Set main assembly…</button></div>") +
    "</div>";
  box.innerHTML =
    "<div class='card' style='max-width:640px'>" +
    "<h2 class='sec'>Project settings</h2>" +
    "<div class='dmeta' style='margin:0'>" +
      "<div><span class='k'>Name</span> " + esc(p.name || "—") + "</div>" +
      "<div><span class='k'>Description</span> " + esc(p.description || "—") + "</div>" +
      "<div><span class='k'>Items</span> " + (ROUTE._items || []).length + "</div>" +
      "<div><span class='k'>Folders</span> " + (ROUTE._folders || []).length + "</div>" +
      "<div><span class='k'>Created</span> " + fmtDate(p.created_at) + "</div>" +
      "<div><span class='k'>State</span> " +
      (p.archived_at ? archBadge(true) : "<span class='dim'>active</span>") + "</div>" +
    "</div>" +
    maSection +
    "<div id='onshape-folder-section' style='margin-top:14px;border-top:1px solid var(--line);padding-top:14px'><h2 class='sec'>Onshape folder sync</h2><p class='dim'>Loading…</p></div>" +
    "<div id='os-import-section' style='margin-top:14px;border-top:1px solid var(--line);padding-top:14px'>" +
    "<h2 class='sec'>Import from Onshape (Dutchman)</h2>" +
    "<p class='dim'>Fetches one Part Studio straight from Onshape with the Dutchman Onshape reader " +
    "(the org's Onshape API keys), checks the .onshape-bundle in as a new version, and converts it " +
    "to a SolidCraft design. The Dutchman channel must reference a build with the Onshape reader " +
    "(System › Dutchman). Re-importing the same Part Studio adds a version to the existing item.</p>" +
    "<div class='modal-field'><label>Part Studio URL</label>" +
    "<input id='os-import-url' placeholder='https://cad.onshape.com/documents/&lt;id&gt;/w/&lt;workspace&gt;/e/&lt;element&gt;'></div>" +
    "<div class='modal-field'><label>Name / part number (optional)</label><input id='os-import-name'></div>" +
    "<div class='row'><label style='display:inline-flex;gap:6px;align-items:center'><input type='checkbox' id='os-import-convert' checked> Check in the converted SolidCraft design</label> " +
    "<button id='os-import-go' class='primary'>Import</button></div>" +
    "<h3>Manual STEP fallback</h3>" +
    "<p class='dim'>Export STEP from Onshape, then upload it here to convert its geometry to SolidCraft. " +
    "This does not require Onshape API keys or the Dutchman Onshape reader. STEP does not include the original feature history. " +
    "The Part Studio URL above is optional for this upload; when supplied, it reuses the linked item, including an item left by a failed import.</p>" +
    "<div class='modal-field'><label for='os-step-file'>Onshape STEP export</label><input id='os-step-file' type='file' accept='.step,.stp'></div>" +
    "<button id='os-step-go'>Upload STEP</button>" +
    "<p id='os-import-status' role='status' aria-live='polite' style='white-space:pre-wrap'></p>" +
    "</div>" +
    "<p class='dim' style='font-size:12.5px;margin-top:12px'>Part-number prefixes and the approval gate are org-wide — set them under Settings. Use the project menu (⋮) to archive or delete this project.</p>" +
    "</div>";
  const setBtn = $("ma-set"), chg = $("ma-change"), clr = $("ma-clear");
  if (setBtn) setBtn.onclick = () => chooseMainAssembly(pid);
  if (chg) chg.onclick = () => chooseMainAssembly(pid);
  if (clr) clr.onclick = () => setMainAssembly(pid, null);
  attachThumbs(box);
  const section = $("onshape-folder-section");
  try {
    const data = await api("GET", "/projects/" + pid + "/onshape-folder");
    if (!section.isConnected) return;
    if (!data.enabled) {
      section.innerHTML = "<h2 class='sec'>Onshape folder sync</h2><p class='dim'>Disabled in organization Settings.</p>";
    } else {
    const link = data.link;
    const result = link && link.result || {};
    section.innerHTML = "<h2 class='sec'>Onshape folder sync</h2>" +
      "<p class='dim'>Imports STEP files for parts and assemblies in the selected folder's direct documents, using each document's default workspace. Existing items in other projects are reported as conflicts.</p>" +
      "<div class='modal-field'><label>Onshape folder ID</label><input id='os-folder-id' value='" + esc(link ? link.folder_id : "") + "' placeholder='Onshape folder ID'></div>" +
      "<div class='row'><button id='os-folder-save'>Link folder</button> " + (link ? "<button id='os-folder-run' class='primary'" + (link.status === "running" ? " disabled" : "") + ">" + (link.status === "running" ? "Syncing…" : "Sync now") + "</button>" : "") + "</div>" +
      (link ? "<p class='dim'>Linked: " + esc(link.folder_name) + " · " + esc(link.status) + "</p>" : "") +
      (result.imported != null ? "<p>Imported or updated " + esc(result.imported) + " item(s) from " + esc(result.documents) + " document(s).</p>" : "") +
      (result.error ? "<p class='err'>" + esc(result.error) + "</p>" : "") +
      ((result.conflicts || []).length ? "<p>Conflicts:</p><ul>" + result.conflicts.map(x => "<li>" + esc(x) + "</li>").join("") + "</ul>" : "") +
      ((result.errors || []).length ? "<p>Errors:</p><ul>" + result.errors.map(x => "<li>" + esc(x) + "</li>").join("") + "</ul>" : "");
    $("os-folder-save").onclick = async () => {
      try { await api("PUT", "/projects/" + pid + "/onshape-folder", {folder_id:$("os-folder-id").value.trim()}); await projectSettingsTab(box); }
      catch(e){ toast("Onshape folder: " + e.message, "err"); }
    };
    const run = $("os-folder-run");
    if (run) run.onclick = async () => {
      try { await api("POST", "/projects/" + pid + "/onshape-folder/sync", {}); await projectSettingsTab(box); }
      catch(e){ toast("Onshape sync: " + e.message, "err"); }
    };
    if (link && link.status === "running") setTimeout(() => { if (section.isConnected && ROUTE.pid === pid && ROUTE._ptab === "settings") projectSettingsTab(box); }, 5000);
    }
  } catch(e) { if (section.isConnected) section.textContent = "Onshape folder sync: " + e.message; }
  const osGo = $("os-import-go");
  const stepGo = $("os-step-go"), status = $("os-import-status");
  stepGo.onclick = async () => {
    const file = $("os-step-file").files[0];
    if (!file) { toast("Choose an exported STEP file first", "err"); return; }
    const options = {
      url: $("os-import-url").value.trim(), name: $("os-import-name").value.trim(),
      convert: $("os-import-convert").checked
    };
    stepGo.disabled = true; osGo.disabled = true;
    status.textContent = "Uploading STEP" + (options.convert ? " and converting to SolidCraft…" : "…");
    try {
      const result = await importOnshapeStep(pid, file, options);
      status.textContent = "STEP saved to " + result.item.part_number + " as v" + result.version.seq +
        (options.convert ? "; SolidCraft design checked in. See the item's Versions tab." : ". See the item's Versions tab.");
      toast("Onshape STEP import complete", "ok");
    } catch(e) {
      status.textContent = "STEP import: " + e.message;
      toast(status.textContent, "err", 10000);
    } finally {
      stepGo.disabled = false; osGo.disabled = false;
    }
  };
  if (osGo) osGo.onclick = async () => {
    const url = $("os-import-url").value.trim();
    if (!url) { toast("Paste a Part Studio link first", "err"); return; }
    const name = $("os-import-name").value.trim() || undefined;
    const convert = $("os-import-convert").checked;
    osGo.disabled = true; stepGo.disabled = true; osGo.textContent = "Importing…";
    status.textContent = "Starting Onshape import…";
    try {
      const r = await api("POST", "/projects/" + pid + "/onshape-import", {
        url, name
      });
      status.textContent = "Fetching from Onshape. Job: " + r.job.id + " (see Tasks).";
      toast("Fetching the Part Studio from Onshape — this can take a while…", "ok", 10000);
      if (!await waitForVersionJob(r.job.id, { isConnected: true })) return;
      if (convert) {
        toast("Importing to SolidCraft…", "ok", 10000);
        const d = await api("GET", "/items/" + r.item.id);
        if (!d.head_version) throw new Error("the imported item has no version");
        await importVersionToSolidcraft(r.item.id, d.head_version);
      }
      toast("Onshape import complete", "ok");
      ALLITEMS = []; ALLITEMS_AT = 0;
      if (box.isConnected && ROUTE.pid === pid && ROUTE._ptab === "settings") await projectSettingsTab(box);
    } catch(e) {
      status.textContent += "\nOnshape import failed: " + e.message +
        "\nYou can upload an exported STEP below to import the geometry instead.";
      toast("Onshape import: " + e.message, "err", 10000);
    } finally {
      stepGo.disabled = false;
      if (osGo.isConnected) { osGo.disabled = false; osGo.textContent = "Import"; }
    }
  };
}

// A manual export stays a STEP source; linking it to Onshape never changes the
// format passed to Dutchman. No Onshape network access or API keys are needed.
async function importOnshapeStep(pid, file, options){
  if (!/\.(step|stp)$/i.test(file.name)) throw new Error("Choose a .step or .stp file");
  if (!file.size) throw new Error("The STEP file is empty");
  let source = null, item;
  if (options.url) {
    const match = options.url.match(/^https:\/\/(?:cad\.)?onshape\.com\/documents\/([a-f\d]{24})\/(w|v)\/([a-f\d]{24})\/e\/([a-f\d]{24})(?:[/?#]|$)/i);
    if (!match) throw new Error("Use a Part Studio workspace or version URL, or leave the URL blank");
    source = { document:match[1], element:match[4] };
    const found = await api("GET", "/integrations/onshape/lookup?document_id=" + source.document + "&element_id=" + source.element);
    item = found.item;
    if (item && item.project_id !== pid)
      throw new Error("This Part Studio is already linked to an item in another project");
  }
  if (!item) {
    const created = await api("POST", "/projects/" + pid + "/items", {
      description:options.name || file.name.replace(/\.[^.]+$/, ""),
      part_number:options.name || null, classification:"BE", folder_id:null
    });
    item = created.item;
  }
  if (source) {
    for (const [key, value] of [["onshape.document_id", source.document], ["onshape.element_id", source.element], ["onshape.url", options.url]])
      await api("PUT", "/items/" + item.id + "/properties/" + encodeURIComponent(key), { value, value_type:"string" });
  }
  const result = await checkinItem(item.id, file, "Manual Onshape STEP export" + (options.url ? " from " + options.url : ""));
  ALLITEMS = []; ALLITEMS_AT = 0;
  if (options.convert) {
    try {
      await importVersionToSolidcraft(item.id, result.version);
    } catch(e) {
      throw new Error("STEP saved to " + item.part_number + " as v" + result.version.seq +
        ", but SolidCraft conversion failed: " + e.message + ". Retry Convert to SolidCraft from the item's Versions tab.");
    }
  }
  return { item, version:result.version };
}

// ── item detail ─────────────────────────────────────────────────────────
function detailTabs(){
  const d = DETAIL.data;
  const vs = d && d.versions ? d.versions.length : 0;
  return [
    ["overview", "Overview"],
    ["preview", "Preview"],
    ["versions", "Versions", vs || null],
    ["revisions", "Revisions", (DETAIL.revs || []).length || null],
    ["deps", "Dependencies"],
    ["bom", "BOM"],
    ["mfg", "Sheet metal"],
    ["comments", "Comments", (DETAIL.comments || []).length || null],
    ["activity", "Activity"],
    ["perm", "Permissions"],
  ];
}
function sourceFilename(vv, d){ return vv.filename || (d.file && d.file.filename) || ""; }
function versionDownloadName(vv,d){
  const name=sourceFilename(vv,d)||d.item.part_number, dot=name.lastIndexOf(".");
  return dot>0 ? name.slice(0,dot)+"-v"+vv.seq+name.slice(dot) : name+"-v"+vv.seq;
}
// Formats the server's `cad.solidcraft` job converts: native CAD goes through
// Dutchman (feature history where it maps), geometry-only formats through the
// SolidCraft CLI.
const CONVERTIBLE_EXTS = ["f3d","f3z","sldprt","sldasm","catpart","pvz","sat","sab","smt","smb",
  "onshape-bundle",
  "step","stp","igs","iges","ipt","iam","fcstd","3dm","stl","obj","3mf","glb","gltf"];
function extOf(filename){
  const n = String(filename || ""); const dot = n.lastIndexOf(".");
  return dot > 0 ? n.slice(dot + 1).toLowerCase() : "";
}
function nativeConversionAllowed(vv, d){
  const ext = String(vv.native_format || extOf(sourceFilename(vv,d))).toLowerCase();
  return CONVERTIBLE_EXTS.includes(ext);
}
function convertibleByName(filename){
  return CONVERTIBLE_EXTS.includes(extOf(filename));
}
function serverPreviewAllowed(vv,d){
  return nativeConversionAllowed(vv,d) || /\.(sc|solidcraft|solvecraft|json)$/i.test(sourceFilename(vv,d));
}
function nativeAction(vv,d){
  if (!vv || !isRole("editor") || (!vv.derived_solidcraft_sha256 && !nativeConversionAllowed(vv,d))) return "";
  return "<button class='small' data-native='" + vv.id +
    "' title='Import this version into SolidCraft with Dutchman (keeping the editable feature tree where it maps) and check the result in as a new version'>" +
    "Convert to SolidCraft</button>";
}
// ── SolidCraft web (the CAD app, bundled in the image and run in a same-origin
// iframe): it fetches the version's design bytes from PDM's public design route,
// and "Save to PDM" checks the edited design back in as a new immutable version. ──
function isNativeDesign(vv,d){
  return /\.(sc|solidcraft|solvecraft|json)$/i.test(sourceFilename(vv,d));
}
function solidcraftWebOpenable(vv,d){
  if (!vv) return false;
  if (isNativeDesign(vv,d)) return true;
  if (vv.derived_solidcraft_sha256) return true;
  return isRole("editor") && nativeConversionAllowed(vv,d);
}
async function openSolidcraftWeb(itemId, versionId){
  try {
    const d = await api("GET", "/items/" + itemId);
    const version = versionId ? (d.versions || []).find(v => v.id === versionId) : d.head_version;
    if (!version) throw new Error("No version available to open");
    const openedVersionId = version.id;
    let designName;
    if (isNativeDesign(version,d)){
      designName = sourceFilename(version,d);
    } else {
      if (!version.derived_solidcraft_sha256){
        const result = await api("POST", "/versions/" + openedVersionId + "/solidcraft");
        if (!result.artifact.sha256 && result.job) await waitForVersionJob(result.job.id, { isConnected:true });
        const data = await api("GET", "/items/" + itemId);
        const cur = (data.versions || []).find(v => v.id === openedVersionId);
        if (!cur || !cur.derived_solidcraft_sha256)
          throw new Error((cur && cur.derived_solidcraft_error) || "No SolidCraft design available for this version");
      }
      const meta = await api("GET", "/versions/" + openedVersionId + "/solidcraft");
      designName = meta.filename;
    }
    if (!designName) throw new Error("This version has no file name to open");
    await PdmEditors.openSolidcraft({ version, name: designName,
      designUrl: "/public/design/" + openedVersionId + "/" + encodeURIComponent(designName),
      checkin: isRole("editor") ? makeScCheckin(itemId, version, designName) : null,
      onSaved: () => { if (CUR_RELOAD) CUR_RELOAD(); },
    });
  } catch(e){ toast("SolidCraft web: " + e.message, "err", 8000); }
}
// Check the design bytes into the item as a new immutable version (checking
// the item out first when unheld). base null = the item's first version.
async function scCheckin(itemId, base, versionId, designName, buffer, message){
  const cur = await api("GET", "/items/" + itemId);
  if (base !== null && (!cur.head_version || cur.head_version.id !== versionId))
    throw new Error("The vault head has changed since this design was opened. Download your edits and compare with the latest version.");
  if (cur.lock && cur.lock.user_id !== myId()) throw new Error("The item is checked out by " + lockName(cur.lock));
  if (!cur.lock) await api("POST", "/items/" + itemId + "/checkout", { workstation:location.hostname });
  const up = await api("PUT", "/blobs", null, new Uint8Array(buffer));
  const result = await api("POST", "/items/" + itemId + "/checkin", {
    sha256:up.blob.sha256, base_version:base, commit_message:message,
    filename:designName, mime_type:"application/octet-stream",
  });
  ALLITEMS = []; ALLITEMS_AT = 0;
  return result.version;
}
// Successive check-ins of one design (the editor's Save to PDM button): the
// base tracks the newest version this session checked in.
function makeScCheckin(itemId, version, designName){
  let head = version;
  return async (buffer, message) => {
    const v = await scCheckin(itemId, head.seq, head.id, designName, buffer, message);
    head = v;
    return v;
  };
}
// "New part in SolidCraft web" (project explorer): a blank SolidCraft web
// session; Save to PDM asks for the item's metadata (part number blank =
// auto-assign from the org's prefix) and checks the design in as v1.
async function newPartInSolidcraftWeb(pid){
  try {
    const fd = await api("GET", "/projects/" + pid + "/folders").catch(() => ({ folders: [] }));
    const folders = fd.folders || [];
    await PdmEditors.openSolidcraft({
      createNew: isRole("editor") ? async (buffer, into) => {
        const f = await openModal("New part", [
          { id:"desc", label:"Description", required:true, placeholder:"what is this part?" },
          { id:"pn", label:"Part number (blank = auto-assign)" },
          { id:"cls", label:"Classification", type:"select", options:["BE","ASM","MFG","SC"], value:"BE" },
          { id:"folder", label:"Folder", type:"select",
            options:[{value:"",label:"(project root)"}]
              .concat(folders.map(x => ({ value:x.id, label:x.name }))) },
          { id:"msg", label:"Version message", value:"Created in SolidCraft web" },
        ], "Create part", null, into);
        if (!f) return null;
        const created = await api("POST", "/projects/" + pid + "/items",
          { part_number: f.pn || null, description: f.desc,
            classification: f.cls, folder_id: f.folder || null });
        const item = created.item;
        const filename = item.part_number + ".sc";
        const version = await scCheckin(item.id, null, null, filename, buffer, f.msg || "Created in SolidCraft web");
        return { item, version, checkin: makeScCheckin(item.id, version, filename) };
      } : null,
      onSaved: () => { render(false); },
    });
  } catch(e){ toast("SolidCraft web: " + e.message, "err", 8000); }
}
function wireNativeActions(root){
  root.querySelectorAll("[data-native]").forEach(b => { b.onclick = () => nativeVersionAction(b.dataset.native,b); });
}
// Dutchman feature-tree conversions can run for many minutes, so the poll is
// generous (15 min); the Tasks view shows live progress meanwhile.
async function waitForVersionJob(id,button){
  for (let i=0;i<900;i++) {
    if (!button.isConnected) return false;
    const {job} = await api("GET","/jobs/"+id);
    if (job.status === "done") return true;
    if (["dead","failed"].includes(job.status)) throw new Error(job.last_error || "Processing failed");
    await new Promise(resolve=>setTimeout(resolve,1000));
  }
  throw new Error("Still processing — check Tasks or click the action again to check progress");
}
async function refreshVersionInfo(itemId,vid){
  const data = await api("GET","/items/"+itemId);
  if (DETAIL.id !== itemId || !$("dtab")) return null;
  DETAIL.data = data;
  const vv = data.versions.find(v=>v.id===vid);
  return vv;
}
// Shared "import to SolidCraft": converts the given version through the
// server's `cad.solidcraft` job (Dutchman for native CAD, with the feature
// tree where it maps; the SolidCraft CLI for geometry-only formats), then
// checks the resulting .sc in as a new immutable version. The source version
// is preserved.
async function importVersionToSolidcraft(itemId, version, button){
  if (!version.derived_solidcraft_sha256){
    if (button) button.textContent = "Converting…";
    const result = await api("POST","/versions/"+version.id+"/solidcraft");
    if (!result.artifact.sha256 && result.job)
      await waitForVersionJob(result.job.id, { isConnected: () => true });
    const data = await api("GET","/items/"+itemId);
    const cur = data.versions.find(v=>v.id===version.id);
    if (!cur || !cur.derived_solidcraft_sha256)
      throw new Error((cur && cur.derived_solidcraft_error) || "The conversion did not produce a native design");
  }
  const meta = await api("GET","/versions/"+version.id+"/solidcraft");
  const item = await api("GET","/items/"+itemId);
  const dup = (item.versions||[]).find(v=>v.sha256 === meta.artifact.sha256);
  if (dup){ toast("Already imported to SolidCraft — see v"+dup.seq,"ok"); return; }
  if (item.lock && item.lock.user_id !== myId())
    throw new Error("checked out by " + lockName(item.lock) + " — undo the checkout and retry");
  if (button) button.textContent = "Checking in…";
  if (!item.lock) await api("POST","/items/"+itemId+"/checkout",{workstation:location.hostname});
  const bytes = await api("GET","/versions/"+version.id+"/solidcraft/blob");
  const up = await api("PUT","/blobs",null,new Uint8Array(bytes));
  const attempt = await api("POST","/items/"+itemId+"/checkin",{
    sha256:up.blob.sha256, size:up.blob.size_bytes,
    base_version: item.head_version ? item.head_version.seq : null,
    commit_message:"Converted v"+version.seq+" to SolidCraft",
    filename:meta.filename, mime_type:"application/octet-stream",
  });
  toast("Imported to SolidCraft — new version v"+attempt.version.seq+" (source version preserved).","ok",7000);
  ALLITEMS = []; ALLITEMS_AT = 0;
  CUR_RELOAD && CUR_RELOAD();
}
// From the version rows / item header (detail view).
async function nativeVersionAction(vid,button){
  const itemId = DETAIL.id;
  const version = DETAIL.data.versions.find(v=>v.id===vid);
  if (!version) return;
  button.disabled = true;
  try {
    await importVersionToSolidcraft(itemId, version, button);
  } catch(e){ toast("Convert to SolidCraft: "+e.message,"err",8000); }
  finally {
    button.disabled=false;
    button.textContent = "Convert to SolidCraft";
  }
}
// From the items list (right-click menu and ⋮): import the item's head version.
async function importItemHead(it){
  try {
    const d = await api("GET", "/items/" + it.id);
    const vv = d.head_version;
    if (!vv) throw new Error("No version to import");
    if (isNativeDesign(vv, d)) { toast("Already a native SolidCraft design","ok"); return; }
    if (!nativeConversionAllowed(vv, d)) throw new Error("This file type does not convert to SolidCraft");
    toast("Importing to SolidCraft via Dutchman — this can take a few minutes…","ok",10000);
    await importVersionToSolidcraft(it.id, vv);
  } catch(e){ toast("Import to SolidCraft: "+e.message,"err",8000); }
}
async function regenerateVersionPreview(vid,button){
  const itemId=DETAIL.id;
  button.disabled=true;button.textContent="Rendering…";
  try {
    const {job}=await api("POST","/versions/"+vid+"/preview");
    if (!await waitForVersionJob(job.id,button)) return;
    const vv=await refreshVersionInfo(itemId,vid);
    if (!vv) return;
    if (vv.thumbnail_error) throw new Error(vv.thumbnail_error);
    if (!vv.thumbnail_sha256) throw new Error("No preview was produced");
    const paths=["/versions/"+vid+"/thumbnail"];
    if (DETAIL.data.head_version.id===vid) paths.push("/items/"+itemId+"/thumbnail");
    for(const path of paths){ const old=THUMB_CACHE.get(path); if(old)URL.revokeObjectURL(old); THUMB_CACHE.delete(path); }
    document.querySelectorAll("[data-thumb]").forEach(el=>{if(paths.includes(el.getAttribute("data-thumb")))el.replaceChildren();});
    await attachThumbs(document);
    if(DETAIL.tab==="preview" && $("pvver") && $("pvver").value===vid) await loadPreview(vid);
    toast("Preview regenerated","ok");
  } catch(e){toast("Preview: "+e.message,"err",8000);}
  finally{button.disabled=false;button.textContent="Regenerate preview";}
}
async function viewDetail(v){
  const id = ROUTE.id;
  if (!PROJECTS.length || !USERS.length) await loadCommon();
  const [d, revs, coms] = await Promise.all([
    api("GET", "/items/" + id),
    api("GET", "/items/" + id + "/revisions").catch(() => ({ revisions: [] })),
    api("GET", "/items/" + id + "/comments").catch(() => ({ comments: [] }))
  ]);
  DETAIL.id = id;
  DETAIL.data = d;
  DETAIL.revs = revs.revisions || [];
  DETAIL.comments = coms.comments || [];
  DETAIL.folderCache = null;
  if (ROUTE._tab){ DETAIL.tab = ROUTE._tab; delete ROUTE._tab; }
  const it = d.item, head = d.head_version, lock = d.lock;
  const proj = PMAP[it.project_id] || {};
  const mine = lock && lock.user_id === myId();
  v.innerHTML =
    "<div class='row' style='margin-bottom:4px'>" +
    "<button id='dback' class='small'>← " + esc(ROUTE.origin === "explorer" ? (proj.name || "Project") : "Back") + "</button>" +
    "</div>" +
    "<div class='dhead'>" +
      "<span class='thumb' data-thumb='/items/" + id + "/thumbnail' style='width:52px;height:52px'></span>" +
      "<div style='min-width:0'>" +
        "<div class='nm'>" + esc(itemName(it)) + " " + revBadge(it.latest_revision) + archBadge(it.archived_at) + "</div>" +
        "<div class='dim'>" + esc(it.part_number) + " · " + esc(proj.name || "") + " · " +
        esc(it.classification || "") + " " + stateBadge(it.lifecycle_state) +
        (lock ? " · <span style='color:var(--warn)'>" + kindSvg("ic-lock") + " " + esc(mine ? "checked out by you" : "checked out by " + lockName(lock)) + "</span>" : "") +
        "</div>" +
      "</div>" +
      "<div class='grow'></div>" +
      (head && solidcraftWebOpenable(head,d) ?
        "<button id='dscweb' class='small' title='Open the head version in the SolidCraft web CAD app'>Edit in SolidCraft</button>" : "") +
      (head ? "<button id='ddownload' class='small'>Download</button>" : "") +
      (head ? nativeAction(head,d) : "") +
      (head && PdmEditors.forFile(head.mime_type || (d.file && d.file.mime_type), d.file && d.file.filename) ?
        "<button id='dwebeditor' class='small'>Open in web editor</button>" : "") +
      "<button id='dco' class='small'" + (lock && !mine ? " disabled" : "") + ">" +
        (lock ? (mine ? "Undo checkout" : "Checked out") : "Check out") + "</button>" +
      "<input type='file' id='dfile' class='hidden'>" +
      "<button id='dnew' class='small primary'>⬆ New version</button>" +
      "<button id='drel' class='small'" + (head ? "" : " disabled") + ">Release revision</button>" +
      "<button id='dkebab' class='kebab-btn vis' title='Item actions'>⋮</button>" +
    "</div>" +
    (DETAIL.conflict ? conflictBannerHtml() : "<div id='dbanner'></div>") +
    "<div class='tabs'>" +
      detailTabs().map(t =>
        "<div class='tab" + (DETAIL.tab === t[0] ? " on" : "") + "' data-t='" + t[0] + "'>" +
        t[1] + (t[2] != null ? " <span class='count'>" + t[2] + "</span>" : "") + "</div>").join("") +
    "</div>" +
    "<div id='dtab'></div>";
  await attachThumbs(v);
  v.querySelectorAll(".tab").forEach(t => t.addEventListener("click", () => {
    DETAIL.tab = t.dataset.t;
    v.querySelectorAll(".tab").forEach(x => x.classList.toggle("on", x === t));
    pushHash();
    renderDetailTab();
  }));
  $("dback").onclick = () => go(ROUTE.origin || "dashboard", { pid: ROUTE._epid || undefined });
  if ($("dscweb")) $("dscweb").onclick = () => openSolidcraftWeb(id);
  if ($("ddownload")) $("ddownload").onclick = () => downloadLatest({ id, filename:d.file && d.file.filename, part_number:it.part_number });
  if ($("dwebeditor")) $("dwebeditor").onclick = () => openWebEditor(id);
  wireNativeActions(v);
  $("dco").onclick = async () => {
    try {
      if (lock) await api("DELETE", "/items/" + id + "/checkout");
      else await api("POST", "/items/" + id + "/checkout", { workstation: location.hostname });
      toast(lock ? "Checkout undone" : "Checked out", "ok");
      CUR_RELOAD && CUR_RELOAD();
    } catch(e){ toast("Checkout: " + e.message, "err"); }
  };
  $("dnew").onclick = () => { $("dfile").click(); };
  $("dfile").onchange = async e => {
    const file = e.target.files[0];
    e.target.value = "";
    if (!file) return;
    const f = await openModal("Check in new version", [
      { id:"msg", label:"Commit message", placeholder:"describe the change" }
    ], "Check in v" + ((head ? head.seq : 0) + 1));
    if (!f) return;
    try {
      const r = await checkinItem(id, file, f.msg || "", file.name);
      toast("Checked in v" + r.version.seq + " (" + bytes(r.version.size_bytes) + ")", "ok");
      DETAIL.conflict = null;
      CUR_RELOAD && CUR_RELOAD();
    } catch(err){
      if (err.conflict){ DETAIL.conflict = err.message; CUR_RELOAD && CUR_RELOAD(); }
      else toast("Check in: " + err.message, "err");
    }
  };
  $("drel").onclick = releaseItem;
  $("dkebab").addEventListener("click", e => openKebabMenu(e, id, "item"));
  const dr = $("drefresh");
  if (dr) dr.onclick = () => { DETAIL.conflict = null; CUR_RELOAD && CUR_RELOAD(); };
  const dd = $("ddismiss");
  if (dd) dd.onclick = () => { DETAIL.conflict = null; render(false); };
  renderDetailTab();
}
function conflictBannerHtml(){
  return "<div class='banner'>⚠ " + esc(DETAIL.conflict) +
    " <button class='small' id='drefresh'>Reload &amp; retry</button>" +
    " <button class='small' id='ddismiss'>Dismiss</button></div>";
}
async function renderDetailTab(){
  const box = $("dtab");
  if (!box) return;
  const t = DETAIL.tab;
  if (t === "overview") await tabOverview(box);
  else if (t === "preview") await tabPreview(box);
  else if (t === "versions") await tabVersions(box);
  else if (t === "revisions") tabRevisions(box);
  else if (t === "comments") tabComments(box);
  else if (t === "activity") await tabActivity(box);
  else if (t === "perm") tabPermissions(box);
  else if (t === "deps") tabSoon(box, bigIcon("ic-file"), "Dependencies",
    "Component / used-by relationships are a PDM backend feature on the roadmap. History, comments and releases for this item are available meanwhile.");
  else if (t === "bom") tabSoon(box, bigIcon("ic-bom"), "Bill of materials",
    "BOM export is a PDM backend feature on the roadmap. This item's classification (" +
    esc(DETAIL.data.item.classification || "") + ") shows under Items.");
  else if (t === "mfg") await tabSheetMetal(box);
}
function tabSoon(box, ic, title, text){
  box.innerHTML = "<div class='soon'><span class='ic'>" + ic + "</span><h3>" +
    esc(title) + "</h3><p>" + esc(text) + "</p></div>";
}

// ── sheet metal tab (MFG): spec + flat-pattern DXF attach/download ─────
async function tabSheetMetal(box){
  const it = DETAIL.data.item;
  const d = await api("GET", "/items/" + it.id + "/sheet-metal");
  const spec = d.sheet_metal_spec || null;
  const mats = await api("GET", "/sheet-materials").catch(() => ({ materials: [] }));
  const matNames = [...new Set((mats.materials || []).map(m => m.material).filter(Boolean))];
  const edit = isRole("editor");
  const specLine = (k, v) => "<div><span class='k'>" + k + "</span> " + (v != null && v !== "" ? esc(String(v)) : "—") + "</div>";
  box.innerHTML =
    "<div class='head'>" +
      "<h2>Sheet metal</h2>" +
      "<span class='dim muted'>" + (spec
        ? "Flagged as a sheet-metal part — it shows under the Items › SHEET METAL filter and can be bulk-exported with flat patterns"
        : "Flag this part as sheet metal to separate it from ordinary 3D models") + "</span>" +
    "</div>" +
    "<div class='grid2'>" +
      "<div class='card'>" +
        "<h3 class='sec'>Spec</h3>" +
        "<div class='dmeta' style='margin:0'>" +
          specLine("Material", spec && spec.material) +
          specLine("Thickness", spec && spec.thickness_mm != null ? spec.thickness_mm + " mm" : null) +
          specLine("Bend radius", spec && spec.bend_radius_mm != null ? spec.bend_radius_mm + " mm" : null) +
          specLine("K-factor", spec && spec.k_factor != null ? spec.k_factor : null) +
        "</div>" +
        (edit ?
          "<div style='margin-top:12px'>" +
            "<div class='row'>" +
              "<input id='sm-mat' list='sm-mat-names' placeholder='Material (e.g. Steel 1.1512)' value='" +
                esc((spec && spec.material) || "") + "' style='flex:1;min-width:160px'>" +
              "<datalist id='sm-mat-names'>" + matNames.map(n => "<option value='" + esc(n) + "'>").join("") + "</datalist>" +
            "</div>" +
            "<div class='row' style='margin-top:6px'>" +
              "<input id='sm-thick' type='number' step='0.01' min='0' placeholder='Thickness mm' value='" +
                (spec && spec.thickness_mm != null ? spec.thickness_mm : "") + "' style='width:110px'>" +
              "<input id='sm-br' type='number' step='0.01' min='0' placeholder='Bend radius mm' value='" +
                (spec && spec.bend_radius_mm != null ? spec.bend_radius_mm : "") + "' style='width:110px'>" +
              "<input id='sm-kf' type='number' step='0.001' placeholder='K-factor' value='" +
                (spec && spec.k_factor != null ? spec.k_factor : "") + "' style='width:90px'>" +
              "<button id='sm-save' class='primary small'>" + (spec ? "Save spec" : "Mark as sheet metal") + "</button>" +
            "</div>" +
          "</div>" : "") +
      "</div>" +
      "<div class='card'>" +
        "<h3 class='sec'>Flat pattern (DXF)</h3>" +
        (spec && spec.flat_pattern_sha
          ? "<p class='dim' style='margin:0 0 8px'>Attached · sha256 " + esc(spec.flat_pattern_sha.slice(0, 16)) + "…</p>" +
            "<div class='row'>" +
              "<button class='primary small' id='sm-dl'>Download flat pattern</button>" +
              (edit ? "<button class='small' id='sm-replace'>Replace…</button>" +
                "<button class='small' id='sm-gen'>Regenerate from 3D model</button>" : "") +
            "</div>"
          : "<p class='dim' style='margin:0 0 8px'>No flat pattern attached yet. For SolidCraft sheet-metal designs the flat pattern is generated from the 3D model; otherwise export it from your CAD system as DXF and attach it." +
            (spec ? "" : " The spec on the left is optional — you can generate or attach a pattern first.") + "</p>" +
            (edit ? "<div class='row'>" +
              "<button class='primary small' id='sm-gen'>Generate from 3D model</button>" +
              "<button class='small' id='sm-attach'>Attach flat pattern (DXF)…</button>" +
            "</div>" : "")) +
        (edit ? "<input type='file' id='sm-file' accept='.dxf,application/dxf' style='display:none'>" : "") +
      "</div>" +
    "</div>";
  const save = $("sm-save");
  if (save) save.onclick = async () => {
    const num = x => x.trim() === "" ? null : Number(x);
    const thick = num($("sm-thick").value), br = num($("sm-br").value), kf = num($("sm-kf").value);
    if ((thick != null && isNaN(thick)) || (br != null && isNaN(br)) || (kf != null && isNaN(kf)))
      return toast("Numeric fields must be numbers", "err");
    try {
      await api("POST", "/items/" + it.id + "/sheet-metal", {
        material: $("sm-mat").value.trim() || null,
        thickness_mm: thick, bend_radius_mm: br, k_factor: kf,
      });
      toast(spec ? "Spec saved" : "Part flagged as sheet metal", "ok");
      DETAIL.data = null;
      await render(false);
    } catch(e){ toast("Save failed: " + e.message, "err"); }
  };
  const dl = $("sm-dl");
  if (dl) dl.onclick = async () => {
    try {
      const buf = await api("GET", "/items/" + it.id + "/flat-pattern");
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([buf], { type: "application/dxf" }));
      a.download = it.part_number + "-flat.dxf";
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 4000);
    } catch(e){ toast("Download failed: " + e.message, "err"); }
  };
  const gen = $("sm-gen");
  if (gen) gen.onclick = async () => {
    gen.disabled = true;
    const prev = gen.textContent;
    gen.textContent = "Generating…";
    try {
      const d = await api("POST", "/items/" + it.id + "/flat-pattern/generate", {});
      let job = d.job;
      for (let i = 0; i < 80; i++){
        await new Promise(r => setTimeout(r, 1500));
        try { job = (await api("GET", "/jobs/" + job.id)).job; } catch(e){ continue; }
        if (job.status === "done" || job.status === "failed" || job.status === "dead") break;
      }
      if (job.status === "done") toast("Flat pattern generated", "ok");
      else toast("Generation " + job.status + ": " + (job.last_error || "timed out — it may still finish in the background"), "err");
      DETAIL.data = null;
      await render(false);
      return;
    } catch(e){ toast("Generate failed: " + e.message, "err"); }
    gen.disabled = false;
    gen.textContent = prev;
  };
  const pick = () => $("sm-file").click();
  const rep = $("sm-replace"); if (rep) rep.onclick = pick;
  const att = $("sm-attach"); if (att) att.onclick = pick;
  const fi = $("sm-file");
  if (fi) fi.onchange = async () => {
    const f = fi.files && fi.files[0];
    if (!f) return;
    fi.value = "";
    try {
      const buf = await f.arrayBuffer();
      await api("POST", "/items/" + it.id + "/flat-pattern", null, new Uint8Array(buf));
      toast("Flat pattern attached", "ok");
      DETAIL.data = null;
      await render(false);
    } catch(e){ toast("Attach failed: " + e.message, "err"); }
  };
}

// ── overview tab ──
async function tabOverview(box){
  const d = DETAIL.data, it = d.item, head = d.head_version;
  const proj = PMAP[it.project_id] || {};
  const [flds, props] = await Promise.all([
    api("GET", "/projects/" + it.project_id + "/folders").catch(() => ({ folders: [] })),
    api("GET", "/items/" + it.id + "/properties").catch(() => ({ properties: [] }))
  ]);
  DETAIL.folderCache = flds.folders || [];
  const list = props.properties || [];
  box.innerHTML =
    "<div class='grid2'>" +
      "<div class='card'>" +
        "<h3 class='dim' style='margin:0 0 10px;font-size:12px;text-transform:uppercase;letter-spacing:.05em'>Metadata</h3>" +
        "<div class='dmeta' style='margin:0'>" +
          "<div><span class='k'>Part no</span> <span class='mono'>" + esc(it.part_number) + "</span></div>" +
          "<div><span class='k'>Project</span> " + esc(proj.name || "—") + "</div>" +
          "<div><span class='k'>Classification</span> " + esc(it.classification || "—") + "</div>" +
          "<div><span class='k'>Lifecycle</span> " + stateBadge(it.lifecycle_state) + "</div>" +
          "<div><span class='k'>Latest revision</span> " +
            (it.latest_revision ? "<span class='rev'>" + esc(it.latest_revision) + "</span>" : "—") + "</div>" +
          "<div><span class='k'>Head version</span> " +
            (head ? "v" + head.seq + " · " + bytes(head.size_bytes) : "—") + "</div>" +
          "<div><span class='k'>File</span> " +
            (d.file ? esc(d.file.filename || "—") + " " + kindBadge(d.file.mime_type, d.file.filename) : "no file yet") + "</div>" +
          "<div><span class='k'>Owner</span> " + esc(it.owner_name || "—") + "</div>" +
          "<div><span class='k'>Created</span> " + fmtDate(it.created_at) + "</div>" +
          "<div><span class='k'>Modified</span> " + fmtTime(it.modified_at) +
            (it.modified_by ? " by " + esc(it.modified_by) : "") + "</div>" +
        "</div>" +
        "<div class='row'><input id='od' value='" + esc(it.description || "") + "' placeholder='description' style='flex:1'></div>" +
        "<div class='row'>" +
          "<select id='ofolder'><option value=''> (no folder)</option>" +
            DETAIL.folderCache.map(f => "<option value='" + f.id + "'" +
              (f.id === it.folder_id ? " selected" : "") + ">" + esc(f.name) + "</option>").join("") +
          "</select>" +
          "<select id='oowner'><option value=''> (keep owner)</option>" +
            USERS.map(u => "<option value='" + u.id + "'" +
              (u.id === it.owner_id ? " selected" : "") + ">" +
              esc(u.display_name || u.email) + "</option>").join("") +
          "</select>" +
          "<button id='osave' class='small'>Save</button>" +
        "</div>" +
      "</div>" +
      "<div class='card'>" +
        "<h3 class='dim' style='margin:0 0 10px;font-size:12px;text-transform:uppercase;letter-spacing:.05em'>Properties</h3>" +
        "<table style='margin:0 0 8px'><tbody>" +
          (list.length ? list.map(p =>
            "<tr><td class='mono muted'>" + esc(p.prop_key || "") + "</td>" +
            "<td>" + esc(typeof p.value === "object" ? JSON.stringify(p.value) : String(p.value != null ? p.value : "")) + "</td>" +
            "<td class='dim'>" + esc(p.value_type || "") + "</td>" +
            (p.computed ? "<td class='dim' title='computed by the server'>⚙</td>" : "<td></td>") +
            "</tr>").join("") :
            "<tr><td class='dim' colspan='3'>No custom properties stored.</td></tr>") +
        "</tbody></table>" +
        "<div class='row'>" +
          "<input id='pk' placeholder='key' style='width:140px'>" +
          "<input id='pv' placeholder='value' style='flex:1;max-width:190px'>" +
          "<select id='pt' style='width:110px'><option value='text'>text</option><option value='number'>number</option></select>" +
          "<button id='padd' class='small'>＋</button>" +
        "</div>" +
      "</div>" +
    "</div>";
  $("osave").onclick = async () => {
    const body = { description: $("od").value };
    const fid = $("ofolder").value, oid = $("oowner").value;
    if (fid) body.folder_id = fid;
    else if (it.folder_id) body.folder_id = null;
    if (oid) body.owner_id = oid;
    try {
      await api("PUT", "/items/" + it.id, body);
      toast("Saved", "ok");
      CUR_RELOAD && CUR_RELOAD();
    } catch(e){ toast("Save: " + e.message, "err"); }
  };
  $("padd").onclick = async () => {
    const k = $("pk").value.trim();
    if (!k){ toast("Property key required", "err"); return; }
    const isNum = $("pt").value === "number";
    const raw = $("pv").value;
    const val = isNum && raw.trim() !== "" ? Number(raw) : raw;
    try {
      await api("PUT", "/items/" + it.id + "/properties/" + encodeURIComponent(k),
        { value: val, value_type: $("pt").value });
      toast("Property saved", "ok");
      CUR_RELOAD && CUR_RELOAD();
    } catch(e){ toast("Property: " + e.message, "err"); }
  };
}

// ── preview tab (rendered PNG with pan/zoom; 2D for image/PDF/DXF) ───────
let PZV = null;
let DXF_PREVIEW_DISPOSE = null;
function setupPanzoom(port, img, hint){
  PZV = { port: port, img: img, s: 1, x: 0, y: 0 };
  const zlab = $("pvzlab");
  function applyPZ(){
    img.style.transform = "translate(" + PZV.x + "px," + PZV.y + "px) scale(" + PZV.s + ")";
    if (zlab) zlab.textContent = Math.round(PZV.s * 100) + "%";
  }
  PZV.apply = applyPZ;
  PZV.zoomAt = (px, py, f) => {
    const ns = Math.min(12, Math.max(0.05, PZV.s * f));
    PZV.x = px - (px - PZV.x) * (ns / PZV.s);
    PZV.y = py - (py - PZV.y) * (ns / PZV.s);
    PZV.s = ns;
    applyPZ();
  };
  PZV.fit = () => {
    const r = port.getBoundingClientRect();
    const iw = img.naturalWidth || 1, ih = img.naturalHeight || 1;
    PZV.s = Math.min(r.width * 0.95 / iw, r.height * 0.95 / ih, 2);
    PZV.x = 0; PZV.y = 0;
    applyPZ();
  };
  PZV.one = () => { PZV.s = 1; PZV.x = 0; PZV.y = 0; applyPZ(); };
  applyPZ();
  img.onload = () => PZV && PZV.fit();
  if (img.complete && img.naturalWidth) PZV.fit();
  setTimeout(() => PZV && PZV.fit(), 150);
  port.onwheel = e => {
    e.preventDefault();
    if (!PZV) return;
    const r = port.getBoundingClientRect();
    PZV.zoomAt(e.clientX - r.left - r.width / 2, e.clientY - r.top - r.height / 2,
      e.deltaY < 0 ? 1.2 : 1 / 1.2);
  };
  let drag = null;
  port.onpointerdown = e => {
    if (e.button !== 0) return;
    drag = { x: e.clientX, y: e.clientY };
    try { port.setPointerCapture(e.pointerId); } catch(x){}
  };
  port.onpointermove = e => {
    if (!drag || !PZV) return;
    PZV.x += e.clientX - drag.x; PZV.y += e.clientY - drag.y;
    drag = { x: e.clientX, y: e.clientY };
    PZV.apply();
  };
  port.onpointerup = port.onpointercancel = () => { drag = null; };
  const zin = $("pvzin"), zout = $("pvzout"), fitb = $("pvfit"), o100 = $("pv100");
  if (zin) zin.onclick = () => PZV && PZV.zoomAt(0, 0, 1.25);
  if (zout) zout.onclick = () => PZV && PZV.zoomAt(0, 0, 0.8);
  if (fitb) fitb.onclick = () => PZV && PZV.fit();
  if (o100) o100.onclick = () => PZV && PZV.one();
  if (hint) hint.textContent = "Rendered preview of this version. Drag to pan · scroll to zoom. Use Regenerate preview to apply the current renderer style.";
}
async function tabPreview(box){
  const d = DETAIL.data, head = d.head_version;
  if (!head){
    box.innerHTML = "<div class='pv-empty'><span class='big'>◻</span><div>No versions yet.</div>" +
      "<div class='pv-hint'>Use “⬆ New version” in the header to check in a file.</div></div>";
    return;
  }
  box.innerHTML =
    "<div class='row' style='margin-bottom:8px'>" +
      "<label class='dim' for='pvver'>preview version</label>" +
      "<select id='pvver' style='max-width:380px'>" +
        (d.versions || []).map(vv =>
          "<option value='" + vv.id + "'" + (vv.id === head.id ? " selected" : "") + ">v" + vv.seq +
          " — " + esc(vv.author_name || "?") + " · " + fmtTime(vv.committed_at) +
          (["PDF", "DXF"].includes(kindInfo(vv.mime_type, d.file && d.file.filename).label) ? "" :
            vv.thumbnail_sha256 ? "" : vv.thumbnail_error ? " · no preview" : " · no image yet") + "</option>").join("") +
      "</select>" +
    "</div>" +
    "<div class='pv-wrap'>" +
      "<div class='pv-vport' id='pvport'>" +
        "<div class='pv-canvas' id='pvcanvas'><div class='pv-empty' style='height:100%'><span class='big spinner'></span></div></div>" +
      "</div>" +
      "<div class='pv-side'>" +
        "<div class='pv-ctl'>" +
          "<button id='pvzin' title='Zoom in'>＋</button>" +
          "<button id='pvzout' title='Zoom out'>−</button>" +
          "<span class='zlabel' id='pvzlab'>…</span>" +
          "<button id='pvfit' title='Fit to view'>Fit</button>" +
          "<button id='pv100' title='Actual size'>100%</button>" +
        "</div>" +
        "<div class='pv-hint' id='pvhint'></div>" +
        "<button id='pvdwn' style='width:100%'>⬇ Download file</button>" +
        "<div id='pvscweb'></div>" +
        "<div id='pvnative'></div>" +
        (isRole("editor") ? "<button id='pvregen' style='width:100%'>Regenerate preview</button>" : "") +
        (PdmEditors.forFile(head.mime_type || (d.file && d.file.mime_type), d.file && d.file.filename) ?
          "<button id='pvwebeditor' style='width:100%'>Open in web editor</button>" : "") +
      "</div>" +
    "</div>";
  $("pvver").onchange = e => loadPreview(e.target.value);
  if ($("pvwebeditor")) $("pvwebeditor").onclick = () => openWebEditor(d.item.id, $("pvver").value);
  $("pvdwn").onclick = async () => {
    const cur = $("pvver") ? $("pvver").value : "";
    const vv = (d.versions || []).find(x => x.id === cur) || head;
    try {
      const buf = await api("GET", "/versions/" + vv.id + "/blob");
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([buf]));
      a.download = versionDownloadName(vv,d);
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 4000);
    } catch(e2){ toast("Download: " + e2.message, "err"); }
  };
  await loadPreview($("pvver").value);
}
async function loadPreview(vid){
  if (DXF_PREVIEW_DISPOSE){ DXF_PREVIEW_DISPOSE(); DXF_PREVIEW_DISPOSE = null; }
  PZV = null;
  const canvas = $("pvcanvas"), port = $("pvport"), hint = $("pvhint");
  if (!canvas) return;
  const d = DETAIL.data;
  const vv = (d.versions || []).find(x => x.id === vid) || d.head_version;
  if (!vv){
    canvas.innerHTML = "<div class='pv-empty' style='height:100%'><span class='big'>◻</span><div>No version selected.</div></div>";
    return;
  }
  canvas.innerHTML = "<div class='pv-empty' style='height:100%'><span class='big spinner'></span><div>loading preview…</div></div>";
  if (hint) hint.textContent = "";
  const fn = sourceFilename(vv,d) || d.item.filename || "";
  if ($("pvnative")){ $("pvnative").innerHTML=nativeAction(vv,d); wireNativeActions($("pvnative")); }
  const scweb = $("pvscweb");
  if (scweb){
    scweb.innerHTML = solidcraftWebOpenable(vv,d)
      ? "<button id='pvscwebbtn' style='width:100%' title='Open this version in the SolidCraft web CAD app; Save to PDM checks the edited design in as a new version'>✎ Edit in SolidCraft web</button>"
      : "";
    const scb = $("pvscwebbtn");
    if (scb) scb.onclick = () => openSolidcraftWeb(d.item.id, vv.id);
  }
  if ($("pvregen")) {
    $("pvregen").classList.toggle("hidden",!serverPreviewAllowed(vv,d));
    $("pvregen").onclick=()=>regenerateVersionPreview(vv.id,$("pvregen"));
  }
  const k = kindInfo(vv.mime_type, fn);
  const controls = port.parentElement.querySelector(".pv-ctl");
  if (controls) controls.classList.toggle("hidden", k.label === "DXF" || k.label === "PDF");
  // Remove handlers from an image preview before mounting an interactive viewer.
  port.onwheel = port.onpointerdown = port.onpointermove = port.onpointerup = port.onpointercancel = null;
  try {
    if (k.cls === "img"){
      const buf = await api("GET", "/versions/" + vv.id + "/blob");
      const url = URL.createObjectURL(new Blob([buf], { type: vv.mime_type || "image/png" }));
      canvas.innerHTML = "<img id='pvimg' src='" + url + "' alt='preview'>";
      if (hint) hint.textContent = "Image artifact — drag to pan, scroll to zoom.";
      setupPanzoom(port, $("pvimg"), hint);
    } else if (k.label === "PDF"){
      const buf = await api("GET", "/versions/" + vv.id + "/blob");
      const url = URL.createObjectURL(new Blob([buf], { type: "application/pdf" }));
      canvas.innerHTML = "<iframe src='" + url + "' title='pdf preview'></iframe>";
      if (hint) hint.textContent = "PDF preview — use the embedded viewer's own pan/zoom.";
    } else if (k.label === "DXF"){
      DXF_PREVIEW_DISPOSE = PdmEditors.preview({ container:canvas,
        name:fn || d.item.part_number + ".dxf",
        load: async signal => {
          const r = await fetch("/versions/" + vv.id + "/blob", {
            headers:{ Authorization:"Bearer " + TOKEN }, signal,
          });
          if (!r.ok) throw new Error("Could not load v" + vv.seq + " (HTTP " + r.status + ")");
          return r.arrayBuffer();
        },
        onStatus: message => { if (hint) hint.textContent = message; },
      });
    } else {
      let url = null;
      if (vv.thumbnail_sha256){
        const r = await fetch("/versions/" + vv.id + "/thumbnail",
          { cache:"no-store", headers: { Authorization: "Bearer " + TOKEN } });
        if (r.ok) url = URL.createObjectURL(await r.blob());
      }
      if (url){
        canvas.innerHTML = "<img id='pvimg' class='model-preview' src='" + url + "' alt='rendered preview'>";
        setupPanzoom(port, $("pvimg"), hint);
      } else if (vv.thumbnail_error){
        canvas.innerHTML = "<div class='pv-empty' style='height:100%'><span class='big'>!</span>" +
          "<div>No rendered preview for v" + vv.seq + ".</div>" +
          "<div class='pv-hint'>" + esc(vv.thumbnail_error) + "</div>" +
          "<div class='pv-hint'>You can still download the file (⬇).</div></div>";
      } else {
        canvas.innerHTML = "<div class='pv-empty' style='height:100%'><span class='big'>⬢</span>" +
          "<div>No rendered preview for v" + vv.seq + " yet.</div>" +
          "<div class='pv-hint'>The server renders previews for CAD files (designs, STEP/IGES, FreeCAD, " +
          "Rhino, Inventor, meshes; other B-rep CAD via conversion). See Tasks &amp; Approvals for job status.</div></div>";
      }
    }
  } catch(e){
    canvas.innerHTML = "<div class='pv-empty' style='height:100%'><span class='big'>!</span>" +
      "<div>Preview failed: " + esc(e.message) + "</div>" +
      "<div class='pv-hint'>You can still download the file (⬇).</div></div>";
  }
}

// ── versions tab ──
async function tabVersions(box){
  const d = DETAIL.data, vs = d.versions || [], head = d.head_version;
  if (!vs.length){
    box.innerHTML = "<div class='empty'>No versions yet — check in a file to create v1.</div>";
    return;
  }
  box.innerHTML =
    "<div id='cmpbox'></div>" +
    "<table><thead><tr>" +
    "<th style='width:30px'></th><th>Ver</th><th>Type</th><th>Size</th><th>SHA-256</th>" +
    "<th>Author</th><th>Committed</th><th>Message</th><th></th>" +
    "</tr></thead><tbody>" +
    vs.map(vv =>
      "<tr>" +
      "<td><input type='checkbox' data-cmp='" + vv.id + "'" +
        (DETAIL.cmp.has(vv.id) ? " checked" : "") + " title='select two versions to compare'></td>" +
      "<td><span style='display:inline-flex;align-items:center;gap:6px'>" +
        "<span class='thumb-sm" +
        ((!vv.thumbnail_sha256 && vv.thumbnail_error) ? " thumb-err" : "") + "'" +
        ((!vv.thumbnail_sha256 && vv.thumbnail_error) ? " title='No preview: " + esc(vv.thumbnail_error) + "'" : "") +
        " data-thumb='" +
        (vv.thumbnail_sha256 ? "/versions/" + vv.id + "/thumbnail" : "") + "'></span>" +
        "<b>v" + vv.seq + "</b>" +
        (head && head.id === vv.id ? " <span class='badge'>head</span>" : "") + "</span></td>" +
      "<td>" + kindBadge(vv.mime_type, sourceFilename(vv,d) || vv.native_format) + "</td>" +
      "<td class='muted'>" + bytes(vv.size_bytes) + "</td>" +
      "<td class='mono muted' title='" + esc(vv.sha256 || "") + "'>" + shaShort(vv.sha256) + "</td>" +
      "<td class='muted'>" + esc(vv.author_name || "—") + "</td>" +
      "<td class='muted' title='" + fmtDate(vv.committed_at) + "'>" + fmtTime(vv.committed_at) + "</td>" +
      "<td class='muted' style='max-width:220px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap' " +
        "title='" + esc(vv.commit_message || "") + "'>" + esc(vv.commit_message || "") + "</td>" +
      "<td style='white-space:nowrap'>" +
        "<button class='small' data-dl='" + vv.id + "' title='Download this version'>⬇</button>" +
        nativeAction(vv,d) +
        (PdmEditors.forFile(vv.mime_type, d.file && d.file.filename) ?
          "<button class='small' data-web='" + vv.id + "'>Open in web editor</button>" : "") +
        (head && head.id !== vv.id ?
          "<button class='small' data-rs='" + vv.id + "' title='Restore this version as a new head'>↺ restore</button>" : "") +
      "</td></tr>").join("") +
    "</tbody></table>" +
    "<div class='dim' style='font-size:12px;margin-top:8px'>Tick two versions to compare metadata. " +
    "Versions are immutable — restoring never overwrites history.</div>";
  await attachThumbs(box);
  box.querySelectorAll("[data-cmp]").forEach(cb => cb.addEventListener("change", () => {
    if (cb.checked) DETAIL.cmp.add(cb.dataset.cmp);
    else DETAIL.cmp.delete(cb.dataset.cmp);
    if (DETAIL.cmp.size > 2) DETAIL.cmp.delete([...DETAIL.cmp][0]);
    updateCompare(box, vs);
  }));
  box.querySelectorAll("[data-dl]").forEach(b => b.addEventListener("click", async () => {
    const vv = vs.find(x => x.id === b.dataset.dl);
    if (!vv) return;
    try {
      const buf = await api("GET", "/versions/" + vv.id + "/blob");
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([buf]));
      a.download = versionDownloadName(vv,d);
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 4000);
    } catch(e){ toast("Download: " + e.message, "err"); }
  }));
  box.querySelectorAll("[data-rs]").forEach(b => b.addEventListener("click", () => {
    const vv = vs.find(x => x.id === b.dataset.rs);
    if (vv) restoreVersion(vv);
  }));
  box.querySelectorAll("[data-web]").forEach(b => b.addEventListener("click", () => openWebEditor(d.item.id, b.dataset.web)));
  wireNativeActions(box);
  updateCompare(box, vs);
}
function updateCompare(box, vs){
  const cbox = box.querySelector("#cmpbox");
  if (!cbox) return;
  const ids = [...DETAIL.cmp];
  if (ids.length !== 2){ cbox.innerHTML = ""; return; }
  const a = vs.find(x => x.id === ids[0]), b = vs.find(x => x.id === ids[1]);
  if (!a || !b){ cbox.innerHTML = ""; return; }
  const row = (name, va, vb) => {
    const diff = String(va != null ? va : "") !== String(vb != null ? vb : "");
    return "<tr" + (diff ? " style='background:var(--active)'" : "") + "><td class='dim'>" + name +
      "</td><td>" + esc(String(va != null ? va : "")) + "</td><td>" +
      esc(String(vb != null ? vb : "")) + "</td></tr>";
  };
  const ka = kindInfo(a.mime_type, ""), kb = kindInfo(b.mime_type, "");
  const typeRow = "<tr" + (ka.label !== kb.label ? " style='background:var(--active)'" : "") + "><td class='dim'>type</td>" +
    "<td>" + ka.icon + " " + esc(ka.label) + "</td><td>" + kb.icon + " " + esc(kb.label) + "</td></tr>";
  cbox.innerHTML =
    "<div class='card' style='margin-bottom:10px'>" +
    "<b style='font-size:13px'>Comparing v" + a.seq + " ↔ v" + b.seq + "</b>" +
    "<table style='margin-top:8px'><thead><tr><th></th><th>v" + a.seq + "</th><th>v" + b.seq + "</th></tr></thead><tbody>" +
    typeRow +
    row("size", bytes(a.size_bytes), bytes(b.size_bytes)) +
    row("sha-256", a.sha256 || "", b.sha256 || "") +
    row("author", a.author_name || "", b.author_name || "") +
    row("committed", fmtDate(a.committed_at), fmtDate(b.committed_at)) +
    row("message", a.commit_message || "", b.commit_message || "") +
    "</tbody></table></div>";
}
async function restoreVersion(vv){
  const id = DETAIL.id;
  const f = await openModal("Restore v" + vv.seq + " as a new version", [
    { id:"msg", label:"Commit message",
      value: "Restore of v" + vv.seq + (vv.commit_message ? " (" + vv.commit_message + ")" : "") }
  ], "Restore");
  if (!f) return;
  try {
    const cur = await api("GET", "/items/" + id);
    if (cur.lock && cur.lock.user_id !== myId()){
      toast("Restore: " + lockName(cur.lock) + " has the item checked out", "err");
      return;
    }
    if (!cur.lock)
      await api("POST", "/items/" + id + "/checkout", { workstation: location.hostname });
    const d2 = await api("POST", "/items/" + id + "/checkin", {
      sha256: vv.sha256, size: vv.size_bytes,
      base_version: cur.head_version ? cur.head_version.seq : null,
      commit_message: f.msg || "",
      filename: (cur.file && cur.file.filename) || null,
      mime_type: vv.mime_type || null,
    });
    toast("Restored v" + vv.seq + " as v" + d2.version.seq, "ok");
    DETAIL.cmp.clear();
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){
    if (e.status === 409){ DETAIL.conflict = "v" + vv.seq + " could not be restored — " + e.message + " Reload to rebase, then try again."; CUR_RELOAD && CUR_RELOAD(); }
    else toast("Restore: " + e.message, "err");
  }
}

// ── revisions tab / release ──
function autoLabel(n){
  if (n < 26) return String.fromCharCode(65 + n);
  let m = n, s = "";
  while (m > 0){ m -= 1; s = String.fromCharCode(65 + (m % 26)) + s; m = Math.floor(m / 26); }
  return s;
}
async function releaseItem(){
  const id = DETAIL.id;
  const head = DETAIL.data.head_version;
  if (!head){ toast("Check in a version first", "err"); return; }
  const f = await openModal("Release revision", [
    { id:"label", label:"Revision label", value: autoLabel(DETAIL.revs.length) },
    { id:"note", label:"Release note", placeholder:"optional" }
  ], "Release v" + head.seq);
  if (!f) return;
  try {
    const r = await api("POST", "/items/" + id + "/release",
      { label: f.label || null, note: f.note || null });
    if (r.status === "pending_approval"){
      toast("Release requested — routed for approval", "ok");
      DETAIL.tab = "overview";
    } else {
      toast("Released as revision " + (r.revision ? r.revision.label : ""), "ok");
      DETAIL.tab = "revisions";
    }
    refreshApprovalsBadge();
    CUR_RELOAD && CUR_RELOAD();
  } catch(e){ toast("Release: " + e.message, "err"); }
}
function tabRevisions(box){
  const revs = DETAIL.revs, vs = DETAIL.data.versions || [];
  const byId = {}; vs.forEach(v => byId[v.id] = v);
  box.innerHTML = revs.length ?
    "<table><thead><tr><th>Rev</th><th>Scheme</th><th>Version</th><th>Type</th><th>Released by</th><th>When</th><th>Note</th></tr></thead><tbody>" +
    revs.map(r => {
      const vv = byId[r.version_id];
      return "<tr>" +
      "<td><span class='rev'>" + esc(r.label) + "</span></td>" +
      "<td class='muted'>" + esc(r.scheme || "") + "</td>" +
      "<td>" + (vv ? "v" + vv.seq : "—") + "</td>" +
      "<td>" + (vv ? kindBadge(vv.mime_type, vv.native_format || sourceFilename(vv, DETAIL.data)) : "—") + "</td>" +
      "<td class='muted'>" + esc((UMAP[r.released_by] && (UMAP[r.released_by].display_name || UMAP[r.released_by].email)) || "—") + "</td>" +
      "<td class='muted' title='" + fmtDate(r.released_at) + "'>" + fmtTime(r.released_at) + "</td>" +
      "<td class='muted'>" + esc(r.release_note || "") + "</td></tr>";
    }).join("") +
    "</tbody></table>" +
    "<div class='dim' style='font-size:12px;margin-top:8px'>Revisions are named, immutable release pointers — " +
    "historical versions are never overwritten.</div>" :
    "<div class='empty'>No named revisions yet — use “Release revision” in the header to tag the current head " +
    "(labels auto-increment A, B, C…).</div>";
}

// ── comments / activity / permissions tabs ──
function tabComments(box){
  const cs = DETAIL.comments;
  box.innerHTML =
    "<div class='comments'>" +
    (cs.length ? cs.map(c => "<div class='comment'>" +
      "<div class='meta'><b>" + esc(c.author_name || "?") + "</b> · " + fmtTime(c.created_at) + "</div>" +
      esc(c.body) + "</div>").join("") :
      "<div class='empty'>No comments yet — start the discussion.</div>") +
    "</div>" +
    "<div class='comment-input'>" +
      "<textarea id='comtxt' placeholder='Add a comment…'></textarea>" +
      "<button id='compost' class='primary'>Post</button>" +
    "</div>";
  $("compost").onclick = async () => {
    const t = $("comtxt").value.trim();
    if (!t){ toast("Comment is empty", "err"); return; }
    try {
      await api("POST", "/items/" + DETAIL.id + "/comments", { body: t });
      toast("Comment posted", "ok");
      CUR_RELOAD && CUR_RELOAD();
    } catch(e){ toast("Comment: " + e.message, "err"); }
  };
  $("comtxt").addEventListener("keydown", e => {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") $("compost").click();
  });
}
async function tabActivity(box){
  const d = await api("GET", "/audit?item=" + DETAIL.id + "&limit=50").catch(() => ({ events: [] }));
  const evs = d.events || [];
  box.innerHTML = evs.length ?
    "<table style='max-width:860px'><thead><tr><th>When</th><th>Actor</th><th>Action</th><th>Detail</th></tr></thead><tbody>" +
    evs.map(ev => "<tr>" +
      "<td class='muted' title='" + fmtDate(ev.at) + "'>" + fmtTime(ev.at) + "</td>" +
      "<td class='muted'>" + esc(actorName(ev)) + "</td>" +
      "<td class='mono'>" + esc(ev.action) + "</td>" +
      "<td class='dim'>" + (ev.detail ? esc(JSON.stringify(ev.detail)) : "") + "</td></tr>").join("") +
    "</tbody></table>" :
    "<div class='empty'>No activity recorded for this item yet.</div>";
}
function tabPermissions(box){
  const it = DETAIL.data.item, lock = DETAIL.data.lock;
  box.innerHTML =
    "<div class='grid2'>" +
      "<div class='card'>" +
        "<h3 class='dim' style='margin:0 0 10px;font-size:12px;text-transform:uppercase;letter-spacing:.05em'>Access</h3>" +
        "<div class='dmeta' style='margin:0'>" +
          "<div><span class='k'>Owner</span> " + esc(it.owner_name || "—") + "</div>" +
          "<div><span class='k'>Checkout</span> " +
            (lock ? esc(lockName(lock)) + " on " + esc(lock.workstation || "?") : "none") + "</div>" +
          "<div><span class='k'>Your role</span> " +
            esc(ME && ME.user ? ME.user.role : "?") + "</div>" +
        "</div>" +
        "<div class='dim' style='font-size:12.5px;line-height:1.7'>" +
          "<b>viewer</b> — browse, preview, download<br>" +
          "<b>editor</b> — check out / check in, comment, release<br>" +
          "<b>admin</b> — plus user, token and settings management" +
        "</div>" +
      "</div>" +
      "<div class='card'>" +
        "<h3 class='dim' style='margin:0 0 10px;font-size:12px;text-transform:uppercase;letter-spacing:.05em'>Members</h3>" +
        "<table style='margin:0'><tbody>" +
        USERS.map(u => "<tr>" +
          "<td><span style='display:inline-flex;gap:8px;align-items:center'>" +
          userAv(u, !u.active) + "<span>" + esc(u.display_name || u.email) + "</span></span></td>" +
          "<td class='muted'>" + esc(u.role) + "</td>" +
          "<td class='muted'>" + (u.active ? "active" : "inactive") + "</td></tr>").join("") +
        "</tbody></table>" +
      "</div>" +
    "</div>";
}

// Full view renders replace controls, previews and scroll state. Keep timer-
// driven renders opt-in; explicit refresh and updates after an action still work.
function refreshModeLabel(){
  return AUTO_REFRESH ? "Auto-refresh every " + POLL_MS / 1000 + "s" : "Manual refresh";
}
function startPolling(){
  setInterval(() => {
    if (!AUTO_REFRESH || !ME || document.hidden || AUTH_BUSY) return;
    // Even when opted in, do not rebuild editing/settings pages or an open
    // editor, menu or dialog. A field may contain a draft after it loses focus.
    if (["detail", "settings", "admin"].includes(ROUTE.name)) return;
    if (document.querySelector(".modal-backdrop, dialog[open], .ctxmenu:not(.hidden), .kebabmenu:not(.hidden)")) return;
    const ae = document.activeElement;
    if (ae && ae !== document.body &&
      (ae.isContentEditable || ["INPUT", "TEXTAREA", "SELECT", "IFRAME"].includes(ae.tagName))) return;
    if (CUR_RELOAD) Promise.resolve(CUR_RELOAD()).catch(() => {});
    refreshApprovalsBadge();
  }, POLL_MS);
}

// ── init ────────────────────────────────────────────────────────────────
(function init(){
  applyTheme(currentTheme());
  $("themebtn").onclick = () => {
    const i = THEMES.findIndex(t => t.id === currentTheme());
    applyTheme(THEMES[(i + 1) % THEMES.length].id);
  };
  document.querySelectorAll(".sb-link, .bb-link").forEach(a =>
    a.addEventListener("click", () => {
      $("sidebar").classList.remove("open");
      go(a.dataset.nav);
    }));
  window.addEventListener("popstate", onHashNav);
  window.addEventListener("hashchange", onHashNav);
  const dens = localStorage.getItem("pdm_density");
  if (dens === "compact") document.documentElement.classList.add("dense");
  const densityBtn = $("densitybtn");
  if (densityBtn){
    densityBtn.textContent = dens === "compact" ? "Compact" : "Comfortable";
    densityBtn.onclick = () => {
      const on = document.documentElement.classList.toggle("dense");
      localStorage.setItem("pdm_density", on ? "compact" : "comfortable");
      densityBtn.textContent = on ? "Compact" : "Comfortable";
    };
  }
  $("menubtn").onclick = () => $("sidebar").classList.toggle("open");
  $("refreshbtn").onclick = () => (CUR_RELOAD ? CUR_RELOAD() : render(true));
  const autoRefresh = document.createElement("button");
  autoRefresh.id = "autorefreshbtn";
  autoRefresh.className = "small";
  autoRefresh.title = "Automatically refresh list views every six seconds. Editing pages refresh manually.";
  const updateRefreshMode = () => {
    autoRefresh.textContent = "Auto-refresh: " + (AUTO_REFRESH ? "On" : "Off");
    autoRefresh.setAttribute("aria-pressed", String(AUTO_REFRESH));
    const status = $("refreshstatus");
    if (status) status.textContent = refreshModeLabel();
  };
  autoRefresh.onclick = () => {
    AUTO_REFRESH = !AUTO_REFRESH;
    localStorage.setItem("pdm_auto_refresh", String(AUTO_REFRESH));
    updateRefreshMode();
  };
  $("refreshbtn").after(autoRefresh);
  updateRefreshMode();
  $("signout").onclick = signout;
  $("userkebab").onclick = showUserMenu;
  $("signin").onclick = signin;
  $("connect").onclick = connectFromToken;
  $("tokcopy").onclick = async () => {
    try { await navigator.clipboard.writeText(TOKEN); toast("Token copied", "ok"); }
    catch(e){ toast("Copy failed", "err"); }
  };
  $("login_password").addEventListener("keydown", e => { if (e.key === "Enter") signin(); });
  $("token").addEventListener("keydown", e => { if (e.key === "Enter") connectFromToken(); });
  $("globsearch").addEventListener("keydown", e => {
    if (e.key === "Enter"){
      const q = e.target.value.trim();
      if (q){ e.target.value = ""; go("search", { q }); }
    }
  });
  document.addEventListener("keydown", e => {
    if (e.key === "/" && !e.ctrlKey && !e.metaKey){
      const ae = document.activeElement;
      if (ae && (ae.tagName === "INPUT" || ae.tagName === "TEXTAREA")) return;
      e.preventDefault();
      const g = $("globsearch");
      if (g && !g.classList.contains("hidden")) g.focus();
    }
  });
  api("GET", "/version").then(d => {
    const parts = [d.version ? "v" + d.version : null, d.build, d.commit,
      d.built_at ? "built " + d.built_at : null].filter(Boolean);
    if (parts.length) $("authbuild").textContent = parts.join(" · ");
  }).catch(() => {});
  startPolling();
  if (TOKEN) connectFromToken();
  else $("login-view").classList.remove("hidden");
})();
})();
