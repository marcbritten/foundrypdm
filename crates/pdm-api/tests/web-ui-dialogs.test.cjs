// Dialogs, errors, toasts and item action menus.
// Run with: node --test crates/pdm-api/tests/web-ui-dialogs.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const app = fs.readFileSync(path.join(__dirname, "../static/app.js"), "utf8").replace(/\r\n/g, "\n");

function loadFunctions(names, globals){
  const context = vm.createContext(globals);
  for (const name of names){
    const start = app.indexOf("function " + name + "(");
    assert(start >= 0, "missing function " + name);
    const asyncStart = app.slice(start - 6, start) === "async " ? start - 6 : start;
    const rest = app.slice(start);
    const end = rest.search(/\n(?:async )?function /);
    vm.runInContext(app.slice(asyncStart, end < 0 ? app.length : start + end), context);
  }
  return context;
}
// A top-level `const NAME = {...};` block from app.js.
function constBlock(name){
  const start = app.indexOf("const " + name + " = ");
  assert(start >= 0, "missing const " + name);
  return app.slice(start, app.indexOf("\n};", start) + 3);
}
const tick = () => new Promise(r => setImmediate(r));
// Values made inside the vm context have its own prototypes; compare them as plain data.
const plain = x => JSON.parse(JSON.stringify(x));

// ── a small DOM stand-in: enough for openModal and toast ─────────────────
class El {
  constructor(tag, doc){ this.tagName = tag.toUpperCase(); this.doc = doc; this.children = []; this.parentNode = null;
    this.attrs = {}; this.dataset = {}; this.listeners = {}; this.textContent = ""; this.className = "";
    this.hidden = false; this.disabled = false; this.value = ""; this.style = {}; }
  appendChild(c){ if (c.parentNode) c.remove(); c.parentNode = this; this.children.push(c);
    if (c.tagName === "OPTION" && this.value === "") this.value = c.value; return c; }
  remove(){ if (this.parentNode){ const p = this.parentNode; p.children = p.children.filter(x => x !== this); this.parentNode = null; } }
  get firstElementChild(){ return this.children[0] || null; }
  get isConnected(){ let n = this; while (n.parentNode) n = n.parentNode; return n === this.doc.body; }
  setAttribute(k, v){ this.attrs[k] = String(v); if (k === "id") this.id = String(v); }
  getAttribute(k){ return k in this.attrs ? this.attrs[k] : null; }
  removeAttribute(k){ delete this.attrs[k]; }
  addEventListener(t, f){ (this.listeners[t] = this.listeners[t] || []).push(f); }
  removeEventListener(t, f){ this.listeners[t] = (this.listeners[t] || []).filter(x => x !== f); }
  dispatch(t, e){ for (const f of this.listeners[t] || []) f(e || { preventDefault(){} }); }
  focus(){ this.doc.activeElement = this; }
  contains(n){ for (; n; n = n.parentNode) if (n === this) return true; return false; }
  closest(){ return null; }
  set innerHTML(s){ this.html = s; }
  get innerHTML(){ return this.html || ""; }
  reportValidity(){ return this.all().filter(e => e.required).every(e => e.value !== ""); }
  matches(sel){
    return sel.split(",").some(s => { s = s.trim();
      const m = s.match(/^([a-z]*)(\[role=(\w+)\])?$/i);
      if (!m) return false;
      return (!m[1] || this.tagName === m[1].toUpperCase()) && (!m[3] || this.attrs.role === m[3]); });
  }
  all(){ return this.children.flatMap(c => [c, ...c.all()]); }
  querySelectorAll(sel){ return this.all().filter(e => e.matches(sel)); }
  querySelector(sel){ return this.querySelectorAll(sel)[0] || null; }
}
function fakeDocument(){
  const doc = { listeners: {}, activeElement: null };
  doc.body = new El("body", doc);
  doc.createElement = tag => new El(tag, doc);
  doc.addEventListener = (t, f) => { (doc.listeners[t] = doc.listeners[t] || []).push(f); };
  doc.removeEventListener = (t, f) => { doc.listeners[t] = (doc.listeners[t] || []).filter(x => x !== f); };
  doc.querySelectorAll = () => [];
  doc.querySelector = sel => sel === ".modal-backdrop:last-of-type"
    ? doc.body.children.filter(c => c.className === "modal-backdrop").pop() || null : null;
  doc.key = key => { for (const f of doc.listeners.keydown || []) f({ key }); };
  return doc;
}
function modalHarness(extra){
  const document = fakeDocument();
  const opener = document.createElement("button"); document.body.appendChild(opener); opener.focus();
  const toasts = [];
  const ctx = loadFunctions(["openModal", "placeToasts"], Object.assign({ document, $: () => null,
    toast: (m, k) => toasts.push([m, k]), JSON, Math, String, Promise }, extra));
  const dialog = () => document.body.children.find(c => c.className === "modal-backdrop");
  const button = text => dialog().all().find(e => e.tagName === "BUTTON" && e.textContent === text);
  const input = id => dialog().all().find(e => e.id && e.id.endsWith("-" + id));
  return { ctx, document, opener, toasts, dialog, button, input };
}

