// Check-in safety and lock visibility in the web portal.
// Run with: node --test crates/pdm-api/tests/web-ui-checkin.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const app = fs.readFileSync(path.join(__dirname, "../static/app.js"), "utf8");
const editors = fs.readFileSync(path.join(__dirname, "../static/editors.js"), "utf8");

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

// Arrays made inside a vm context have their own prototype; compare plain copies.
const plain = x => JSON.parse(JSON.stringify(x));
const CHECKIN = ["checkinItem", "commitVersion", "checkinPrecheck", "checkinRefusal", "uploadBlob",
  "newerVersionError", "lockHeldError", "clockTime"];

// A server double for one item: `state.head` is the head version, `state.lock` the lock.
// Every request is recorded; `refuse` makes the next check-in fail with that 409 message.
function server(state){
  const calls = [];
  const api = async (method, url, body) => {
    calls.push([method, url, body]);
    if (method === "GET" && url === "/items/item") return { head_version: state.head, lock: state.lock, file: { filename: "part.step" } };
    if (method === "POST" && url === "/items/item/checkout"){ state.lock = { user_id: "me" }; return {}; }
    if (method === "DELETE" && url === "/items/item/checkout"){ state.lock = null; return {}; }
    if (method === "POST" && url === "/items/item/checkin"){
      if (state.refuse){
        const msg = state.refuse; state.refuse = null;
        if (state.raceTo) state.head = state.raceTo;
        throw Object.assign(new Error(msg), { status: 409 });
      }
      state.lock = null;
      state.head = { id: "v" + ((state.head ? state.head.seq : 0) + 1), seq: (state.head ? state.head.seq : 0) + 1 };
      return { version: state.head };
    }
    assert.fail("unexpected request " + method + " " + url);
  };
  return { api, calls };
}
class FakeXHR {
  constructor(){ this.upload = {}; FakeXHR.sent.push(this); }
  open(method, url){ this.method = method; this.url = url; }
  setRequestHeader(){}
  send(body){
    this.body = body;
    setImmediate(() => {
      if (FakeXHR.fail){ FakeXHR.fail = false; this.onerror(); return; }
      if (this.upload.onprogress) this.upload.onprogress({ lengthComputable: true, loaded: 3, total: 3 });
      this.status = 200;
      this.responseText = JSON.stringify({ blob: { sha256: "abc", size_bytes: 3 } });
      this.onload();
    });
  }
}
FakeXHR.sent = [];
function checkinContext(api, extra){
  return loadFunctions(CHECKIN, Object.assign({
    api, myId: () => "me", lockName: l => l.user_id === "bob" ? "Bob" : "someone",
    fmtTime: () => "5m ago", location: { hostname: "test" }, TOKEN: "t", XMLHttpRequest: FakeXHR,
    ALLITEMS: [], ALLITEMS_AT: 0, JSON, Date, String, Object, Error, Promise,
  }, extra || {}));
}
const file = { name: "part.step", type: "", size: 3 };

test("a check-in on top of an older version is refused before any upload or lock", async () => {
  const state = { head: { id: "v2", seq: 2, author_name: "Bob", committed_at: "2026-11-20T14:05:00Z" }, lock: null };
  const { api, calls } = server(state);
  FakeXHR.sent = [];
  const ctx = checkinContext(api);
  const err = await ctx.checkinItem("item", file, "msg", "part.step", 1).then(() => null, e => e);
  assert.ok(err, "the check-in must be refused");
  assert.deepEqual(err.conflict && err.conflict.seq, 2);
  assert.match(err.message, /^v2 was checked in by Bob at 2026-11-2\d \d\d:\d\d after you opened v1\. Your file was not checked in\. Download v2/);
  assert.equal(FakeXHR.sent.length, 0, "no upload");
  assert.deepEqual(calls.map(c => c[0] + " " + c[1]), ["GET /items/item"], "no checkout, no check-in");
});

test("the shown base is sent, and a refused check-in lets go of the lock it took", async () => {
  // Head is v1 when the check-in starts; someone checks in v2 between the pre-check and ours.
  const state = { head: { id: "v1", seq: 1 }, lock: null, refuse: "stale base version 1, current head is 2",
    raceTo: { id: "v2", seq: 2, author_name: "Bob", committed_at: "2026-11-20T14:05:00Z" } };
  const { api, calls } = server(state);
  const ctx = checkinContext(api);
  const err = await ctx.checkinItem("item", file, "msg", "part.step", 1).then(() => null, e => e);
  const checkin = calls.find(c => c[1] === "/items/item/checkin");
  assert.equal(checkin[2].base_version, 1, "the version the user saw, not a fresh head");
  assert.equal(checkin[2].sha256, "abc");
  assert.equal(err.conflict.seq, 2);
  assert.match(err.message, /v2 was checked in by Bob/);
  assert.ok(calls.some(c => c[0] === "DELETE" && c[1] === "/items/item/checkout"), "the lock taken here is released");
  assert.equal(state.lock, null);
});

