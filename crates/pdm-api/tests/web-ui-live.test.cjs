// Run with: node --test crates/pdm-api/tests/web-ui-live.test.cjs
// Live updates: the stream client, what concerns each page, and refresh-or-bar.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const app = fs.readFileSync(path.join(__dirname, "../static/app.js"), "utf8");

function loadFunctions(names, globals){
  const context = vm.createContext(globals);
  for (const name of names){
    const start = app.indexOf("function " + name + "(");
    assert(start >= 0, "missing function " + name);
    const asyncStart = app.slice(start - 6, start) === "async " ? start - 6 : start;
    const rest = app.slice(start);
    const end = rest.search(/\n(?:async )?function |\n\/\/ ──/);
    vm.runInContext(app.slice(asyncStart, end < 0 ? app.length : start + end), context);
  }
  return context;
}

const ITEM = "11111111-1111-4111-8111-111111111111";
const OTHER = "22222222-2222-4222-8222-222222222222";
const VERSION = "33333333-3333-4333-8333-333333333333";
const ME_ID = "44444444-4444-4444-8444-444444444444";
const PAT = "55555555-5555-4555-8555-555555555555";

function liveState(){
  return { state:"off", ctl:null, lastId:null, backoff:0, retryTimer:null, hideTimer:null,
    authTimer:null, pending:[], batchTimer:null, deferTimer:null, barRoute:null, barTimer:null,
    refreshing:false, lastRefresh:0, refreshTimer:null, news:null };
}
const tick = () => new Promise(r => setImmediate(r));

// A small stand-in for the page: the bar's region, the view, focus and open dialogs.
function page(opts = {}){
  const region = { style:{}, children:[], textContent:"", contains:() => false,
    querySelector(sel){ return sel === ".livebar" ? this.children[0] || null : null; },
    appendChild(el){ this.children.push(el); } };
  Object.defineProperty(region, "textContent", {
    get(){ return this.children.map(c => c.text()).join(""); },
    set(v){ if (v === "") this.children = []; },
  });
  const el = tag => {
    const kids = [];
    return { tag, className:"", textContent:"", type:"", onclick:null,
      append(...k){ kids.push(...k); }, kids,
      querySelector(sel){ return kids.find(k => "." + k.className === sel) || null; },
      text(){ return kids.length ? kids.map(k => k.textContent).join(" ") : this.textContent; } };
  };
  const dom = { dialog:false, bulkbar:false, focus:null, ...opts };
  return {
    region, dom,
    document:{
      hidden:false,
      get activeElement(){ return dom.focus; },
      body:{},
      createElement:el,
      querySelector(sel){
        if (sel.includes("dialog[open]")) return dom.dialog ? {} : null;
        if (sel === "#view .bulkbar") return dom.bulkbar ? {} : null;
        return null;
      },
      querySelectorAll(){ return []; },
    },
    $:id => id === "livebar" ? region : id === "topbar" ? { offsetHeight:48 } : null,
    barText:() => region.textContent,
  };
}

function liveContext(opts = {}){
  const p = page(opts.dom);
  const reloads = [], badges = [], timers = [];
  const ctx = loadFunctions(["canRerender", "liveConcerns", "liveFlush", "liveRefresh", "liveDefer",
    "liveWho", "liveBar", "liveBarHide", "liveQueue", "myId"], {
    LIVE:liveState(), LIVE_EDIT_PAGES:["detail", "settings", "admin", "tool"], LIVE_BATCH_MS:500,
    LIVE_GAP_MS:3000,
    ME:{ user:{ id:ME_ID } }, AUTH_BUSY:false, AUTO_REFRESH:!!opts.auto,
    ROUTE:{ name:opts.route || "items" }, DETAIL:{ id:ITEM, data:{ versions:[{ id:VERSION }] } },
    NC:{ active:false }, UMAP:{ [PAT]:{ display_name:"Pat" } },
    CUR_RELOAD:() => { reloads.push(1); return Promise.resolve(); },
    refreshApprovalsBadge:() => badges.push(1),
    render:() => reloads.push("render"),
    document:p.document, $:p.$,
    setTimeout:(fn, ms) => { timers.push({ fn, ms }); return timers.length; }, clearTimeout(){},
    setInterval:() => 0, clearInterval(){},
  });
  return { ctx, p, reloads, badges, timers };
}

