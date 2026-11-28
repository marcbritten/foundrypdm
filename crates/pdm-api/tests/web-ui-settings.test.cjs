// Settings, Administration, sign-in and tool-editor behaviour of the web portal.
// Run with: node --test crates/pdm-api/tests/web-ui-settings.test.cjs
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const app = fs.readFileSync(path.join(__dirname, "../static/app.js"), "utf8");
const html = fs.readFileSync(path.join(__dirname, "../static/index.html"), "utf8");
const core = fs.readFileSync(path.join(__dirname, "../../pdm-core/src/lib.rs"), "utf8");

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
// A top-level `const NAME = …;` from app.js, evaluated on its own.
function constValue(name){
  const m = app.match(new RegExp("\\nconst " + name + " = ([\\s\\S]*?\\]);\\r?\\n"));
  assert(m, "missing const " + name);
  return vm.runInNewContext("(" + m[1] + ")");
}

// Minimal DOM stand-ins: enough for handlers that read values and set text.
function classList(){
  const s = new Set();
  return { add: c => s.add(c), remove: c => s.delete(c), contains: c => s.has(c),
    toggle: (c, on) => { if (on === undefined ? !s.has(c) : on) s.add(c); else s.delete(c); return s.has(c); } };
}
function stubEl(id){
  return { id, value: "", checked: false, disabled: false, hidden: false, type: "text", textContent: "",
    innerHTML: "", dataset: {}, style: {}, classList: classList(), attrs: {},
    addEventListener(){}, focus(){}, select(){}, remove(){}, closest(){ return null; },
    querySelector(){ return null; }, querySelectorAll(){ return []; },
    setAttribute(k, v){ this.attrs[k] = v; }, getAttribute(k){ return this.attrs[k]; } };
}
function stubDom(){
  const els = new Map();
  const $ = id => { if (!els.has(id)) els.set(id, stubEl(id)); return els.get(id); };
  return { $, els };
}
const flush = () => new Promise(r => setImmediate(r));

// ── Laser-nest defaults (Part source unit was never saved) ──────────────

function nestDefaultsKeys(){
  const body = core.slice(core.indexOf("pub struct NestDefaults"));
  const fields = body.slice(0, body.indexOf("\n}")).match(/pub (\w+):/g).map(f => f.slice(4, -1));
  return fields.map(f => f.replace(/_([a-z])/g, (_, c) => c.toUpperCase()));
}

test("nest defaults are sent with the server's camelCase keys, Part source unit included", () => {
  const keys = nestDefaultsKeys();
  assert(keys.includes("partUnit"), "NestDefaults has part_unit (serde camelCase: partUnit)");
  const c = loadFunctions(["nestDefaultsBody"], { NEST_LEN_FIELDS: constValue("NEST_LEN_FIELDS") });
  const { body, bad } = c.nestDefaultsBody({ units:"mm", partUnit:"mm", width:"1200", length:"",
    margin:" 10 ", spacingX:"5", spacingY:"5", grainDirection:"along_length", nestingCorner:"lower_left",
    guillotine:false, addPartLabels:true });
  assert.deepEqual([...bad], []);
  for (const k of Object.keys(body)) assert(keys.includes(k), "the server would drop key " + k);
  assert.equal(body.partUnit, "mm");
  assert.equal(body.width, 1200);
  assert.equal(body.length, null);
  assert.equal(body.margin, 10);
});

test("a nest default that is not a number is reported, not saved as blank", () => {
  const c = loadFunctions(["nestDefaultsBody"], { NEST_LEN_FIELDS: constValue("NEST_LEN_FIELDS") });
  const { bad } = c.nestDefaultsBody({ units:"in", partUnit:"auto", width:"48x", margin:"-1" });
  assert.deepEqual([...bad], ["Sheet width", "Sheet margin"]);
});