test("a check-in on top of the shown head goes through and keeps nothing locked", async () => {
  const state = { head: { id: "v3", seq: 3 }, lock: null };
  const { api, calls } = server(state);
  const progress = [];
  const ctx = checkinContext(api);
  const r = await ctx.checkinItem("item", file, "msg", "part.step", 3, (a, b) => progress.push([a, b]));
  assert.equal(r.version.seq, 4);
  assert.equal(calls.find(c => c[1] === "/items/item/checkin")[2].base_version, 3);
  assert.ok(!calls.some(c => c[0] === "DELETE"));
  assert.deepEqual(progress, [[3, 3]], "upload progress is reported");
  assert.equal(FakeXHR.sent.at(-1).body, file, "the File itself is sent, not a copy in memory");
});

test("a lock the user already holds is kept when the check-in fails", async () => {
  const state = { head: { id: "v1", seq: 1 }, lock: { user_id: "me" }, refuse: "no active checkout" };
  const { api, calls } = server(state);
  const ctx = checkinContext(api);
  const err = await ctx.checkinItem("item", file, "", "part.step", 1).then(() => null, e => e);
  assert.ok(!calls.some(c => c[1] === "/items/item/checkout"), "no second checkout, no release");
  assert.match(err.message, /Your checkout ended before the check-in finished\. Check out again, then retry\./);
});

test("someone else's lock is named with its end time, with advice the user can follow", async () => {
  const state = { head: { id: "v1", seq: 1 }, lock: { user_id: "bob", lease_expires_at: "2099-01-02T03:04:00Z" } };
  const { api } = server(state);
  FakeXHR.sent = [];
  const ctx = checkinContext(api);
  const err = await ctx.checkinItem("item", file, "", "part.step", 1).then(() => null, e => e);
  assert.ok(err.lockHeld);
  assert.match(err.message, /^Bob has this item checked out \(lock ends at 2099-01-0\d \d\d:\d\d\)\. Ask them to check in or undo their checkout, then try again\.$/);
  assert.equal(FakeXHR.sent.length, 0);
});

test("clockTime shows today's end as a time and a later one with its date", () => {
  const ctx = loadFunctions(["clockTime"], { Date, String });
  const soon = new Date(); soon.setHours(17, 5, 0, 0);
  assert.equal(ctx.clockTime(soon.toISOString()), "17:05");
  assert.match(ctx.clockTime("2099-03-04T12:00:00Z"), /^2099-03-0\d \d\d:\d\d$/);
  assert.equal(ctx.clockTime(null), "—");
});

// doUpload with the panel and the blob upload stubbed: what is created, when, and the retry.
function uploadHarness({ uploadFails = 0, checkinFails = 0 } = {}){
  const state = { head: null, lock: null }, made = [], rows = [];
  const base = server(state);
  const api = async (method, url, body) => {
    if (method === "POST" && url === "/projects/p1/items"){
      made.push(body); return { item: { id: "item", part_number: "BE-000010" } };
    }
    if (method === "POST" && url === "/items/item/checkin" && checkinFails > 0){
      checkinFails--; base.calls.push([method, url, body]); throw new Error("network error");
    }
    return base.api(method, url, body);
  };
  let uploads = 0;
  const ctx = loadFunctions(["doUpload", "commitVersion", "checkinPrecheck", "checkinRefusal", "newerVersionError",
    "lockHeldError", "clockTime", "folderPathOptions"], {
    api, myId: () => "me", lockName: () => "x", fmtTime: () => "", location: { hostname: "t" },
    ALLITEMS: [], ALLITEMS_AT: 0, Date, String, Object, Error,
    ROUTE: { name: "explorer", pid: "p1", _folder: null, _folders: [{ id: "f1", name: "Housings" }, { id: "f2", name: "Rev B", parent_id: "f1" }] },
    DETAIL: {}, render(){}, go(){}, CUR_RELOAD: null, toast: m => assert.fail(m),
    uploadBlob: async () => { uploads++; if (uploadFails > 0){ uploadFails--; throw new Error("The connection dropped"); } return { blob: { sha256: "abc", size_bytes: 3 } }; },
    uploadPanel: () => ({ add: name => { const row = { name, log: [] };
      row.step = t => row.log.push(["step", t]); row.progress = () => {};
      row.done = (t, b) => { row.log.push(["done", t]); row.buttons = b; };
      row.fail = (t, r) => { row.log.push(["fail", t]); row.retry = r; };
      rows.push(row); return row; } }),
  });
  return { ctx, made, rows, calls: base.calls, state, uploads: () => uploads };
}