test("the stream parser handles split chunks, comments, several data lines and CRLF", () => {
  const ctx = loadFunctions(["sseParse"], {});
  // Objects made inside the vm context have its prototypes: compare them as plain data.
  const sseParse = buf => JSON.parse(JSON.stringify(ctx.sseParse(buf)));
  let r = sseParse(": ping\n\nevent: ready\ndata: {\"last_id\":5}\n\nevent: cha");
  assert.deepEqual(r.events, [{ event:"ready", data:"{\"last_id\":5}", id:null }]);
  assert.equal(r.rest, "event: cha");
  r = sseParse(r.rest + "nge\nid: 6\ndata: a\ndata: b\n\n");
  assert.deepEqual(r.events, [{ event:"change", data:"a\nb", id:"6" }]);
  assert.equal(r.rest, "");
  r = sseParse("event: resync\r\ndata: {}\r\n\r\n");
  assert.deepEqual(r.events, [{ event:"resync", data:"{}", id:null }]);
  assert.deepEqual(sseParse(":only a comment\n\n").events, [], "a heartbeat is not an event");
});

test("an item page cares about its own item, its versions and its jobs only", () => {
  const { ctx } = liveContext({ route:"detail" });
  const c = h => ctx.liveConcerns(h);
  assert.equal(c({ action:"item.updated", object_type:"item", object_id:ITEM }), true);
  assert.equal(c({ action:"item.updated", object_type:"item", object_id:OTHER }), false);
  assert.equal(c({ action:"version.regenerate_preview", object_type:"version", object_id:VERSION }), true);
  assert.equal(c({ action:"job.finished", object_type:"job", object_id:OTHER, item_id:ITEM }), true);
  assert.equal(c({ action:"job.finished", object_type:"job", object_id:OTHER, item_id:OTHER }), false);
});

test("lists care about what they show; sign-ins and job starts don't refresh item lists", () => {
  const { ctx } = liveContext({ route:"items" });
  const c = h => ctx.liveConcerns(h);
  assert.equal(c({ action:"item.created", object_type:"item", object_id:OTHER }), true);
  assert.equal(c({ action:"user.login", object_type:"user", object_id:PAT }), false);
  assert.equal(c({ action:"job.started", object_type:"job", item_id:OTHER }), false);
  assert.equal(c({ action:"job.finished", object_type:"job", item_id:OTHER }), true, "a preview arrived");
  assert.equal(c({ object_type:"*" }), true, "too much missed: everything");
  ctx.ROUTE = { name:"jobs" };
  assert.equal(c({ action:"job.started", object_type:"job" }), true);
  assert.equal(c({ action:"item.updated", object_type:"item" }), false);
  ctx.ROUTE = { name:"laser-nests" };
  assert.equal(c({ action:"job.finished", object_type:"job", nest_id:OTHER }), true);
  assert.equal(c({ action:"job.finished", object_type:"job", item_id:OTHER }), false);
  ctx.ROUTE = { name:"settings" };
  assert.equal(c({ action:"item.updated", object_type:"item" }), false);
});

test("with Auto-refresh off, someone else's change shows the Refresh bar and nothing reloads", () => {
  const { ctx, p, reloads } = liveContext({ route:"items" });
  ctx.liveQueue({ audit_id:9, actor_id:PAT, action:"item.created", object_type:"item", object_id:OTHER });
  ctx.liveFlush();
  assert.deepEqual(reloads, []);
  assert.match(p.barText(), /^Updated by Pat\. Refresh Dismiss$/);
  assert.equal(ctx.LIVE.lastId, 9, "the cursor follows the hints");
});

test("the user's own change shows no bar", () => {
  const { ctx, p } = liveContext({ route:"detail" });
  ctx.liveQueue({ audit_id:3, actor_id:ME_ID, action:"item.updated", object_type:"item", object_id:ITEM });
  ctx.liveFlush();
  assert.equal(p.barText(), "");
});

test("with Auto-refresh on, a list refreshes once for a burst of changes", async () => {
  const { ctx, p, reloads } = liveContext({ route:"items", auto:true });
  for (const id of [1, 2, 3])
    ctx.liveQueue({ audit_id:id, action:"job.finished", object_type:"job", item_id:OTHER });
  ctx.liveFlush();
  await tick();
  assert.equal(reloads.length, 1);
  assert.equal(p.barText(), "");
});

test("live refreshes are spaced out: the next burst waits for the gap, then refreshes once", async () => {
  const { ctx, reloads, timers } = liveContext({ route:"items", auto:true });
  const burst = id => { ctx.liveQueue({ audit_id:id, actor_id:PAT, action:"item.created", object_type:"item" }); ctx.liveFlush(); };
  burst(1);
  await tick();
  burst(2); burst(3);
  await tick();
  assert.equal(reloads.length, 1, "no second refresh inside the gap");
  const later = timers.filter(t => t.ms >= 250);
  assert(later.length >= 1 && later.every(t => t.ms <= 3000), "one trailing refresh is scheduled");
  ctx.LIVE.lastRefresh = 0;
  later[later.length - 1].fn();
  await tick();
  assert.equal(reloads.length, 2);
});

