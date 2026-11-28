// Run with: node --test crates/pdm-api/tests/web-ui-sec-sysinfo.test.cjs
// The System page shows what describes the whole server only to host admins; everyone else
// gets the build, CAD CLI version, GPU policy and their own organization's counts, and
// "Only a host admin can see ..." where the rest would be.
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
const own = { projects:3, items:7, files:9, revisions:2 };
const answers = {
  org: {
    host_details:false, pdm:{ version:"9.9.9", build:"T-1", update_available:true, latest:"9.9.10" }, solidcraft_cli:{ present:true, version:"cli 1.2.3" },
    gpu:{ policy:"auto" }, counts:own,
  },
  host: {
    host_details:true, pdm:{ version:"9.9.9", build:"T-1", update_available:true, latest:"9.9.10", releases_url:"https://updates.example/rel" },
    solidcraft_cli:{ present:true, version:"cli 1.2.3", path:"/opt/bin/cad-cli" },
    gpu:{ policy:"auto", survey:{ lands_on:"Test adapter", backends:["vulkan"] } },
    postgres:{ version:"PostgreSQL 16", ok:true }, storage:{ kind:"local" }, uptime_seconds:90,
    counts:{ organizations:4, users:12, ...own },
  },
};

async function renderSystem(kind, meHost){
  const v = { innerHTML:"" }, els = {};
  const ctx = loadFunctions(["viewSystem", "isHostAdmin", "hostOnlyNote", "hostSeeNote", "fmtUptime"], {
    ME:{ host_admin:meHost === undefined ? kind === "host" : meHost, user:{ role:"admin", active:true } }, esc, toast(){},
    $:id => (els[id] = els[id] || {}),
    api:async (method, url) => {
      if (url === "/system-info") return answers[kind];
      if (url === "/settings/dutchman") return kind === "host"
        ? { path:"/opt/bin/dm", settings:{ channel:"latest" }, channels:[{ value:"latest", label:"Latest" }] }
        : { path:null, settings:{ channel:"latest" }, channels:[{ value:"latest", label:"latest" }] };
      if (url === "/settings/cad-cli") return kind === "host" ? { path:"/opt/bin/cad-cli", latest:"cli 9" } : { path:null, latest:null };
      return {};
    },
  });
  ctx.viewSystem(v);
  for (let i = 0; i < 20 && !v.innerHTML.includes("Counts"); i++) await new Promise(r => setImmediate(r));
  return v.innerHTML;
}

test("an org admin, editor or viewer sees the build and own counts, not the server", async () => {
  const html = await renderSystem("org");
  for (const kept of ["T-1", "cli 1.2.3", "Projects (this org)", ">7<", "Policy"])
    assert.ok(html.includes(kept), "keeps " + kept);
  for (const hidden of ["PostgreSQL", "Object storage", "Organizations", "Users<", "Uptime", "Binary path", "Lands on",
                        "Test adapter", "/opt/bin", "GitHub latest", "<a href='https://updates"])
    assert.ok(!html.includes(hidden), "hides " + hidden);
  assert.ok(html.includes("available 9.9.10"), "says an update exists, without the link");
  assert.ok(html.includes("not installed — pulled on demand"), "Dutchman state without the path");
  assert.match(html, /Infrastructure<\/h2><p class='dim host-only'>Only a host admin can see this\.<\/p>/);
  assert.match(html, /Only a host admin can see which hardware the renderer uses\./);
  assert.match(html, /Only a host admin can see the server-wide counts\./);
});

test("a host admin sees everything and no 'can see' notes", async () => {
  const html = await renderSystem("host");
  for (const shown of ["PostgreSQL 16", "Object storage", "Organizations", ">12<", "Uptime", "Binary path", "/opt/bin/cad-cli",
                       "/opt/bin/dm", "Test adapter", "GitHub latest", "<a href='https://updates.example/rel'"])
    assert.ok(html.includes(shown), "shows " + shown);
  assert.doesNotMatch(html, /id='(cli-upd|dm-upd|scweb-upd)' class='primary' disabled/);
  assert.doesNotMatch(html, /Only a host admin can see/);
});

test("the page follows the server's answer, not the browser's idea of the role", async () => {
  // A stale ME that still says host admin reveals nothing the server left out and does not
  // offer the host admin's controls.
  const html = await renderSystem("org", true);
  assert.match(html, /Infrastructure<\/h2><p class='dim host-only'>Only a host admin can see this\.<\/p>/);
  for (const id of ["cli-upd", "dm-upd", "scweb-upd"])
    assert.match(html, new RegExp("id='" + id + "' class='primary' disabled>"), id);
  assert.match(html, /<select id='dm-channel' disabled>/);
  assert.equal((html.match(/Only a host admin can change this/g) || []).length, 3);
});