test("switching nest units keeps sizes and returns the typed value on the way back", () => {
  const c = loadFunctions(["lengthToMm", "mmToText"], {});
  const mm = c.lengthToMm("48", "in");
  assert.equal(c.mmToText(mm, "mm"), "1219.2");
  assert.equal(c.mmToText(mm, "in"), "48");
  assert.equal(c.mmToText(c.lengthToMm("0.39", "in"), "in"), "0.39");
  assert.equal(c.lengthToMm("", "in"), null);
  assert.equal(c.lengthToMm("abc", "mm"), null);
});

test("a blank nest length says what the panel will use, in the unit shown", () => {
  const c = loadFunctions(["nestBlankHint", "ncDefaultState", "lengthToMm", "mmToText"], {});
  const s = c.ncDefaultState();
  assert.equal(s.units, "in", "the panel's built-in values are in inches");
  assert.equal(c.nestBlankHint("width", "in"), "Blank: " + s.sheets[0].width + " in.");
  assert.equal(c.nestBlankHint("width", "mm"),
    "Blank: " + c.mmToText(s.sheets[0].width * 25.4, "mm") + " mm (" + s.sheets[0].width + " in).");
  assert.equal(c.nestBlankHint("nosuch", "in"), "");
});

// ── Saving one Settings section keeps edits in the others ───────────────

const SETTINGS_FNS = ["viewSettings", "setSectionState", "wireDirty", "unsavedEdits", "guardUnsavedEdits",
  "saveSection", "unsavedHTML", "setHint", "setInput", "setSelect", "secretHint", "nestDefaultsBody",
  "lengthToMm", "mmToText", "nestBlankHint", "fieldProblem", "prefixTable", "wirePrefixTable", "themePickerHTML",
  "markPicked", "wireThemePicker", "gpuPickerHTML", "wireGpuPicker"];

async function settingsHarness(){
  const { $ } = stubDom();
  const calls = [], notes = [];
  let renders = 0;
  const v = stubEl("view");
  const ctx = loadFunctions(SETTINGS_FNS, {
    $, ROUTE:{ name:"settings" }, TOOL:{}, window:{ addEventListener(){} }, document:{ querySelector: () => null },
    location:{ origin:"http://pdm.test" }, localStorage:{ getItem: () => null },
    esc: s => String(s ?? ""), isRole: () => true, myName: () => "Admin", currentTheme: () => "dark",
    THEMES: constValue("THEMES"), GPU_POLICIES: constValue("GPU_POLICIES"),
    NEST_LEN_FIELDS: constValue("NEST_LEN_FIELDS"),
    toast: (m, k) => notes.push([m, k]), render: () => { renders++; },
    addPrefix(){}, changePassword(){}, editPrefix(){}, deletePrefix(){}, applyTheme(){},
    releaseApprovalCard(){}, // another Settings card, loaded on its own
    fontPickerHTML: () => "", wireFontPicker(){}, // font settings (reading-font PR), tested on their own
    isHostAdmin: () => false, hostOnlyNote: () => "", // host-admin gating, tested on its own
    api: async (method, url, body) => {
      calls.push({ method, url, body });
      if (method === "GET"){
        if (url.endsWith("part-number-prefixes")) return { prefixes: [{ category:"Bracket", prefix:"BR" }] };
        if (url.endsWith("gpu-policy")) return { gpu_policy:"auto" };
        if (url.endsWith("/mail")) return { host:"", port:587, configured:false };
        if (url.endsWith("/onshape")) return { configured:false };
        if (url === "/projects") return { projects: [] };
        if (url.endsWith("preview-style")) return { preview_style:"transparent" };
        if (url.endsWith("nest-defaults")) return { defaults: null };
      }
      if (url === "/settings/mail") return Object.assign({ configured: !!body.host, password_set: !!body.password }, body);
      if (url === "/settings/onshape") return { configured:false, secret_set:false };
      return {};
    },
  });
  await ctx.viewSettings(v);
  return { ctx, $, v, calls, notes, renders: () => renders };
}

