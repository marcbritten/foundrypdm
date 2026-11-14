// Run with: node --test crates/pdm-api/tests/web-ui.test.cjs
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

test("SolidCraft supports successive saves and rejects a changed vault head", async () => {
  let head = { id:"v1", seq:1 }, options;
  const bases = [];
  const context = loadFunctions(["openSolidcraftWeb"], {
    api: async (method, url, body) => {
      if (method === "GET") return { head_version:head, item:{}, file:{} };
      if (url === "/blobs") return { blob:{ sha256:"edited" } };
      if (url.endsWith("/checkin")){
        bases.push(body.base_version);
        head = { id:"v" + (head.seq + 1), seq:head.seq + 1 };
        return { version:head };
      }
    },
    PdmEditors:{ openSolidcraft: async config => { options = config; } },
    isNativeDesign:() => true, sourceFilename:() => "design.sc", isRole:() => true,
    myId:() => "user", location:{ hostname:"test" }, CUR_RELOAD:null,
    toast:message => assert.fail(message), Uint8Array,
  });
  await context.openSolidcraftWeb("item");
  await options.checkin(new ArrayBuffer(1), "first");
  await options.checkin(new ArrayBuffer(1), "second");
  assert.deepEqual(bases, [1, 2]);
  head = { id:"external", seq:4 };
  await assert.rejects(options.checkin(new ArrayBuffer(1), "stale"), /head has changed/);
  assert.deepEqual(bases, [1, 2]);
});

test("web editors use the selected historical version's filename and MIME type", async () => {
  let options;
  const version = { id:"old", seq:1, filename:"old.dxf", mime_type:"application/dxf" };
  const context = loadFunctions(["sourceFilename", "openWebEditor"], {
    api:async () => ({ versions:[version], file:{ filename:"latest.pdf" }, item:{} }),
    PdmEditors:{
      forFile:(mime, filename) => {
        assert.equal(mime, "application/dxf");
        assert.equal(filename, "old.dxf");
        return { extension:"dxf" };
      },
      open:async config => { options = config; },
    },
    isRole:() => false, toast:message => assert.fail(message),
  });
  await context.openWebEditor("item", "old");
  assert.equal(options.name, "old.dxf");
  assert.equal(options.version.id, "old");
});

test("native conversions deduplicate by content and never silently rebase conflicts", async () => {
  const notices = [], writes = [];
  let sameContent = false;
  const context = loadFunctions(["nativeVersionAction"], {
    DETAIL:{ id:"item", data:{ versions:[{ id:"source", seq:1, derived_solidcraft_sha256:"native" }] } },
    api:async (method, url, body) => {
      if (url.endsWith("/solidcraft")) return { filename:"design.sc", artifact:{ sha256:"native" } };
      if (url === "/items/item" && method === "GET") return {
        versions:[{ seq:2, filename:"design.sc", sha256:sameContent ? "native" : "different" }],
        head_version:{ seq:2 }, lock:{ user_id:"user" },
      };
      if (url.endsWith("/solidcraft/blob")) return new ArrayBuffer(1);
      if (url === "/blobs") return { blob:{ sha256:"native", size_bytes:1 } };
      if (url.endsWith("/checkin")){
        writes.push(body.base_version);
        throw Object.assign(new Error("stale base"), { status:409 });
      }
      assert.fail("unexpected API request " + url);
    },
    myId:() => "user", toast:message => notices.push(message), Uint8Array,
  });
  const button = {};
  await context.nativeVersionAction("source", button);
  assert.deepEqual(writes, [2], "a conflict must not retry against a newer head");
  assert.match(notices.at(-1), /stale base/);
  assert.equal(button.disabled, false);
  sameContent = true;
  await context.nativeVersionAction("source", button);
  assert.deepEqual(writes, [2]);
  assert.match(notices.at(-1), /Already converted/);
});

// A small browser bridge double: storage and download bytes are real, while
// dialog elements expose only the DOM methods the editor host needs.
function editorHarness(checkin){
  const storage = new Map(), downloads = [], blobs = [];
  let dialog, design = "first design", collectError = false;
  function element(tag){
    const children = new Map();
    const el = {
      tagName:tag, style:{}, setAttribute(){}, addEventListener(){},
      appendChild(){}, showModal(){}, close(){}, remove(){},
      querySelector(selector){
        if (!children.has(selector)) children.set(selector, element(selector));
        return children.get(selector);
      },
      click(){ if (tag === "a") downloads.push({ name:el.download, blob:blobs.at(-1) }); },
    };
    if (tag === "dialog") dialog = el;
    if (tag === "iframe") el.contentWindow = { solidcraft:{ execute:async (_, args) => {
      if (collectError) throw new Error("editor unavailable");
      storage.set("solidcraft.file." + args.path.replace("/opfs/", ""), "t" + design);
    } } };
    return el;
  }
  const window = {
    localStorage:{ getItem:key => storage.get(key), removeItem:key => storage.delete(key) },
    addEventListener(){}, removeEventListener(){},
  };
  const context = vm.createContext({
    window, document:{ createElement:element, body:{ appendChild(){} } },
    navigator:{}, location:{ origin:"http://test" }, Blob, TextEncoder, Uint8Array, ArrayBuffer,
    URL:{ createObjectURL:blob => { blobs.push(blob); return "blob:test"; }, revokeObjectURL(){} },
    setTimeout:() => 0, clearTimeout(){}, fetch:async () => ({ ok:true }),
    confirm:() => true, prompt:() => "Edit",
  });
  vm.runInContext(editors, context);
  return {
    open:() => window.PdmEditors.openSolidcraft({ version:{ seq:1 }, name:"design.sc", designUrl:"/design", checkin }),
    button:selector => dialog.querySelector(selector), downloads,
    design:value => { design = value; }, failCollection:() => { collectError = true; },
  };
}

test("SolidCraft edited download releases busy state and saves collect fresh bytes", async () => {
  const saves = [];
  const harness = editorHarness(async bytes => {
    saves.push(new TextDecoder().decode(bytes));
    return { seq:saves.length + 1 };
  });
  await harness.open();
  await harness.button("[data-edited]").onclick();
  assert.equal(harness.downloads.length, 1);
  assert.equal(harness.downloads[0].name, "design.sc");
  assert.equal(await harness.downloads[0].blob.text(), "first design");
  assert.equal(harness.button("[data-checkin]").disabled, false);
  harness.design("newer design");
  await harness.button("[data-checkin]").onclick();
  harness.design("third design");
  await harness.button("[data-checkin]").onclick();
  assert.deepEqual(saves, ["newer design", "third design"]);
  assert.match(harness.button("strong").textContent, /v3/);
});

test("failed SolidCraft saves retain downloadable bytes when the editor stops responding", async () => {
  const harness = editorHarness(async () => { throw new Error("network offline"); });
  await harness.open();
  await harness.button("[data-checkin]").onclick();
  assert.match(harness.button("[role=status]").textContent, /network offline/);
  harness.failCollection();
  await harness.button("[data-edited]").onclick();
  assert.equal(await harness.downloads[0].blob.text(), "first design");
  assert.equal(harness.button("[data-checkin]").disabled, false);
  assert.match(harness.button("[role=status]").textContent, /last collected design/);
});