// ── openModal ────────────────────────────────────────────────────────────
test("openModal with onSubmit stays open while busy, ignores a second submit and shows the server's error inside", async () => {
  const h = modalHarness();
  const calls = [];
  let release;
  const p = h.ctx.openModal("New machine", [{ id:"name", label:"Name", required:true }],
    { label:"Add machine", busyLabel:"Adding…", onSubmit: v => { calls.push(v.name);
      return new Promise((res, rej) => { release = { res, rej }; }); } });
  const name = h.input("name");
  assert.equal(h.document.activeElement, name, "focus moves into the dialog");
  const label = h.dialog().all().find(e => e.tagName === "LABEL");
  assert.equal(label.htmlFor, name.id, "the label is bound to its field");
  name.value = "Mill 1";
  const add = h.button("Add machine");
  assert.equal(add.className, "primary", "no inline color: the primary style");
  add.onclick();
  await tick();
  assert.equal(add.textContent, "Adding…");
  assert.equal(add.disabled, true);
  assert.equal(name.disabled, true);
  add.onclick(); h.dialog().all().find(e => e.tagName === "FORM").onsubmit({ preventDefault(){} });
  await tick();
  assert.deepEqual(calls, ["Mill 1"], "a second submit while busy does nothing");
  release.rej(new Error("a machine with this name already exists"));
  await tick(); await tick();
  assert(h.dialog(), "the dialog stays open on an error");
  const err = h.dialog().all().find(e => e.className === "modal-err");
  assert.equal(err.hidden, false);
  assert.equal(err.getAttribute("role"), "alert");
  assert.equal(err.textContent, "A machine with this name already exists");
  assert.equal(name.value, "Mill 1", "what was typed is kept");
  assert.equal(name.disabled, false);
  assert.equal(add.textContent, "Add machine");
  add.onclick();
  await tick();
  release.res({ machine:{ id:"m1" } });
  assert.deepEqual(plain(await p), { machine:{ id:"m1" } }, "resolves with onSubmit's result");
  assert.equal(h.dialog(), undefined, "closed after success");
  assert.equal(h.document.activeElement, h.opener, "focus returns to the opener");
});

test("openModal marks the field an error names, and danger dialogs use the danger style", async () => {
  const h = modalHarness();
  const p = h.ctx.openModal("Delete BE-1?", [{ id:"confirm", label:"Type BE-1 to confirm", required:true }],
    { label:"Delete item", danger:true, onSubmit: v => {
      if (v.confirm !== "BE-1") throw Object.assign(new Error("Type BE-1 exactly to confirm."), { field:"confirm" });
    } });
  const del = h.button("Delete item");
  assert.equal(del.className, "danger");
  h.input("confirm").value = "delete";
  del.onclick(); await tick();
  const confirm = h.input("confirm");
  assert.equal(confirm.getAttribute("aria-invalid"), "true");
  assert.equal(confirm.getAttribute("aria-describedby"), h.dialog().all().find(e => e.className === "modal-err").id);
  assert.equal(h.document.activeElement, confirm);
  confirm.value = "BE-1";
  del.onclick(); await tick();
  assert.deepEqual(plain(await p), { confirm:"BE-1" }, "resolves with the values when onSubmit returns nothing");
});