test("saving one Settings section does not redraw the page", async () => {
  const h = await settingsHarness();
  h.$("mail-host").value = "smtp.example.test"; h.$("mail-port").value = "2525"; h.$("mail-from").value = "pdm@example.test";
  h.$("mail-tls").value = "starttls";
  await h.$("mail-save").onclick(); await flush();
  h.$("os-base").value = "https://cad.onshape.com"; h.$("os-proj").value = "";
  await h.$("os-save").onclick(); await flush();
  h.$("nd-units").value = "in"; h.$("nd-punit").value = "mm"; h.$("nd-grain").value = "any"; h.$("nd-corner").value = "lower_left";
  await h.$("nd-save").onclick(); await flush();
  await h.$("preview-style-save").onclick(); await flush();
  assert.equal(h.renders(), 0, "a section save re-rendered the whole page");
  const puts = h.calls.filter(c => c.method === "PUT").map(c => c.url);
  assert.deepEqual(puts, ["/settings/mail", "/settings/onshape", "/settings/nest-defaults", "/settings/preview-style"]);
  const mail = h.calls.find(c => c.url === "/settings/mail" && c.method === "PUT").body;
  assert.equal(mail.port, 2525);
  const nd = h.calls.find(c => c.url === "/settings/nest-defaults" && c.method === "PUT").body;
  assert.equal(nd.partUnit, "mm");
  assert.equal(h.$("mail-stat").textContent, "Status: configured");
});

test("an invalid mail port is reported instead of being replaced with 587", async () => {
  const h = await settingsHarness();
  h.$("mail-port").value = "smtp";
  await h.$("mail-save").onclick();
  assert.equal(h.calls.filter(c => c.method === "PUT").length, 0);
  assert.equal(h.$("mail-port").attrs["aria-invalid"], "true");
  assert.match(h.$("mail-port-hint").textContent, /1 to 65535/, "the problem is said under the field");
});

test("a failed section save keeps the button usable and says what was not saved", async () => {
  const c = loadFunctions(["saveSection", "setSectionState"], { toast: (m, k) => c.notes.push([m, k]) });
  c.notes = [];
  const btn = stubEl("b"); btn.textContent = "Save mail";
  const out = await c.saveSection(btn, "Saving…", "Mail settings", async () => { throw new Error("mail host refused"); });
  assert.equal(out, null);
  assert.equal(btn.disabled, false);
  assert.equal(btn.textContent, "Save mail");
  assert.deepEqual(c.notes[0], ["Mail settings not saved: mail host refused", "err"]);
});

test("picking a theme or GPU policy marks the card in place without a redraw", async () => {
  const cards = ["dark", "light"].map(id => { const b = stubEl(id); b.attrs["data-theme-id"] = id; b.dataset.themeId = id; return b; });
  let theme = "dark";
  const root = { querySelectorAll: () => cards };
  const c = loadFunctions(["wireThemePicker", "markPicked"], {
    applyTheme: id => { theme = id; }, currentTheme: () => theme,
    render: () => assert.fail("theme change re-rendered the page"), toast: () => assert.fail("toast for an on-screen change") });
  c.wireThemePicker(root);
  cards[1].onclick();
  assert.equal(theme, "light");
  assert(cards[1].classList.contains("on") && !cards[0].classList.contains("on"));
  assert.equal(cards[1].attrs["aria-pressed"], "true");

  const gpu = ["auto", "cpu"].map(id => { const b = stubEl(id); b.attrs["data-gpu-id"] = id; b.dataset.gpuId = id; return b; });
  gpu[0].classList.add("on");
  const g = loadFunctions(["wireGpuPicker", "markPicked"], {
    api: async (m, u, b) => ({ gpu_policy: b.gpu_policy }), render: () => assert.fail("GPU change re-rendered the page"), toast(){} });
  g.wireGpuPicker({ querySelectorAll: () => gpu }, true);
  await gpu[1].onclick();
  assert(gpu[1].classList.contains("on") && !gpu[0].classList.contains("on"));
});

// ── Part-number prefixes: Edit and Delete work ──────────────────────────