test("the user's own change from elsewhere (a desktop client) refreshes an opted-in list", async () => {
  const { ctx, p, reloads } = liveContext({ route:"items", auto:true });
  ctx.liveQueue({ audit_id:5, actor_id:ME_ID, action:"item.checkout", object_type:"item" });
  ctx.liveFlush();
  await tick();
  assert.equal(reloads.length, 1);
  assert.equal(p.barText(), "", "but it is never shown as news");
});

test("an item page, a dialog, a focused field or selected rows get the bar instead of a reload", () => {
  const cases = [
    { route:"detail" },
    { route:"items", dom:{ dialog:true } },
    { route:"items", dom:{ focus:{ tagName:"INPUT" } } },
    { route:"items", dom:{ bulkbar:true } },
  ];
  for (const k of cases){
    const { ctx, p, reloads } = liveContext({ ...k, auto:true });
    ctx.liveQueue({ audit_id:4, actor_id:PAT, action:"item.updated", object_type:"item", object_id:ITEM });
    ctx.liveFlush();
    assert.deepEqual(reloads, [], JSON.stringify(k));
    assert.match(p.barText(), /Updated by Pat/, JSON.stringify(k));
  }
});

test("a background job names itself; an unknown user is 'another user'", () => {
  const { ctx } = liveContext();
  assert.equal(ctx.liveWho({ object_type:"job", action:"job.failed" }), "A background job failed");
  assert.equal(ctx.liveWho({ object_type:"job", action:"job.finished" }), "A background job finished");
  assert.equal(ctx.liveWho({ actor_id:OTHER, object_type:"item" }), "Updated by another user");
});

// The stream client, with fetch() answering from a list of chunks.
function streamContext(responses, extra = {}){
  const calls = [], queued = [], timers = [], notices = [];
  let auth = null;
  const ctx = loadFunctions(["liveConnect", "liveStop", "liveRetry", "liveSignedOut", "sseParse",
    "liveJson", "liveStatus", "refreshModeLabel"], {
    LIVE:liveState(), LIVE_FIRST_MS:15000, LIVE_STALL_MS:60000, POLL_MS:6000, AUTO_REFRESH:false,
    ME:{ user:{ id:ME_ID } }, TOKEN:"pdm_session", document:{ hidden:false },
    $:() => null, Math, Date, Number, encodeURIComponent, JSON, TextDecoder, Error,
    AbortController,
    liveQueue:h => queued.push(h),
    signedOutNotice:() => notices.push(1), REAUTH:null, ...extra,
    setTimeout:(fn, ms) => { timers.push({ fn, ms }); return timers.length; }, clearTimeout(){},
    setInterval:(fn, ms) => { if (ms === 2000 && !auth && responses.authCheck) auth = fn; return 1; },
    clearInterval(){},
    fetch:async (url, opts) => {
      calls.push({ url, auth:opts.headers.Authorization });
      const r = responses.shift();
      if (r instanceof Error) throw r;
      const chunks = (r.chunks || []).map(c => new TextEncoder().encode(c));
      return { ok:r.status === 200, status:r.status, body:{ getReader:() => ({
        read:async () => chunks.length ? { value:chunks.shift(), done:false } : { done:true } }) } };
    },
  });
  return { ctx, calls, queued, timers, notices };
}

test("the stream sends the bearer header, goes live on ready and queues changes", async () => {
  const s = streamContext([{ status:200, chunks:[
    "event: ready\ndata: {\"last_id\":40}\n\n",
    ": ping\n\nevent: change\nid: 41\ndata: {\"audit_id\":41,\"object_type\":\"item\"}\n\n"] }]);
  await s.ctx.liveConnect();
  assert.deepEqual(s.calls, [{ url:"/events/stream", auth:"Bearer pdm_session" }]);
  assert.equal(s.ctx.LIVE.lastId, 40);
  assert.deepEqual(s.queued.map(h => h.audit_id), [41]);
  // The stream ended (server restart): try again later, picking up where it left off.
  assert.equal(s.ctx.LIVE.state, "down");
  const retry = s.timers.find(t => t.ms >= 1000);
  assert(retry, "a reconnect is scheduled");
});