test("openModal without onSubmit keeps its old contract; Escape and Cancel resolve null", async () => {
  const h = modalHarness();
  const p = h.ctx.openModal("Move", [{ id:"folder", label:"Folder", type:"select",
    options:[{ value:"", label:"(project root)" }, { value:"f1", label:"Housings" }] }], "Move");
  h.input("folder").value = "f1";
  h.button("Move").onclick();
  assert.deepEqual(plain(await p), { folder:"f1" });
  const q = h.ctx.openModal("Rename", [{ id:"name", label:"Name" }]);
  assert(h.button("Save"), "the default label is a verb, not OK");
  h.document.key("Escape");
  assert.equal(await q, null);
  const r = h.ctx.openModal("Rename", [{ id:"name", label:"Name" }], "Rename");
  h.button("Cancel").onclick();
  assert.equal(await r, null);
});

test("openModal stays open on a click beside it once something was typed", async () => {
  const h = modalHarness();
  const p = h.ctx.openModal("New project", [{ id:"name", label:"Name" }], "Create project");
  h.input("name").value = "Frames";
  const wrap = h.dialog();
  wrap.onmousedown({ target: wrap });
  assert(h.dialog(), "typed text is not thrown away by a stray click");
  h.input("name").value = "";
  wrap.onmousedown({ target: wrap });
  assert.equal(await p, null);
});

// ── api() errors and an ended session ────────────────────────────────────
function response(status, body, ctype){
  return { ok: status < 400, status, headers:{ get: () => ctype || "application/json" },
    json: async () => body, text: async () => body, arrayBuffer: async () => new ArrayBuffer(0) };
}
function apiHarness(responses, extra){
  const sent = [];
  const ctx = loadFunctions(["api", "apiErrorText", "bytes"], Object.assign({
    TOKEN:"t1", ME:{ user:{ id:"u1" } }, AUTH_BUSY:false, JSON, String, Number, isNaN,
    fetch: async (url, init) => { sent.push([url, init.headers.Authorization]); return responses.shift(); },
    reauth: async () => false, location:{ reload(){ throw new Error("reloaded"); } },
  }, extra));
  return { ctx, sent };
}

test("api() errors drop the server's category prefix and keep code, status and detail", async () => {
  const { ctx } = apiHarness([response(409, { error:{ code:"conflict", message:"conflict: a project with this name already exists" } })]);
  const e = await ctx.api("POST", "/projects", { name:"x" }).catch(e => e);
  assert.equal(e.message, "a project with this name already exists");
  assert.equal(e.status, 409);
  assert.equal(e.code, "conflict");
  assert.equal(e.detail, "a project with this name already exists");
});

test("api() never shows a proxy's HTML error page; the status is detail, not the headline", async () => {
  const page = "<!DOCTYPE html><html><head><title>502 Bad Gateway</title></head><body><h1>Bad Gateway</h1></body></html>";
  const { ctx } = apiHarness([response(502, page, "text/html")]);
  const e = await ctx.api("GET", "/items").catch(e => e);
  assert(!/</.test(e.message), "no HTML in the message: " + e.message);
  assert.match(e.message, /^the server did not answer; try again in a moment.*\(HTTP 502\)$/);
  const { ctx: c2 } = apiHarness([response(500, "", "text/plain")]);
  assert.match((await c2.api("GET", "/x").catch(e => e)).message, /^the server ran into a problem.*\(HTTP 500\)$/);
});

test("api() formats byte counts in the server's size messages", async () => {
  const { ctx } = apiHarness([response(422, { error:{ code:"validation",
    message:"validation error: upload size 209715200 exceeds limit 104857600" } })]);
  assert.equal((await ctx.api("PUT", "/blobs").catch(e => e)).message, "upload size 200.0 MB exceeds limit 100.0 MB");
});

