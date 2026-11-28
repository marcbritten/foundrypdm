// Run with: node --test crates/pdm-api/tests/web-ui-sec-operator.test.cjs
// The operator role: the Users dialogs offer it with a one-line description of each role, and the
// portal ranks roles the way the server does.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const app = fs.readFileSync(path.join(__dirname, "../static/app.js"), "utf8");
const routes = fs.readFileSync(path.join(__dirname, "../src/routes.rs"), "utf8");

function loadFunctions(names, globals){
  const context = vm.createContext(globals);
  for (const name of names){
    const start = app.indexOf("function " + name + "(");
    assert(start >= 0, "missing function " + name);
    const asyncStart = app.slice(start - 6, start) === "async " ? start - 6 : start;
    const rest = app.slice(start);
    // Ends at the next function or section marker (isRole is followed by the state block).
    const end = rest.search(/\n(?:async )?function |\n\/\/ ──/);
    vm.runInContext(app.slice(asyncStart, end < 0 ? app.length : start + end), context);
  }
  return context;
}

// The server's ranks, read from routes.rs: [["viewer", 0], ...].
const serverRanks = [...routes.match(/const ROLE_LEVEL[^=]*=\s*&\[([^\]]*)\]/)[1]
  .matchAll(/\("(\w+)",\s*(\d+)\)/g)].map(m => [m[1], Number(m[2])]);

test("the server ranks viewer < operator < editor < admin", () => {
  assert.deepEqual(serverRanks, [["viewer", 0], ["operator", 1], ["editor", 2], ["admin", 3]]);
});

test("the role picker lists every role in rank order, each with a one-line description", () => {
  const { roleOptions } = loadFunctions(["roleOptions"], {});
  const opts = roleOptions();
  assert.deepEqual([...opts].map(o => o.value), serverRanks.map(([r]) => r));
  for (const o of opts){
    const [name, desc] = o.label.split(" — ");
    assert.equal(name.toLowerCase(), o.value, o.label);
    assert.ok(desc && desc.length > 10 && !desc.includes("\n"), "description for " + o.value);
  }
  assert.match(opts.find(o => o.value === "operator").label, /job status and inspections/);
});

test("Add user, Invite user and Edit user all offer the operator role", async () => {
  const seen = [];
  const ctx = loadFunctions(["roleOptions", "addUser", "inviteUser", "editUser"], {
    openModal: async (title, fields) => { seen.push({ title, role: fields.find(f => f.id === "role") }); return null; },
    api: async () => { throw new Error("no request expected"); }, toast(){}, render(){}, myId: () => "me"
  });
  await ctx.addUser();
  await ctx.inviteUser();
  await ctx.editUser({ id:"u1", email:"op@example.test", display_name:"", role:"operator", active:true });
  assert.equal(seen.length, 3);
  for (const { title, role } of seen){
    assert.ok(role, title + " has a role field");
    assert.ok(role.options.some(o => o.value === "operator"), title + " offers operator");
  }
  assert.equal(seen[0].role.value, "editor", "new users still default to editor");
  assert.equal(seen[2].role.value, "operator", "an operator's own role is preselected");
});

test("the portal ranks roles the way the server does", () => {
  const ctx = loadFunctions(["isRole"], {});
  const can = (role, min) => { ctx.ME = { user: { role } }; return ctx.isRole(min); };
  for (const [role, rank] of serverRanks){
    for (const [min, need] of serverRanks){
      assert.equal(can(role, min), rank >= need, role + " vs " + min);
    }
  }
  ctx.ME = null;
  assert.equal(ctx.isRole("operator"), false, "signed out counts as a viewer");
});
