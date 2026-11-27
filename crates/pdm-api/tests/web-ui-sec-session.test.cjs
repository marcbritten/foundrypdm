// Run with: node --test crates/pdm-api/tests/web-ui-sec-session.test.cjs
// A saved web sign-in the server refuses (expired) sends the person back to sign-in with a
// plain message.
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
    // Ends at the next function or section marker (startPolling is followed by the init block).
    const end = rest.search(/\n(?:async )?function |\n\/\/ ──/);
    vm.runInContext(app.slice(asyncStart, end < 0 ? app.length : start + end), context);
  }
  return context;
}

function reconnect(apiError){
  const toasts = [], store = new Map([["pdm_token", "pdm_saved"]]);
  // The sign-in view starts hidden when a token is saved (index.html, init).
  const loginClasses = new Set(["hidden"]);
  const loginView = { classList:{ remove:c => loginClasses.delete(c), add:c => loginClasses.add(c) } };
  let entered = false;
  const context = loadFunctions(["connectFromToken"], {
    TOKEN:"pdm_saved", ME:null,
    $:id => id === "login-view" ? loginView : { value:"" },
    localStorage:{
      getItem:k => store.has(k) ? store.get(k) : null,
      setItem:(k, v) => store.set(k, v),
      removeItem:k => store.delete(k),
    },
    authBusy(){},
    toast:(msg, kind) => toasts.push({ msg, kind }),
    api:async () => { throw apiError; },
    enterWorkspace:() => { entered = true; },
  });
  return { context, toasts, store, entered:() => entered, signInShown:() => !loginClasses.has("hidden") };
}

test("an expired saved sign-in says so and asks to sign in again", async () => {
  const e = new Error("session expired — please sign in again"); e.status = 401;
  const r = reconnect(e);
  await r.context.connectFromToken();
  assert.deepEqual(r.toasts, [{ msg:"Your session expired — sign in again", kind:"err" }]);
  assert.equal(r.store.has("pdm_token"), false, "the dead token is forgotten");
  assert.equal(r.context.TOKEN, "");
  assert.equal(r.entered(), false);
  assert.equal(r.signInShown(), true, "the sign-in form is shown, not a blank page");
});

function call401(editorOpen){
  let reloaded = false;
  const context = loadFunctions(["api"], {
    TOKEN:"pdm_session", ME:{ user:{} }, AUTH_BUSY:false,
    fetch:async () => ({ ok:false, status:401,
      headers:{ get:() => "application/json" },
      json:async () => ({ error:{ code:"session_expired", message:"your session expired — sign in again" } }) }),
    location:{ reload(){ reloaded = true; } },
    document:{ querySelector:sel => editorOpen && sel === "dialog.web-editor[open]" ? {} : null },
  });
  return { context, reloaded:() => reloaded };
}

test("an expired sign-in while a web editor is open keeps the page (and the unsaved model)", async () => {
  const r = call401(true);
  await assert.rejects(r.context.api("POST", "/items/i/checkin", {}), e => e.status === 401);
  assert.equal(r.reloaded(), false, "reloading would throw away the open editor's work");
});

test("an expired sign-in with no editor open reloads to the sign-in form", async () => {
  const r = call401(false);
  await assert.rejects(r.context.api("GET", "/projects"), e => e.status === 401);
  assert.equal(r.reloaded(), true);
});

test("an open web editor in a visible tab keeps the sign-in in use", () => {
  const timers = [], calls = [];
  let editorOpen = true, hidden = false;
  const context = loadFunctions(["startPolling"], {
    AUTO_REFRESH:false, ME:{ user:{} }, TOKEN:"pdm_session", AUTH_BUSY:false, POLL_MS:6000,
    setInterval:(fn, ms) => timers.push({ fn, ms }),
    fetch:(url, opts) => { calls.push({ url, auth:opts.headers.Authorization }); return Promise.resolve({}); },
    document:{
      get hidden(){ return hidden; },
      querySelector:sel => editorOpen && sel === "dialog.web-editor[open]" ? {} : null,
    },
  });
  context.startPolling();
  const ping = timers.find(t => t.ms < 60000 && t.ms !== 6000);
  assert(ping, "a keep-alive timer shorter than a minute");
  ping.fn();
  assert.deepEqual(calls, [{ url:"/me", auth:"Bearer pdm_session" }]);
  editorOpen = false; ping.fn();
  editorOpen = true; hidden = true; ping.fn();
  assert.equal(calls.length, 1, "no ping without an open editor in a visible tab");
});

test("a saved sign-in that fails another way keeps the reason", async () => {
  const r = reconnect(new Error("network error — is the server reachable?"));
  await r.context.connectFromToken();
  assert.deepEqual(r.toasts, [{
    msg:"Saved session ended (network error — is the server reachable?) — sign in again", kind:"err",
  }]);
  assert.equal(r.signInShown(), true);
});
