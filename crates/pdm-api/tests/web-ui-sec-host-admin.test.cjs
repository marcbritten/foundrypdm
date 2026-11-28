// Run with: node --test crates/pdm-api/tests/web-ui-sec-host-admin.test.cjs
// Host admin controls in the portal: server-wide settings are read-only for org admins, and
// a host admin's account buttons are locked for anyone who is not a host admin.
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

const esc = s => String(s).replace(/[&<>"']/g, c => ({ "&":"&amp;", "<":"&lt;", ">":"&gt;", '"':"&quot;", "'":"&#39;" }[c]));
const admin = { role:"admin", active:true };
const load = ME => loadFunctions(["isHostAdmin", "hostOnlyNote", "hostLock", "gpuPickerHTML", "usersTable"], {
  ME, esc, userAv:() => "",
  GPU_POLICIES:[{ id:"auto", name:"Auto", desc:"" }, { id:"cpu", name:"CPU", desc:"" }],
});

test("isHostAdmin follows GET /me, and the login reply's user right after sign-in", () => {
  assert.equal(load(null).isHostAdmin(), false);
  assert.equal(load({ host_admin:true, user:admin }).isHostAdmin(), true);
  assert.equal(load({ host_admin:false, user:{ ...admin, is_host_admin:true } }).isHostAdmin(), false);
  assert.equal(load({ user:{ ...admin, is_host_admin:true } }).isHostAdmin(), true);
  assert.equal(load({ user:{ ...admin, is_host_admin:false } }).isHostAdmin(), false);
  assert.equal(load({ user:{ role:"editor", active:true, is_host_admin:true } }).isHostAdmin(), false);
});

test("server-wide settings are read-only with a note for an org admin", () => {
  const org = load({ host_admin:false, user:admin });
  assert.match(org.hostOnlyNote(), /Only a host admin can change this/);
  const html = org.gpuPickerHTML("auto", org.isHostAdmin());
  assert.equal((html.match(/ disabled>/g) || []).length, 2);
  const host = load({ host_admin:true, user:admin });
  assert.doesNotMatch(host.hostOnlyNote(), /Only a host admin/);
  assert.match(host.hostOnlyNote(), /applies to the whole server/);
  assert.doesNotMatch(host.gpuPickerHTML("auto", host.isHostAdmin()), /disabled/);
});

test("the System page disables the shared update controls for non-host admins", () => {
  const sys = app.slice(app.indexOf("function viewSystem("), app.indexOf("async function viewSettings("));
  for (const id of ["cli-upd", "dm-upd", "scweb-upd"])
    assert.match(sys, new RegExp("id='" + id + "' class='primary'\" \\+ hostDis \\+"), id);
  assert.match(sys, /<select id='dm-channel'" \+ hostDis \+/);
  assert.equal((sys.match(/hostOnlyNote\(\)/g) || []).length, 3);
});

const users = [
  { id:"h", email:"h@x", role:"admin", active:true, is_host_admin:true, created_at:"" },
  { id:"a", email:"a@x", role:"admin", active:true, is_host_admin:false, created_at:"" },
  { id:"e", email:"e@x", role:"editor", active:true, is_host_admin:false, created_at:"" },
];
const row = (html, id) => html.split("<tr>").find(r => r.includes("data-edit='" + id + "'"));

test("an org admin sees who is host admin but cannot touch that account or grant it", () => {
  const html = load({ host_admin:false, user:admin }).usersTable(users);
  assert.match(row(html, "h"), /host admin<\/span>/);
  assert.equal((row(html, "h").match(/ disabled title=/g) || []).length, 4);
  // The reason is also visible text, not only a tooltip on disabled buttons.
  assert.match(html, /<\/table><p class='dim'>Only a host admin can change a host admin's account\.<\/p>$/);
  assert.doesNotMatch(row(html, "a"), /disabled/);
  assert.doesNotMatch(html, /data-hostadm/);
});

test("a host admin can grant host admin to an active admin and revoke it", () => {
  const html = load({ host_admin:true, user:admin }).usersTable(users);
  assert.doesNotMatch(html, /disabled/);
  assert.match(row(html, "h"), /data-hostadm='h'>Revoke host admin</);
  assert.match(row(html, "a"), /data-hostadm='a'>Make host admin</);
  assert.doesNotMatch(row(html, "e"), /data-hostadm/);
  assert.match(html, /<\/table>$/);
});

test("granting and revoking use the app's dialog, and revoking yourself drops your host controls", async () => {
  for (const [target, me, body, hostAfter] of [
    [{ id:"a", email:"a@x", is_host_admin:false }, "h", { host_admin:true }, true],
    [{ id:"h", email:"h@x", is_host_admin:true }, "h", { host_admin:false }, false],
  ]) {
    const calls = [], dialogs = [];
    let rendered = 0;
    const ME = { host_admin:true, user:{ id:me, ...admin } };
    const ctx = loadFunctions(["toggleHostAdmin"], {
      ME, esc, myId:() => ME.user.id, render:() => { rendered++; },
      api:async (method, url, b) => { calls.push([method, url, b]); return {}; },
      openModal:async (title, fields, opt, note) => { dialogs.push({ title, opt, note }); return opt.onSubmit({}); },
    });
    await ctx.toggleHostAdmin(target);
    assert.equal(JSON.stringify(calls), JSON.stringify([["PUT", "/users/" + target.id + "/host-admin", body]]));
    assert.equal(dialogs.length, 1);
    assert.equal(dialogs[0].opt.danger, !body.host_admin);
    assert.equal(ME.host_admin, hostAfter);
    assert.equal(rendered, 1);
  }
  assert.doesNotMatch(app.slice(app.indexOf("async function toggleHostAdmin("), app.indexOf("async function toggleUserActive(")), /confirm\(/);
});