test("prefix Edit and Delete buttons are wired once the table is in the page", () => {
  const seen = [];
  const c = loadFunctions(["prefixTable", "wirePrefixTable"], {
    esc: s => String(s), editPrefix: cat => seen.push(["edit", cat]), deletePrefix: (cat, p) => seen.push(["del", cat, p]) });
  const markup = c.prefixTable([{ category:"Bracket", prefix:"BR" }], true);
  assert.match(markup, /data-edit='Bracket'/);
  assert.match(markup, /data-del='Bracket'/);
  const edit = stubEl("e"); edit.dataset.edit = "Bracket";
  const del = stubEl("d"); del.dataset.del = "Bracket"; del.dataset.prefix = "BR";
  c.wirePrefixTable({ querySelectorAll: sel => sel === "[data-edit]" ? [edit] : sel === "[data-del]" ? [del] : [] });
  edit.onclick(); del.onclick();
  assert.deepEqual(seen, [["edit", "Bracket"], ["del", "Bracket", "BR"]]);
});

test("deleting a prefix asks in the app's own dialog, names prefix and category, and looks destructive", async () => {
  const asked = [], deleted = [];
  const c = loadFunctions(["deletePrefix"], {
    esc: s => String(s), confirm: () => assert.fail("browser confirm() used"),
    openModal: async (title, fields, submit, extra) => { asked.push({ title, submit, extra }); return submit.onSubmit({}); },
    api: async (m, url) => { deleted.push([m, url]); return {}; }, refreshPrefixes: async () => {}, toast(){} });
  await c.deletePrefix("Bracket", "BR");
  assert.equal(asked.length, 1);
  assert.match(asked[0].title, /BR/);
  assert.match(asked[0].title, /Bracket/);
  assert.equal(asked[0].submit.label, "Delete prefix");
  assert.equal(asked[0].submit.danger, true);
  assert.equal(JSON.stringify(deleted), JSON.stringify([["DELETE", "/settings/part-number-prefixes/Bracket"]]));
});

test("reset password shows the new password after it is set", async () => {
  const { $ } = stubDom();
  const dialogs = [], posted = [];
  $("rp-new").value = "Kq7mP2xR9wTz";
  const c = loadFunctions(["resetPassword", "secretFieldHTML", "wireSecretField", "copyText"], {
    $, esc: s => String(s), genPassword: () => "Kq7mP2xR9wTz", toast(){},
    openModal: async (title, fields, submit, extra) => {
      dialogs.push({ title, fields, submit, extra });
      await null; // the user submits after the dialog is drawn
      return submit && submit.onSubmit ? submit.onSubmit({}) : {};
    },
    api: async (m, url, body) => { posted.push([url, body]); return {}; } });
  await c.resetPassword({ id:"u1", email:"jo@example.test", display_name:"Jo" });
  assert.equal(JSON.stringify(posted), JSON.stringify([["/users/u1/password", { password:"Kq7mP2xR9wTz" }]]));
  assert.equal(dialogs.length, 2, "no dialog showed the new password");
  assert.match(dialogs[0].extra, /type='text'/, "the generated password is hidden in a password field");
  assert.match(dialogs[1].extra, /Kq7mP2xR9wTz/);
  assert.match(dialogs[1].extra, /not shown again/);
});

function inviteHarness(apiImpl, users){
  const dialogs = [], notes = [], resets = [];
  const c = loadFunctions(["inviteUser", "findUserByEmail", "inviteMailFailed"], {
    USERS: users || [], esc: s => String(s), render(){}, toast: m => notes.push(m), resetPassword: async u => resets.push(u.id),
    api: apiImpl,
    openModal: async (title, fields, submit, extra) => {
      dialogs.push({ title, submit, extra });
      if (submit && submit.onSubmit) return submit.onSubmit({ email:"New@example.test", display_name:"", role:"editor" });
      return {};
    } });
  return { c, dialogs, notes, resets };
}

test("a failed invite that created the account says so and offers a password reset", async () => {
  const made = { id:"u9", email:"new@example.test" };
  const h = inviteHarness(async (m, url) => {
    if (url === "/users/invite") throw new Error("user created but the invite email failed: smtp refused");
    if (url === "/users") return { users: [made] };
  });
  await h.c.inviteUser();
  assert.equal(h.dialogs.length, 2);
  assert.match(h.dialogs[1].extra, /was added/);
  assert.match(h.dialogs[1].extra, /smtp refused/);
  assert.deepEqual([...h.resets], ["u9"]);
});