test("a failed upload creates nothing, and Retry uploads, creates and checks in once", async () => {
  const h = uploadHarness({ uploadFails: 1 });
  await h.ctx.doUpload("p1", { cls: "BE", folder: "f2", msg: "" }, [{ name: "hinge.step", type: "" }]);
  assert.equal(h.made.length, 0, "no empty item is left behind");
  const row = h.rows[0];
  assert.match(row.log.at(-1)[1], /^Not uploaded: The connection dropped/);
  assert.equal(row.retry[0], "Retry");
  await row.retry[1]();
  assert.equal(h.made.length, 1);
  assert.equal(h.made[0].folder_id, "f2");
  assert.deepEqual(row.log.at(-1), ["done", "BE-000010 v1 · in Housings / Rev B"]);
  assert.equal(row.buttons[0][0], "Show", "the items landed outside the folder on screen");
  assert.equal(h.calls.find(c => c[1] === "/items/item/checkin")[2].base_version, null);
});

test("a check-in that fails after the item exists releases the lock and retries into the same item", async () => {
  const h = uploadHarness({ checkinFails: 1 });
  await h.ctx.doUpload("p1", { cls: "BE", folder: "", msg: "first" }, [{ name: "clip.step", type: "" }]);
  const row = h.rows[0];
  assert.match(row.log.at(-1)[1], /^BE-000010 was created but the file was not checked in: network error/);
  assert.equal(h.state.lock, null, "the checkout taken for the upload is released");
  assert.equal(row.retry[0], "Retry into BE-000010");
  await row.retry[1]();
  assert.equal(h.made.length, 1, "no second item");
  assert.equal(h.uploads(), 1, "the uploaded bytes are reused");
  assert.deepEqual(row.log.at(-1), ["done", "BE-000010 v1 · in the project root"]);
  assert.deepEqual(plain(row.buttons), [], "already in view: the explorer shows the project root");
});

test("the upload dialog starts in the folder on screen, shows folder paths and configured categories once", async () => {
  let fields;
  const ctx = loadFunctions(["uploadModal", "categoryOptions", "folderPathOptions"], {
    ROUTE: { pid: "p1", _folder: "f2", _folders: [{ id: "f1", name: "Housings" }, { id: "f2", name: "Rev B", parent_id: "f1" }, { id: "f3", name: "Rev B" }] },
    api: async () => ({ prefixes: [{ category: "be", prefix: "BE-" }, { category: "Fixtures", prefix: "FX-" }] }),
    openModal: async (title, f) => { fields = f; return null; }, esc: String, bytes: String,
  });
  await ctx.uploadModal("p1", [{ name: "a.step", size: 1 }]);
  const folder = fields.find(f => f.id === "folder"), cls = fields.find(f => f.id === "cls");
  assert.equal(folder.value, "f2");
  assert.deepEqual(plain(folder.options.map(o => o.label)), ["(project root)", "Housings", "Housings / Rev B", "Rev B"]);
  assert.deepEqual(plain(cls.options.map(o => o.value)), ["be", "Fixtures", "ASM", "MFG", "SC"], "a configured legacy class is not listed twice");
});