test("api() keeps the page on an ended session: signs in again, then repeats the request once", async () => {
  let asked = 0;
  const h = apiHarness([response(401, { error:{ code:"auth_failed", message:"authentication failed" } }),
    response(200, { comment:{ id:"c1" } })], { reauth: async () => { asked++; h.ctx.TOKEN = "t2"; return true; } });
  assert.deepEqual(await h.ctx.api("POST", "/items/i1/comments", { body:"hello" }), { comment:{ id:"c1" } });
  assert.equal(asked, 1);
  assert.deepEqual(h.sent, [["/items/i1/comments", "Bearer t1"], ["/items/i1/comments", "Bearer t2"]]);

  const declined = apiHarness([response(401, {})], { reauth: async () => false });
  const e = await declined.ctx.api("GET", "/items").catch(e => e);
  assert.equal(e.status, 401);
  assert.match(e.message, /session ended/);

  const again = apiHarness([response(401, {}), response(401, {})], { reauth: async () => true });
  assert.equal((await again.ctx.api("GET", "/items").catch(e => e)).status, 401, "retried once, not in a loop");

  // A request sent before a sign-in that has since happened is repeated without asking again.
  const late = apiHarness([response(401, {}), response(200, { ok:true })], { reauth: async () => assert.fail("asked twice") });
  late.ctx.fetch = async (url, init) => { late.sent.push(init.headers.Authorization); late.ctx.TOKEN = "t2";
    return [response(401, {}), response(200, { ok:true })][late.sent.length - 1]; };
  assert.equal((await late.ctx.api("GET", "/items")).ok, true);
  assert.deepEqual(late.sent, ["Bearer t1", "Bearer t2"]);

  const anon = apiHarness([response(401, {})], { ME:null, reauth: async () => assert.fail("no dialog before sign-in") });
  assert.equal((await anon.ctx.api("GET", "/me").catch(e => e)).status, 401);
});

// ── toasts ───────────────────────────────────────────────────────────────
function toastHarness(){
  const document = fakeDocument();
  const host = document.createElement("div"); host.id = "toast";
  const status = document.createElement("div"); status.setAttribute("role", "status");
  const alert = document.createElement("div"); alert.setAttribute("role", "alert");
  host.appendChild(status); host.appendChild(alert); document.body.appendChild(host);
  const timers = [];
  const ctx = loadFunctions(["toast", "placeToasts"], { document, $: id => id === "toast" ? host : null, String,
    setTimeout: (f, ms) => { timers.push({ f, ms }); return timers.length; }, clearTimeout(){} });
  return { ctx, status, alert, timers };
}

test("error toasts stay until dismissed in an alert region; information hides itself in a status region", () => {
  const h = toastHarness();
  h.ctx.toast("Couldn't archive BE-1: not found", "err");
  assert.equal(h.alert.children.length, 1);
  assert.equal(h.timers.length, 0, "an error has no hide timer");
  const dismiss = h.alert.children[0].children.find(c => c.textContent === "Dismiss");
  h.ctx.toast("Archived BE-2", "ok", { action:{ label:"Undo", run(){} } });
  assert.equal(h.status.children.length, 1);
  assert.equal(h.alert.children.length, 1, "a later message does not hide the error");
  assert.equal(h.timers[0].ms, 8000, "a message with Undo stays a little longer");
  h.ctx.toast("Downloading", "", 6000);
  assert.equal(h.status.children.length, 1, "one information message at a time");
  assert.equal(h.timers[1].ms, 6000, "the old numeric delay still works");
  dismiss.onclick();
  assert.equal(h.alert.children.length, 0);
});

test("repeated errors are not stacked twice and at most three stay", () => {
  const h = toastHarness();
  for (const m of ["a", "b", "a", "c", "d"]) h.ctx.toast(m, "err");
  assert.deepEqual(h.alert.children.map(c => c.dataset.msg), ["a", "c", "d"], "a repeat moves to the end; the oldest goes");
  let undone = 0;
  h.ctx.toast("Archived BE-1", "ok", { action:{ label:"Undo", run: () => undone++ } });
  h.status.children[0].children.find(c => c.textContent === "Undo").onclick();
  assert.equal(undone, 1);
  assert.equal(h.status.children.length, 0, "the message goes once Undo is pressed");
});