test("a reconnect asks for what it missed; resync reconnects at once", async () => {
  const s = streamContext([
    { status:200, chunks:["event: ready\ndata: {\"last_id\":7}\n\nevent: resync\ndata: {}\n\n"] },
  ]);
  await s.ctx.liveConnect();
  const now = s.timers.find(t => t.ms === 0);
  assert(now, "resync reconnects without waiting");
  s.ctx.LIVE.lastId = 12;
  const more = streamContext([{ status:200, chunks:[] }]);
  more.ctx.LIVE.lastId = 12;
  await more.ctx.liveConnect();
  assert.equal(more.calls[0].url, "/events/stream?since=12");
});

test("an ended sign-in pauses live updates without asking, until a new sign-in", async () => {
  for (const r of [{ status:401 }, { status:200, chunks:["event: ready\ndata: {}\n\nevent: expired\ndata: {}\n\n"] }]){
    const s = streamContext([r]);
    await s.ctx.liveConnect();
    assert.equal(s.ctx.LIVE.state, "signedout");
    assert.equal(s.timers.length, 0, "no reconnect with the same token");
    assert.equal(s.notices.length, 1, "a message with a Sign in button, not the sign-in dialog");
  }
});

test("a server that cannot be reached is retried with a growing pause", async () => {
  const s = streamContext([new TypeError("Failed to fetch"), new TypeError("Failed to fetch")]);
  await s.ctx.liveConnect();
  const first = s.timers.pop().ms;
  await s.ctx.liveConnect();
  const second = s.timers.pop().ms;
  assert(first >= 1500 && first <= 2500, "first pause about 2 s, got " + first);
  assert(second >= 3000 && second <= 5000, "then about 4 s, got " + second);
  assert.equal(s.ctx.LIVE.state, "down");
});

test("closing the bar from the keyboard puts focus on the page heading, not on <body>", () => {
  let focused = null;
  const heading = { hasAttribute:() => false, focus(){ focused = this; } };
  const view = { querySelector:() => heading };
  const region = { textContent:"x", contains:() => true };
  const ctx = loadFunctions(["liveBarHide"], {
    LIVE:liveState(), clearInterval(){}, document:{ activeElement:{} },
    $:id => id === "livebar" ? region : id === "view" ? view : null,
  });
  ctx.liveBarHide();
  assert.equal(region.textContent, "");
  assert.equal(focused, heading);
  assert.equal(heading.tabIndex, -1);
});

test("the six-second refresh stays on pages live updates do not cover, and stops while signed out", () => {
  const p = page();
  const reloads = [], timers = [];
  const ctx = loadFunctions(["startPolling", "canRerender"], {
    LIVE:{ ...liveState(), state:"live" }, LIVE_EDIT_PAGES:["detail", "settings", "admin", "tool"],
    LIVE_ROUTES:["items", "dashboard"], AUTO_REFRESH:true, ME:{ user:{} }, TOKEN:"t", AUTH_BUSY:false,
    POLL_MS:6000, ROUTE:{ name:"items" }, NC:{ active:false }, document:p.document,
    CUR_RELOAD:() => { reloads.push(ctx.ROUTE.name); return Promise.resolve(); },
    refreshApprovalsBadge(){}, Promise,
    setInterval:(fn, ms) => timers.push({ fn, ms }), fetch:() => Promise.resolve({}),
  });
  ctx.startPolling();
  const poll = timers.find(t => t.ms === 6000).fn;
  poll();                                        // live, and the stream covers Items
  ctx.ROUTE = { name:"tools" }; poll();          // nothing about tools is in the activity log
  ctx.LIVE.state = "down"; ctx.ROUTE = { name:"items" }; poll();   // fallback while not live
  ctx.LIVE.state = "signedout"; poll();          // waits for the user to sign in again
  assert.deepEqual(reloads, ["tools", "items"]);
});

test("a sign-in that ended after the user already signed in again: no message, the stream carries on", async () => {
  const s = streamContext([{ status:200, chunks:["event: ready\ndata: {}\n\nevent: expired\ndata: {}\n\n"] }]);
  const read = s.ctx.liveConnect();
  s.ctx.TOKEN = "pdm_new_session";   // the in-place sign-in finished while the old stream ran
  await read;
  assert.equal(s.notices.length, 0);
  assert.notEqual(s.ctx.LIVE.state, "signedout");
  assert(s.timers.some(t => t.ms === 0), "reconnects with the new sign-in at once");
});

test("no signed-out message while the sign-in dialog is already open", async () => {
  const s = streamContext([{ status:401 }], { REAUTH:Promise.resolve(false) });
  await s.ctx.liveConnect();
  assert.equal(s.ctx.LIVE.state, "signedout");
  assert.equal(s.notices.length, 0);
});