test("release names the version, and stops if a newer one arrived after the page loaded", async () => {
  const calls = [], notices = [];
  let note, current = { id: "v3", seq: 3 };
  const ctx = loadFunctions(["releaseItem", "autoLabel", "lockSummary", "clockTime"], {
    DETAIL: { id: "item", revs: [], data: { head_version: { id: "v3", seq: 3, author_name: "Ana", committed_at: "2026-11-20T14:05:00Z" },
      lock: { user_id: "bob", workstation: "WS-2", acquired_at: "y", lease_expires_at: "2099-01-01T10:00:00Z" } } },
    openModal: async (t, f, label, extra) => { note = extra; return { label: "A", note: "" }; },
    api: async (method, url, body) => { calls.push([method, url]);
      if (method === "GET") return { head_version: current };
      return { status: "pending_approval" }; },
    esc: String, fmtTime: () => "2h ago", myId: () => "me", lockName: () => "Bob", Date, String,
    location: { hostname: "pdm.example" },
    toast: (m, k) => notices.push([m, k]), refreshApprovalsBadge(){}, CUR_RELOAD: null,
  });
  const detail = ctx.DETAIL;
  await ctx.releaseItem();
  assert.match(note, /Releases v3, checked in by Ana at 2026-11-2\d \d\d:\d\d\./);
  assert.match(note, /the label and note entered here are not kept/);
  assert.match(note, /checked out by Bob on WS-2 2h ago · lock ends at/);
  assert.match(notices.at(-1)[0], /^Sent for approval\. When it is approved, the version that is current then is released/);
  current = { id: "v4", seq: 4, author_name: "Bob", committed_at: "2026-11-21T09:00:00Z" };
  calls.length = 0;
  await ctx.releaseItem();
  assert.ok(!calls.some(c => c[0] === "POST"), "nothing is released");
  assert.equal(detail.conflict.seq, 4, "the item page banner says why and offers v4");
  assert.match(detail.conflict.text, /^Not released: v4 was checked in by Bob at 2026-11-2\d \d\d:\d\d after this page loaded\./);
});

test("a check-out made in this portal reads as the web portal, not the server's host name", () => {
  const ctx = loadFunctions(["lockSummary", "clockTime"], {
    myId: () => "me", lockName: () => "Ana", fmtTime: () => "3h ago", location: { hostname: "pdm.example" }, Date, String,
  });
  assert.match(ctx.lockSummary({ user_id: "ana", workstation: "pdm.example", acquired_at: "", lease_expires_at: "" }),
    /^checked out by Ana in the web portal 3h ago · lock ends at/);
  assert.match(ctx.lockSummary({ user_id: "me", workstation: "WS-12", acquired_at: "", lease_expires_at: "" }),
    /^checked out by you on WS-12 3h ago/);
});

test("a check-in that does not say which version it builds on is refused", async () => {
  const { api, calls } = server({ head: { id: "v1", seq: 1 }, lock: null });
  const ctx = checkinContext(api);
  await assert.rejects(ctx.checkinItem("item", file, "", "part.step"), /did not say which version/);
  assert.equal(calls.length, 0);
});

test("a STEP import into a linked item goes on top of its current version", async () => {
  const state = { head: { id: "v1", seq: 1 }, lock: null };
  const { api: itemApi, calls } = server(state);
  const api = async (method, url, body) => {
    if (url.startsWith("/integrations/onshape/lookup?")) return { item: { id: "item", part_number: "BE-100", project_id: "project" } };
    if (method === "PUT" && url.includes("/properties/")) return {};
    return itemApi(method, url, body);
  };
  const ctx = loadFunctions(["importOnshapeStep", ...CHECKIN], {
    api, myId: () => "me", lockName: () => "x", fmtTime: () => "", location: { hostname: "t" }, TOKEN: "t",
    XMLHttpRequest: FakeXHR, ALLITEMS: [], ALLITEMS_AT: 0, JSON, Date, String, Object, Error, Promise,
    importVersionToSolidcraft: async () => assert.fail("conversion not asked for"),
  });
  const url = "https://cad.onshape.com/documents/01f013e44a26420bf40d219b/w/84d2c05360df47c98d789b08/e/abc41199f68ccad25c577b1a";
  const result = await ctx.importOnshapeStep("project", { name: "model.step", type: "", size: 12 }, { url, convert: false });
  assert.equal(result.version.seq, 2);
  assert.equal(calls.find(c => c[1] === "/items/item/checkin")[2].base_version, 1);
});

test("restore goes on top of the shown head and reports a newer one in the banner", async () => {
  const state = { head: { id: "v5", seq: 5, author_name: "Bob", committed_at: "2026-11-20T14:05:00Z" }, lock: null };
  const { api, calls } = server(state);
  const detail = { id: "item", cmp: new Set(), data: { head_version: { id: "v4", seq: 4 }, file: { filename: "part.step" } } };
  const ctx = loadFunctions(["restoreVersion", ...CHECKIN], {
    api, DETAIL: detail, openModal: async () => ({ msg: "Restore of v2" }), myId: () => "me",
    lockName: () => "x", fmtTime: () => "1m ago", location: { hostname: "t" }, CUR_RELOAD: null,
    toast: m => assert.fail(m), ALLITEMS: [], ALLITEMS_AT: 0, Date, String, Object, Error,
  });
  await ctx.restoreVersion({ seq: 2, sha256: "old", size_bytes: 9, mime_type: "" });
  assert.ok(!calls.some(c => c[1] === "/items/item/checkin"), "v5 is not silently replaced");
  assert.equal(detail.conflict.seq, 5);
  assert.equal(detail.conflict.itemId, "item");
  assert.match(detail.conflict.text, /^v2 was not restored: v5 was checked in by Bob at 2026-11-2\d \d\d:\d\d after you opened v4\./);
});