// ── item action menus ────────────────────────────────────────────────────
function menuCtx(role, extra){
  return loadFunctions(["itemMenuEntries"], Object.assign({
    isRole: min => ({ viewer:0, editor:1, admin:2 })[role] >= ({ viewer:0, editor:1, admin:2 })[min],
    ROUTE:{ name:"items" }, DETAIL:{}, PMAP:{}, myName: () => "Me",
    PdmEditors:{ forFile: () => null }, convertibleByName: () => false,
  }, extra));
}
const labels = entries => plain(entries).filter(e => e !== "sep").map(e => e.label);
const item = o => Object.assign({ id:"i1", part_number:"BE-1", project_id:"p1", head_version_seq:2,
  lifecycle_state:"released", archived_at:null, locked_by:null }, o);

test("viewers get no edit or destructive entries", () => {
  assert.deepEqual(labels(menuCtx("viewer").itemMenuEntries(item())), ["Open details", "Download latest"]);
  assert.deepEqual(labels(menuCtx("viewer").itemMenuEntries(item({ locked_by:"Me" }))),
    ["Open details", "Download latest", "Undo checkout"], "anyone may let go of their own checkout");
});

test("editors get one list for both menus, with honest labels and the archived state", () => {
  const ed = menuCtx("editor");
  assert.deepEqual(labels(ed.itemMenuEntries(item())), ["Open details", "Download latest", "Check out",
    "Open to upload a new version", "Move to folder…", "Assign owner…", "Set as main assembly",
    "Open to release", "Mark obsolete…", "Archive", "Delete…"]);
  const archived = labels(ed.itemMenuEntries(item({ archived_at:"2026-01-01" })));
  assert(archived.includes("Unarchive") && !archived.includes("Archive"));
  assert(!archived.includes("Check out") && !archived.includes("Mark obsolete…"));
  const onPage = menuCtx("editor", { ROUTE:{ name:"detail" }, DETAIL:{ id:"i1" } }).itemMenuEntries(item());
  assert(!labels(onPage).some(l => /^Open /.test(l)), "the item page's own header covers open, upload and release");
  assert.equal(onPage.at(-1).danger, true);
});

test("archive reads the real state when the caller has no row, and offers Undo", async () => {
  const calls = [], notes = [];
  const ctx = loadFunctions(["doArchiveItem"], {
    menuItem: async () => item({ archived_at:"2026-01-01" }),
    api: async (m, u) => { calls.push(m + " " + u); return {}; },
    toast: (m, k, o) => notes.push({ m, k, o }), ALLITEMS:[], CUR_RELOAD:null, Object, Date,
  });
  await ctx.doArchiveItem("i1");
  assert.deepEqual(calls, ["PATCH /items/i1/unarchive"], "an archived item is unarchived, not archived again");
  assert.equal(notes[0].m, "Unarchived BE-1");
  assert.equal(notes[0].o.action.label, "Undo");
  await notes[0].o.action.run();
  assert.deepEqual(calls, ["PATCH /items/i1/unarchive", "PATCH /items/i1/archive"]);
});

test("delete names the item, asks for its part number and deletes only on a match", async () => {
  const calls = [];
  let seen;
  const ctx = loadFunctions(["doDeleteItem"], {
    menuItem: async () => assert.fail("the caller passed the item"),
    openModal: async (title, fields, opt, extra) => {
      seen = { title, label: fields[0].label, opt, extra };
      const wrong = await opt.onSubmit({ confirm:"delete" }).catch(e => e);
      assert.equal(wrong.field, "confirm");
      assert.deepEqual(calls, [], "a wrong part number deletes nothing");
      await opt.onSubmit({ confirm:"be-1 " });
      return true;
    },
    api: async (m, u) => { calls.push(m + " " + u); return {}; },
    esc: String, toast(){}, go(){}, ALLITEMS:[], CUR_RELOAD:null, ROUTE:{ name:"items" }, DETAIL:{}, Number, Object,
  });
  await ctx.doDeleteItem("i1", item({ description:"Bracket", latest_revision:"A" }));
  assert.equal(seen.title, "Delete BE-1 — Bracket?");
  assert.equal(seen.label, "Type BE-1 to confirm");
  assert.equal(seen.opt.label, "Delete item");
  assert.equal(seen.opt.danger, true);
  assert.match(seen.extra, /its 2 versions\. This cannot be undone\. It has released revision A\./);
  assert.deepEqual(calls, ["DELETE /items/i1"]);
});

