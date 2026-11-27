// "Design change or minor update?" at web check-in.
// Run with: node --test crates/pdm-api/tests/web-ui-checkin-new-version.test.cjs
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
// Arrays made inside a vm context have their own prototype; compare plain copies.
const plain = x => JSON.parse(JSON.stringify(x));

// With the check-in safety change, bytes go up with XMLHttpRequest and the body is built by
// commitVersion; the base is the version the page shows.
class FakeXHR {
  constructor(){ this.upload = {}; }
  open(){} setRequestHeader(){}
  send(){ setImmediate(() => { this.status = 200; this.responseText = JSON.stringify({ blob: { sha256: "abc", size_bytes: 3 } }); this.onload(); }); }
}
async function checkinBody(newVersion){
  let body;
  const ctx = loadFunctions(["checkinItem", "commitVersion", "checkinPrecheck", "checkinRefusal", "uploadBlob",
    "newerVersionError", "lockHeldError", "clockTime"], {
    api: async (method, url, b) => {
      if (method === "GET") return { head_version: { seq: 4, version: 1, revision: 4 }, lock: null };
      if (url.endsWith("/checkin")){ body = b; return { version: { seq: 5, version: 2, revision: 1 } }; }
      return {};
    },
    myId: () => "me", lockName: () => "x", fmtTime: () => "", location: { hostname: "t" }, TOKEN: "t",
    XMLHttpRequest: FakeXHR, ALLITEMS: [], ALLITEMS_AT: 0, JSON, Date, String, Object, Error, Promise,
  });
  await ctx.checkinItem("item", { name: "a.step", type: "", size: 3 }, "m", "a.step", 4, null, newVersion);
  return body;
}

test("a design change is checked in as a new version; anything else as a revision", async () => {
  assert.equal((await checkinBody(true)).new_version, true);
  assert.equal((await checkinBody(false)).new_version, false);
  assert.equal((await checkinBody(undefined)).new_version, false, "callers that do not ask keep today's behaviour");
});

test("the question names both outcomes with their numbers and preselects neither", () => {
  const ctx = loadFunctions(["changeKindField"], {});
  const field = plain(ctx.changeKindField({ seq: 4, version: 1, revision: 4 }));
  assert.equal(field.required, true);
  assert.equal(field.value, "", "no default: the user answers");
  assert.deepEqual(field.options, [
    { value: "", label: "Choose one" },
    { value: "design", label: "Design change: V2 - R1" },
    { value: "minor", label: "Minor update: V1 - R5" },
  ]);
  const unknown = plain(ctx.changeKindField({ seq: 4 }));
  assert.deepEqual(unknown.options.map(o => o.label), ["Choose one", "Design change", "Minor update"]);
});

test("versions show their major version and revision", () => {
  const ctx = loadFunctions(["versionRevLabel"], {});
  assert.equal(ctx.versionRevLabel({ seq: 5, version: 2, revision: 1 }), "V2 - R1");
  assert.equal(ctx.versionRevLabel({ seq: 5 }), "");
  assert.equal(ctx.versionRevLabel(null), "");
});