test("a long job points at the Dashboard's Active jobs, not a missing Tasks view", async () => {
  const ctx = loadFunctions(["waitForVersionJob"], {
    api: async () => ({ job: { status: "running" } }), setTimeout: fn => fn(), Promise, Error,
  });
  const err = await ctx.waitForVersionJob("j", { isConnected: true }).then(() => null, e => e);
  assert.match(err.message, /Dashboard under Active jobs/);
  assert.doesNotMatch(err.message, /Tasks/);
});

// ── SolidCraft web editor: closing with edits asks first ───────────────────────────────
// A small DOM double: listeners, inline style and focus are real enough for the close flow.
function editorDom(appDocuments){
  const removed = [], windowListeners = {};
  function element(tag){
    const listeners = {}, children = new Map(), classes = new Set();
    const el = {
      tagName: tag, style: {}, hidden: false, isConnected: true, open: false,
      setAttribute(){}, focus(){ el.focused = true; },
      addEventListener(type, fn){ (listeners[type] = listeners[type] || []).push(fn); },
      dispatch(type, ev){ for (const fn of listeners[type] || []) fn(ev || {}); },
      appendChild(){}, showModal(){ el.open = true; }, close(){ el.open = false; el.dispatch("close"); },
      remove(){ el.isConnected = false; removed.push(tag); },
      classList: { add: c => classes.add(c), remove: c => classes.delete(c), contains: c => classes.has(c) },
      querySelector(selector){
        if (!children.has(selector)){
          const child = element(selector);
          if (selector === "[data-ask]") child.style.display = "none"; // as in the markup
          children.set(selector, child);
        }
        return children.get(selector);
      },
      click(){},
    };
    if (tag === "dialog") editorDom.dialog = el;
    if (tag === "iframe"){
      const frameListeners = {};
      el.contentWindow = {
        addEventListener(type, fn){ (frameListeners[type] = frameListeners[type] || []).push(fn); },
        fire(type){ for (const fn of frameListeners[type] || []) fn({}); },
        solidcraft: {
          execute: async (_, args) => { storage.set("solidcraft.file." + args.path.replace("/opfs/", ""), "tdesign"); },
          request: appDocuments ? async () => appDocuments() : undefined,
        },
      };
    }
    return el;
  }
  const storage = new Map();
  const window = {
    localStorage: { getItem: k => storage.get(k), removeItem: k => storage.delete(k) },
    addEventListener(type, fn){ (windowListeners[type] = windowListeners[type] || new Set()).add(fn); },
    removeEventListener(type, fn){ if (windowListeners[type]) windowListeners[type].delete(fn); },
  };
  const context = vm.createContext({
    window, document: { createElement: element, body: { appendChild(){} } },
    navigator: {}, location: { origin: "http://test" }, Blob, TextEncoder, Uint8Array, ArrayBuffer, Promise,
    URL: { createObjectURL: () => "blob:test", revokeObjectURL(){} }, setTimeout, clearTimeout,
    fetch: async () => ({ ok: true }), prompt: () => "Edit",
  });
  vm.runInContext(editors, context);
  const unloadBlocked = () => {
    let blocked = false;
    for (const fn of windowListeners.beforeunload || []) fn({ preventDefault(){ blocked = true; } });
    return blocked;
  };
  return { window, removed, unloadBlocked };
}
async function openEditor(config, appDocuments){
  const dom = editorDom(appDocuments);
  await dom.window.PdmEditors.openSolidcraft(Object.assign({ version: { seq: 1 }, name: "design.sc", designUrl: "/d" }, config));
  const dialog = editorDom.dialog, frame = dialog.querySelector("iframe");
  frame.dispatch("load");
  const q = s => dialog.querySelector(s);
  return { dom, dialog, frame, q, asking: () => q("[data-ask]").style.display !== "none" };
}
const settle = () => new Promise(r => setImmediate(r));