test("marking obsolete asks first and explains a missing lifecycle setup", async () => {
  const ctx = loadFunctions(["doLifecycle"], {
    openModal: async (title, fields, opt, extra) => {
      assert.equal(title, "Mark BE-1 as obsolete?");
      assert.match(extra, /from Released to Obsolete/);
      const e = await opt.onSubmit({}).catch(e => e);
      assert.match(e.message, /no lifecycle rules set up/);
      return null;
    },
    api: async () => { throw new Error("org has no lifecycle definition configured"); },
    esc: String, String, ALLITEMS:[], CUR_RELOAD: () => assert.fail("cancelled"),
  });
  await ctx.doLifecycle("i1", "obsolete", item());
});

// ── bulk actions ─────────────────────────────────────────────────────────
test("bulk move goes on past a failure and says which items were not moved and why", async () => {
  const calls = [], notes = [];
  const rows = [item({ id:"a", part_number:"BE-1" }), item({ id:"b", part_number:"BE-2" }), item({ id:"c", part_number:"BE-3" })];
  const ctx = loadFunctions(["moveItems", "bulkRun", "bulkReport", "bulkItem", "itemCountLabel", "cachedItem"], {
    ROUTE:{ name:"explorer", _items: rows, _folders:[{ id:"f1", name:"Housings" }] }, ALLITEMS:[], DETAIL:{},
    openModal: async (title, fields, opt) => { assert.equal(title, "Move 3 items to a folder"); await opt.onSubmit({ folder:"f1" }); return true; },
    api: async (m, u) => { calls.push(u); if (u === "/items/b") throw new Error("checked out by Sam"); return {}; },
    toast: (m, k) => notes.push([m, k]), CUR_RELOAD:null, String,
  });
  const failed = await ctx.moveItems("p1", ["a", "b", "c"]);
  assert.deepEqual(calls, ["/items/a", "/items/b", "/items/c"], "the third item is still moved");
  assert.deepEqual(plain(failed), ["b"]);
  assert.deepEqual(notes, [["Moved 2 of 3 items. Not moved: BE-2 (checked out by Sam).", "err"]]);
});

// ── manufacturing hub names and labels ───────────────────────────────────
test("the hub pages through the item list until every item it shows has a name", async () => {
  const pages = [];
  const page = (n, ids, more) => ({ items: ids.map(id => ({ id, part_number: id })), has_more: more, page: n });
  const ctx = loadFunctions(["itemsById"], {
    api: async (m, u) => { pages.push(u); return u.endsWith("page=2") ? page(2, ["x201"], true) : page(3, ["x401"], false); },
  });
  const map = await ctx.itemsById(["x1", "x201", null], Promise.resolve(page(1, ["x1"], true)));
  assert.deepEqual(Object.keys(map), ["x1", "x201"]);
  assert.deepEqual(pages, ["/items?include_archived=true&limit=200&page=2"], "stops once every id is named");
});

test("stored codes show as words", () => {
  const ctx = loadFunctions(["enumLabel", "enumOptions"], { Object, String });
  vm.runInContext(constBlock("ENUM_LABELS"), ctx);
  assert.equal(ctx.enumLabel("machine_kind", "cnc_mill"), "CNC mill");
  assert.equal(ctx.enumLabel("job_status", "in_progress"), "In progress");
  assert.equal(ctx.enumLabel("job_status", "on_hold"), "on hold", "an unknown code still reads as words");
  assert.equal(ctx.enumLabel("maintenance", null), "—");
  assert.deepEqual(JSON.parse(JSON.stringify(ctx.enumOptions("maintenance"))),
    [{ value:"ok", label:"OK" }, { value:"due", label:"Due" }, { value:"overdue", label:"Overdue" }]);
});