test("a failed invite that created nothing keeps the invite dialog open with the error", async () => {
  const h = inviteHarness(async (m, url) => {
    if (url === "/users/invite") throw new Error("mail server is not configured");
    if (url === "/users") return { users: [] };
  });
  await assert.rejects(h.c.inviteUser(), /not configured/, "the error goes back to the dialog");
  assert.equal(h.dialogs.length, 1);
  // An email that was already a user is never reported as "was added".
  const h2 = inviteHarness(async (m, url) => {
    if (url === "/users/invite") throw new Error("conflict");
    if (url === "/users") return { users: [{ id:"u1", email:"new@example.test" }] };
  }, [{ id:"u1", email:"new@example.test" }]);
  await assert.rejects(h2.c.inviteUser(), /conflict/);
  assert.equal(h2.dialogs.length, 1);
});

test("Invite user is disabled with a reason when no mail server is set up", async () => {
  const { $ } = stubDom();
  const v = stubEl("view");
  const c = loadFunctions(["viewAdmin", "usersTable", "auditTable", "themePickerHTML", "wireThemePicker", "markPicked"], {
    $, v, esc: s => String(s ?? ""), isRole: () => true, myName: () => "Admin", ME:{ user:{ role:"admin" } },
    currentTheme: () => "dark", THEMES: constValue("THEMES"), userAv: () => "", fmtDate: () => "", actorName: () => "",
    USERS: [], UMAP: {}, go(){}, addUser(){}, inviteUser(){}, giveToken(){}, resetPassword(){}, editUser(){}, toggleUserActive(){},
    api: async (m, url) => url === "/settings/mail" ? { configured:false } : url === "/users" ? { users: [] } : {} });
  await c.viewAdmin(v);
  assert.match(v.innerHTML, /id='invbtn' disabled/);
  assert.match(v.innerHTML, /needs a mail server/);
  const withMail = loadFunctions(["viewAdmin", "usersTable", "auditTable", "themePickerHTML", "wireThemePicker", "markPicked"], {
    $, esc: s => String(s ?? ""), isRole: () => true, myName: () => "Admin", ME:{ user:{ role:"admin" } },
    currentTheme: () => "dark", THEMES: constValue("THEMES"), userAv: () => "", fmtDate: () => "", actorName: () => "",
    USERS: [], UMAP: {}, go(){}, addUser(){}, inviteUser(){}, giveToken(){}, resetPassword(){}, editUser(){}, toggleUserActive(){},
    api: async (m, url) => url === "/settings/mail" ? { configured:true } : url === "/users" ? { users: [] } : {} });
  const v2 = stubEl("view2");
  await withMail.viewAdmin(v2);
  assert.doesNotMatch(v2.innerHTML, /id='invbtn' disabled/);
});

// ── Tool editor keeps edits across tabs and tool-type changes ───────────

function toolHarness(){
  const els = new Map();
  const $ = id => els.get(id) || null;
  const make = id => { const e = stubEl(id); els.set(id, e); return e; };
  const TOOL = { id:"t1", tab:"general", data:{ id:"t1", name:"6 mm end mill", kind:"end_mill", units:"mm",
    vendor:null, dimensions:{ diameter:6, flute_length:18 } }, cutting:[], docs:[] };
  const tab = stubEl("ttab");
  let geom = [];
  tab.querySelectorAll = () => geom;
  els.set("ttab", tab);
  const c = loadFunctions(["toolDraft", "captureToolInputs", "collectToolEdits"], {
    $, TOOL, TOOL_FIELDS: constValue("TOOL_FIELDS"),
    TOOL_GEOMETRY: vm.runInNewContext("(" + app.match(/\nconst TOOL_GEOMETRY = ([\s\S]*?\});\r?\n/)[1] + ")") });
  // Show one tab: only its inputs exist, like the real DOM.
  const show = (ids, g) => {
    for (const id of ["g-desc","g-vendor","g-pn","g-link","c-kind","c-units","c-rotation","c-flutes","c-material","p-tnum"])
      els.delete(id);
    const d = c.toolDraft().t;
    const key = { "g-vendor":"vendor", "c-kind":"kind", "c-units":"units" };
    for (const id of ids) make(id).value = String(d[key[id]] ?? "");
    geom = (g || []).map(k => { const e = stubEl("geom-" + k); e.value = String(d.dimensions[k] ?? ""); return e; });
    return geom;
  };
  return { c, $, TOOL, show };
}