test("closing an untouched SolidCraft editor closes at once", async () => {
  const e = await openEditor({ checkin: async () => ({ seq: 2 }) });
  assert.equal(e.dom.unloadBlocked(), false);
  await e.q("[data-close]").onclick();
  assert.equal(e.dialog.isConnected, false);
});

test("closing after an edit asks first; Escape keeps editing; Discard closes", async () => {
  const e = await openEditor({ checkin: async () => ({ seq: 2 }) });
  e.frame.contentWindow.fire("keydown");
  assert.equal(e.dom.unloadBlocked(), true, "leaving the page asks too");
  await e.q("[data-close]").onclick();
  assert.equal(e.dialog.isConnected, true);
  assert.equal(e.asking(), true);
  assert.equal(e.q("[data-ask-keep]").focused, true, "the safe choice has focus");
  assert.equal(e.q("[data-ask-save]").hidden, false);
  e.dialog.dispatch("cancel", { preventDefault(){}, cancelable: true });
  await settle();
  assert.equal(e.asking(), false, "Escape while asking: keep editing");
  assert.equal(e.dialog.isConnected, true);
  e.dialog.dispatch("cancel", { preventDefault(){}, cancelable: true });
  await settle();
  assert.equal(e.asking(), true, "Escape asks before closing");
  e.q("[data-ask-discard]").onclick();
  assert.equal(e.dialog.isConnected, false);
  assert.equal(e.dom.unloadBlocked(), false, "the unload guard goes with the editor");
});

test("a browser-forced close (repeated Escape) brings the editor back", async () => {
  const e = await openEditor({ checkin: async () => ({ seq: 2 }) });
  e.frame.contentWindow.fire("pointerdown");
  e.dialog.dispatch("cancel", { preventDefault(){}, cancelable: false });
  e.dialog.close();
  await settle();
  assert.equal(e.dialog.open, true, "reopened");
  assert.equal(e.dialog.isConnected, true);
  assert.equal(e.asking(), true, "and asks what to do with the edits");
});

test("Save to PDM from the question checks in and then closes; later closes do not ask", async () => {
  const saved = [];
  const e = await openEditor({ checkin: async (bytes, message) => { saved.push(message); return { seq: 2 }; } });
  e.frame.contentWindow.fire("pointerdown");
  await e.q("[data-close]").onclick();
  assert.equal(e.asking(), true);
  await e.q("[data-ask-save]").onclick();
  assert.deepEqual(saved, ["Edit"]);
  assert.equal(e.dialog.isConnected, false, "closed once the design is in PDM");
});

test("a failed save keeps the editor open and still asks on close", async () => {
  const e = await openEditor({ checkin: async () => { throw new Error("network offline"); } });
  e.frame.contentWindow.fire("pointerdown");
  await e.q("[data-close]").onclick();
  await e.q("[data-ask-save]").onclick();
  assert.equal(e.dialog.isConnected, true);
  assert.match(e.q("[role=status]").textContent, /network offline/);
  await e.q("[data-close]").onclick();
  assert.equal(e.asking(), true);
});

test("a save started while the question shows hides it, and Discard cannot cut the save short", async () => {
  let finish;
  const e = await openEditor({ checkin: () => new Promise(resolve => { finish = resolve; }) });
  e.frame.contentWindow.fire("pointerdown");
  await e.q("[data-close]").onclick();
  assert.equal(e.asking(), true);
  const saving = e.q("[data-checkin]").onclick(); // the toolbar's own Save to PDM
  await settle();
  assert.equal(e.asking(), false, "the question is answered by the save");
  e.q("[data-ask-discard]").onclick();
  await e.q("[data-close]").onclick();
  assert.equal(e.dialog.isConnected, true, "the editor stays while the save runs");
  for (let i = 0; i < 20 && !finish; i++) await settle();
  finish({ seq: 2 });
  assert.equal(await saving, true);
  await e.q("[data-close]").onclick();
  assert.equal(e.dialog.isConnected, false, "saved: closing does not ask");
});

test("SolidCraft's own unsaved-changes flag also makes close ask", async () => {
  const e = await openEditor({ checkin: null }, () => ({ documents: [{ name: "design.sc", dirty: true, drawing: false }] }));
  await e.q("[data-close]").onclick();
  assert.equal(e.asking(), true);
  assert.equal(e.q("[data-ask-save]").hidden, true, "no Save to PDM without the right to check in");
});