// ── review follow-ups ────────────────────────────────────────────────────
test("a 404 or a server fault reads as a plain sentence; the server's text is detail only", async () => {
  const { ctx } = apiHarness([response(404, { error:{ code:"not_found", message:"not found: item 3fa85f64-5717-4562-b3fc-2c963f66afa6" } }),
    response(500, { error:{ code:"db_error", message:"database error: pool timed out" } })]);
  const nf = await ctx.api("DELETE", "/items/x").catch(e => e);
  assert.equal(nf.message, "not found; it may have been deleted");
  assert.equal(nf.detail, "item 3fa85f64-5717-4562-b3fc-2c963f66afa6");
  const se = await ctx.api("GET", "/items").catch(e => e);
  assert.match(se.message, /^the server ran into a problem.*\(HTTP 500\)$/);
  assert.equal(se.detail, "pool timed out");
  const { ctx: c3 } = apiHarness([response(404, { error:{ code:"not_found", message:"not found: sheet DXF not produced" } })]);
  assert.equal((await c3.api("GET", "/x").catch(e => e)).message, "sheet DXF not produced", "a 404 that says something keeps its words");
});

test("Escape while a native dialog is busy does nothing; a close the browser forces opens it again", async () => {
  const h = modalHarness();
  // The native path: showModal, cancel and close events.
  h.document.createElement = tag => { const el = new El(tag, h.document);
    if (tag === "dialog"){ el.showModal = () => { el.open = true; }; el.close = () => { el.open = false; }; }
    return el; };
  let release;
  const p = h.ctx.openModal("Delete BE-1?", [], { label:"Delete item", danger:true,
    onSubmit: () => new Promise(r => { release = r; }) });
  const wrap = h.dialog();
  h.button("Delete item").onclick(); await tick();
  let prevented = false;
  wrap.dispatch("cancel", { preventDefault(){ prevented = true; } });
  assert(prevented && h.dialog(), "Escape is held while the request runs");
  wrap.open = false; wrap.dispatch("close");
  assert(h.dialog() && wrap.open, "a close the browser forces opens the dialog again");
  release({ ok:true });
  assert.deepEqual(plain(await p), { ok:true }, "the request's result reaches the caller");
  assert.equal(h.dialog(), undefined, "closed after success");
  assert.equal(h.document.activeElement, h.opener);
});

test("declining to sign in again keeps offering a way back on each later refusal", async () => {
  const notes = [];
  let dialogs = 0;
  const ctx = loadFunctions(["apiErrorText", "signedOutNotice", "reauth"], {
    ME:{ user:{ id:"u1", email:"a@example.test" } }, $: () => null, localStorage:{ setItem(){} },
    openModal: async () => { dialogs++; return null; }, toast: (m, k, o) => notes.push([m, k, o && o.action.label]),
    render(){}, location:{ reload(){} }, JSON, String, placeToasts: () => null,
  });
  assert.equal(await ctx.reauth(), false);
  assert.equal(await ctx.reauth(), false);
  assert.equal(dialogs, 1, "no second dialog without asking");
  assert.equal(notes.length, 2, "the signed-out message comes back once it is no longer on screen");
  assert.deepEqual(notes[1], ["You are signed out. Changes are not saved until you sign in again.", "err", "Sign in"]);
});

test("project delete refuses when the project's name is not known", async () => {
  const notes = [];
  const ctx = loadFunctions(["doDeleteProject"], {
    PMAP:{}, loadCommon: async () => {}, openModal: async () => assert.fail("no dialog without a name"),
    api: async () => assert.fail("no request"), toast: (m, k) => notes.push(k), String,
  });
  await ctx.doDeleteProject("p1");
  assert.deepEqual(notes, ["err"]);
});

test("a menu's item names the bulk report when the list keeps no rows", async () => {
  const notes = [];
  const ctx = loadFunctions(["moveItems", "bulkRun", "bulkReport", "bulkItem", "itemCountLabel", "cachedItem"], {
    ROUTE:{ name:"items", _folders:[] }, ALLITEMS:[], DETAIL:{},
    openModal: async (title, fields, opt) => { assert.equal(title, "Move BE-9 to a folder"); await opt.onSubmit({ folder:"" }); return true; },
    api: async () => { throw new Error("checked out by Sam"); },
    toast: (m, k) => notes.push(m), CUR_RELOAD:null, String,
  });
  await ctx.moveItems("p1", ["i9"], [item({ id:"i9", part_number:"BE-9" })]);
  assert.deepEqual(notes, ["Moved 0 of 1 item. Not moved: BE-9 (checked out by Sam)."]);
});
