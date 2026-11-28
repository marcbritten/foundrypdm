// Settings card for the release approval setting.
// Run with: node --test crates/pdm-api/tests/web-ui-release-approval.test.cjs
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
    const end = rest.search(/\n(?:async )?function /);
    vm.runInContext(app.slice(asyncStart, end < 0 ? app.length : start + end), context);
  }
  return context;
}

function harness(api, canEdit){
  const els = new Map();
  const $ = id => { if (!els.has(id)) els.set(id, { id, checked:false, disabled:false, textContent:"" }); return els.get(id); };
  const box = { innerHTML:"" };
  const notes = [], calls = [];
  const c = loadFunctions(["releaseApprovalCard"], {
    $, esc: s => String(s), toast: (m, k) => notes.push([m, k]),
    render: () => assert.fail("the card redrew the page"),
    api: async (method, url, body) => { calls.push({ method, url, body }); return api(method, url, body); } });
  return { c, $, box, notes, calls, run: () => c.releaseApprovalCard(box, canEdit) };
}

test("the Settings page has a release approval control that says its default", async () => {
  assert.match(app, /releaseApprovalCard\(\$\("release-approval"\), canEdit\)/, "viewSettings does not show the card");
  const h = harness(async () => ({ require_approval_on_release:false }), true);
  await h.run();
  assert.match(h.box.innerHTML, /id='ra-on' type='checkbox'/);
  assert.doesNotMatch(h.box.innerHTML, /id='ra-on' type='checkbox' checked/);
  assert.match(h.box.innerHTML, /Default: off\./);
  assert.equal(h.calls[0].url, "/settings/release-approval");
});

test("saving the release approval sends the checkbox and shows it saved in place", async () => {
  const h = harness(async (m, u, body) => m === "PUT" ? { require_approval_on_release: body.require_approval_on_release }
    : { require_approval_on_release:false }, true);
  await h.run();
  h.$("ra-on").checked = true;
  await h.$("ra-save").onclick();
  const put = h.calls.find(x => x.method === "PUT");
  assert.equal(JSON.stringify(put.body), JSON.stringify({ require_approval_on_release:true }));
  assert.equal(h.$("ra-state").textContent, "Saved");
  assert.equal(h.$("ra-save").disabled, false);
});

test("a failed save says so and keeps the choice", async () => {
  const h = harness(async m => { if (m === "PUT") throw new Error("admin only"); return { require_approval_on_release:false }; }, true);
  await h.run();
  h.$("ra-on").checked = true;
  await h.$("ra-save").onclick();
  assert.deepEqual(h.notes[0], ["Release approval not saved: admin only", "err"]);
  assert.equal(h.$("ra-on").checked, true);
});

test("members who are not admins see the setting but cannot change it", async () => {
  const h = harness(async () => ({ require_approval_on_release:true }), false);
  await h.run();
  assert.match(h.box.innerHTML, /checked disabled/);
  assert.doesNotMatch(h.box.innerHTML, /ra-save/);
});

test("a failed load says so and offers to try again", async () => {
  let fail = true;
  const h = harness(async () => { if (fail) throw new Error("network error"); return { require_approval_on_release:true }; }, true);
  await h.run();
  assert.match(h.box.innerHTML, /not loaded: network error/);
  fail = false;
  await h.$("ra-retry").onclick();
  assert.match(h.box.innerHTML, /checked/);
});