test("tool edits survive a tab switch and are all sent by one save", () => {
  const h = toolHarness();
  h.show(["g-vendor"]);
  h.$("g-vendor").value = "Typed vendor"; h.c.captureToolInputs();
  const geom = h.show(["c-kind", "c-units"], ["diameter", "flute_length"]);
  geom[0].value = "6.35"; h.c.captureToolInputs();
  h.show(["g-vendor"]);
  assert.equal(h.$("g-vendor").value, "Typed vendor", "General lost its edit after visiting Cutter");
  h.c.captureToolInputs();
  const edits = h.c.collectToolEdits();
  assert.equal(edits.vendor, "Typed vendor");
  assert.deepEqual({ ...edits.dimensions }, { diameter: 6.35, flute_length: 18 });
});

test("changing the tool type keeps the type and the geometry both types share", () => {
  const h = toolHarness();
  h.show(["c-kind", "c-units"], ["diameter", "flute_length"]);
  h.$("c-kind").value = "drill"; h.c.captureToolInputs();
  assert.equal(h.TOOL.draft.t.kind, "drill");
  const edits = h.c.collectToolEdits();
  assert.equal(edits.kind, "drill");
  assert.deepEqual({ ...edits.dimensions }, { diameter: 6 }, "drill geometry should keep the diameter only");
  assert(!("vendor" in edits) && !("units" in edits), "fields nobody touched are not sent");
  // Back to end mill before saving: the cutting length typed earlier is still there.
  h.$("c-kind").value = "end_mill"; h.c.captureToolInputs();
  assert.deepEqual({ ...h.c.collectToolEdits().dimensions }, { diameter: 6, flute_length: 18 });
});

// ── Sign-in ─────────────────────────────────────────────────────────────

test("sign-in fields are a form with visible labels; the organization field waits until needed", () => {
  const login = html.slice(html.indexOf("<div id=\"login-view\""), html.indexOf("<div id=\"ws\""));
  const form = login.slice(login.indexOf("<form id=\"signin-form\">"), login.indexOf("</form>"));
  assert(form.length > 0, "sign-in is not a form");
  for (const id of ["login_email", "login_password", "login_org"]){
    assert.match(form, new RegExp("<input id=\"" + id + "\""), id + " outside the sign-in form");
    assert.match(login, new RegExp("<label for=\"" + id + "\">"), id + " has no label");
  }
  assert.doesNotMatch(form, /placeholder="(email|password|organization)/i, "placeholder used as the label");
  assert.match(login, /<div class="f" id="login_org_f" hidden>/);
  assert.match(login, /<button id="signin" type="submit"/);
  assert.doesNotMatch(login, /id="tokshow"/, "the after-sign-in token panel is never visible");
  assert.doesNotMatch(app, /\$\("tokcopy"\)|\$\("tokval"\)/);
});

test("the organization field opens only for the several-organizations answer", () => {
  const c = loadFunctions(["signinNeedsOrg"], {});
  const several = { error:{ code:"bad_request", message:"…" } };
  assert.equal(c.signinNeedsOrg(400, several, ""), true);
  assert.equal(c.signinNeedsOrg(400, several, "Test Org"), false, "already named one");
  assert.equal(c.signinNeedsOrg(401, { error:{ code:"bad_credentials" } }, ""), false);
  assert.equal(c.signinNeedsOrg(400, {}, ""), false, "a body that is not the API's error");
});

// ── Getting started ─────────────────────────────────────────────────────

test("getting started keeps its steps until done and hides admin steps from others", () => {
  const c = loadFunctions(["gettingStartedSteps"], {});
  const editor = c.gettingStartedSteps({ admin:false, projects:1, items:0, users:1 });
  assert.deepEqual([...editor.map(s => s.key)], ["project", "upload"]);
  assert.deepEqual([...editor.map(s => s.done)], [true, false], "the guide must outlive step 1");
  const admin = c.gettingStartedSteps({ admin:true, projects:1, items:3, users:1, mail:false, prefixes:0 });
  assert.deepEqual([...admin.map(s => s.key)], ["project", "upload", "mail", "invite", "prefix"]);
  assert.deepEqual([...admin.filter(s => !s.done).map(s => s.key)], ["mail", "invite", "prefix"]);
});

test("a hidden getting-started guide stays hidden", async () => {
  const c = loadFunctions(["gettingStartedHTML", "gettingStartedSteps"], {
    localStorage:{ getItem: k => k === "pdm_getting_started" ? "hidden" : null }, isRole: () => true,
    api: async () => assert.fail("fetched while hidden"), PROJECTS: [], USERS: [], esc: String,
    GS_ADMIN: null, GS_ADMIN_AT: 0 });
  assert.equal(await c.gettingStartedHTML({ items:0 }), "");
});

test("an optional step left undone does not keep the guide on the dashboard", async () => {
  const c = loadFunctions(["gettingStartedHTML", "gettingStartedSteps"], {
    localStorage:{ getItem: () => null }, isRole: () => true, esc: String,
    api: async () => assert.fail("re-read inside the minute"), PROJECTS: [{}], USERS: [{}, {}],
    GS_ADMIN: { mail: true, prefixes: 0 }, GS_ADMIN_AT: Date.now() });
  assert.equal(await c.gettingStartedHTML({ items: 3 }), "");
});

test("saving a tool with nothing edited sends nothing", async () => {
  const h = toolHarness();
  let states = [];
  const c = loadFunctions(["saveTool", "toolDraft", "captureToolInputs", "collectToolEdits"], {
    $: h.$, TOOL: h.TOOL, TOOL_FIELDS: constValue("TOOL_FIELDS"), TOOL_GEOMETRY: {},
    document: { querySelectorAll: () => [] }, toolStateText: t => states.push(t),
    api: async () => assert.fail("saved with no edits"), render: async () => {}, toast(){} });
  await c.saveTool();
  assert.deepEqual(states, ["No changes to save"]);
});

function secretHarness(clipboardOk){
  const notes = [];
  const field = stubEl("pw"); field.value = "Kq7mP2xR9wTz"; field.type = "text";
  let selected = false; field.select = () => { selected = true; };
  const show = stubEl("show"); show.textContent = "Hide";
  const copy = stubEl("copy"); copy.textContent = "Copy";
  const c = loadFunctions(["wireSecretField", "copyText"], {
    navigator: { clipboard: { writeText: async () => { if (!clipboardOk) throw new Error("blocked"); } } },
    document: { execCommand: () => false }, setTimeout: () => 0, toast: (m, k) => notes.push([m, k]) });
  c.wireSecretField(field, show, copy);
  return { field, show, copy, notes, selected: () => selected };
}

test("Copy copies the password and says so", async () => {
  const h = secretHarness(true);
  await h.copy.onclick();
  assert.equal(h.copy.textContent, "Copied");
  assert.equal(h.notes.length, 0);
});

test("when the clipboard is blocked, a hidden password is shown and selected with what to do", async () => {
  const h = secretHarness(false);
  h.show.onclick(); // Hide
  assert.equal(h.field.type, "password");
  await h.copy.onclick();
  assert.equal(h.field.type, "text", "a password field cannot be selected for copying");
  assert(h.selected(), "the password is selected for a manual copy");
  assert.match(h.notes[0][0], /Ctrl\+C/);
  assert.equal(h.copy.textContent, "Copy");
});
