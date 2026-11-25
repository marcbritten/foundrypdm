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

for (const folderState of ["disabled", "enabled", "unavailable"]) {
  test("Onshape imports and converts with folder sync " + folderState, async () => {
    const elements = new Map(), calls = [], converted = [], notices = [];
    const element = id => {
      if (!elements.has(id)) elements.set(id, { isConnected:true, value:"", checked:true });
      return elements.get(id);
    };
    element("os-import-url").value = " https://cad.onshape.com/documents/doc/w/ws/e/studio ";
    element("os-import-name").value = " Bracket ";
    const box = { isConnected:true };
    const context = loadFunctions(["projectSettingsTab"], {
      ROUTE:{ pid:"project", _items:[], _folders:[], _ptab:"settings" }, PMAP:{},
      $:element, esc:String, fmtDate:() => "", attachThumbs(){},
      api:async (method, url, body) => {
        calls.push({ method, url, body });
        if (url.endsWith("/onshape-folder")) {
          if (folderState === "unavailable") throw new Error("folder service unavailable");
          return { enabled:folderState === "enabled" };
        }
        if (url.endsWith("/onshape-import")) return { item:{ id:"item" }, job:{ id:"fetch" } };
        if (url === "/items/item") return { head_version:{ id:"bundle", seq:1 } };
        assert.fail("unexpected API request " + url);
      },
      waitForVersionJob:async id => {
        assert.equal(id, "fetch");
        // The user can leave Settings while the fetch runs. Conversion should
        // still honor the choice made when the import was submitted.
        box.isConnected = false;
        elements.delete("os-import-convert");
        return true;
      },
      importVersionToSolidcraft:async (item, version) => converted.push([item, version.id]),
      toast:message => notices.push(message), ALLITEMS:[], ALLITEMS_AT:0,
    });
    await context.projectSettingsTab(box);
    assert.equal(typeof element("os-import-go").onclick, "function");
    await element("os-import-go").onclick();
    const request = calls.find(call => call.url.endsWith("/onshape-import"));
    assert.equal(request.body.url, "https://cad.onshape.com/documents/doc/w/ws/e/studio");
    assert.equal(request.body.name, "Bracket");
    assert.deepEqual(converted, [["item", "bundle"]]);
    assert.equal(element("os-import-go").disabled, false);
    assert.equal(notices.at(-1), "Onshape import complete");
  });
}

const onshapeUrl = "https://cad.onshape.com/documents/01f013e44a26420bf40d219b/w/84d2c05360df47c98d789b08/e/abc41199f68ccad25c577b1a";

function stepImportHarness(existing){
  const calls = [], checkins = [], conversions = [];
  let conversionError = null;
  const context = loadFunctions(["importOnshapeStep"], {
    api:async (method, url, body) => {
      calls.push({ method, url, body });
      if (url.startsWith("/integrations/onshape/lookup?")) return { item:existing };
      if (url === "/projects/project/items") return { item:{ id:"new-item", part_number:"BE-123", project_id:"project" } };
      if (method === "PUT" && url.includes("/properties/")) return {};
      assert.fail("unexpected request " + method + " " + url);
    },
    checkinItem:async (item, file, message) => {
      checkins.push({ item, file, message });
      return { version:{ id:"step-version", seq:1, native_format:"step" } };
    },
    importVersionToSolidcraft:async (item, version) => {
      conversions.push({ item, version });
      if (conversionError) throw new Error(conversionError);
    },
    ALLITEMS:[], ALLITEMS_AT:0,
  });
  return { context, calls, checkins, conversions, failConversion:message => { conversionError = message; } };
}

test("manual STEP fallback works without Onshape keys or a URL and preserves the STEP format", async () => {
  const h = stepImportHarness();
  const file = { name:"bracket.STEP", size:12 };
  const result = await h.context.importOnshapeStep("project", file, { convert:true });
  assert.equal(result.item.id, "new-item");
  assert.equal(h.calls.length, 1, "no Onshape API or configuration requests");
  assert.equal(h.checkins[0].file, file);
  assert.equal(h.conversions[0].version.native_format, "step");
});

test("manual STEP reuses the failed Onshape import's item for the supplied URL", async () => {
  const h = stepImportHarness({ id:"failed-import", part_number:"BE-100", project_id:"project" });
  await h.context.importOnshapeStep("project", { name:"model.stp", size:12 }, { url:onshapeUrl, convert:true });
  assert.equal(h.calls.some(call => call.url === "/projects/project/items"), false);
  assert.equal(h.checkins[0].item, "failed-import");
  assert.match(h.checkins[0].message, /01f013e44a26420bf40d219b/);
  assert.equal(h.calls.find(call => call.url.endsWith("onshape.document_id")).body.value, "01f013e44a26420bf40d219b");
  assert.equal(h.calls.find(call => call.url.endsWith("onshape.element_id")).body.value, "abc41199f68ccad25c577b1a");
  assert.equal(h.conversions[0].item, "failed-import");
});

test("manual STEP rejects cross-project links and invalid input before writing", async () => {
  const h = stepImportHarness({ id:"other", project_id:"another-project" });
  const file = { name:"model.step", size:12 };
  await assert.rejects(h.context.importOnshapeStep("project", file, { url:onshapeUrl }), /another project/);
  await assert.rejects(h.context.importOnshapeStep("project", file, { url:"https://example.com/file" }), /Part Studio/);
  await assert.rejects(h.context.importOnshapeStep("project", { name:"model.zip", size:12 }, {}), /\.step or \.stp/);
  await assert.rejects(h.context.importOnshapeStep("project", { name:"model.step", size:0 }, {}), /empty/);
  assert.equal(h.calls.length, 1);
  assert.equal(h.checkins.length, 0);
});

test("manual STEP can skip conversion and reports a saved source when conversion fails", async () => {
  const h = stepImportHarness();
  const file = { name:"model.step", size:12 };
  await h.context.importOnshapeStep("project", file, { convert:false });
  assert.equal(h.conversions.length, 0);
  h.failConversion("converter unavailable");
  await assert.rejects(h.context.importOnshapeStep("project", file, { convert:true }), /STEP saved to BE-123 as v1.*converter unavailable.*Versions tab/);
  assert.equal(h.checkins.length, 2);
});

test("native Onshape failures remain visible with their job ID and offer STEP fallback", async () => {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, { isConnected:true, value:"", checked:true });
    return elements.get(id);
  };
  element("os-import-url").value = onshapeUrl;
  const context = loadFunctions(["projectSettingsTab"], {
    ROUTE:{ pid:"project", _items:[], _folders:[] }, PMAP:{}, $:element,
    esc:String, fmtDate:() => "", attachThumbs(){}, toast(){},
    api:async (method, url) => url.endsWith("/onshape-folder") ? { enabled:false } : { item:{ id:"item" }, job:{ id:"failed-job" } },
    waitForVersionJob:async () => { throw new Error("unrecognized subcommand 'onshape'"); },
  });
  await context.projectSettingsTab({ isConnected:true });
  await element("os-import-go").onclick();
  assert.match(element("os-import-status").textContent, /failed-job.*\n.*unrecognized subcommand.*\n.*exported STEP/);
  assert.equal(element("os-import-go").disabled, false);
  assert.equal(element("os-step-go").disabled, false);
});

test("SolidCraft supports successive saves and rejects a changed vault head", async () => {
  let head = { id:"v1", seq:1 }, options, links = 0;
  const bases = [];
  const context = loadFunctions(["openSolidcraftWeb", "scCheckin", "makeScCheckin"], {
    api: async (method, url, body) => {
      if (method === "GET") return { head_version:head, item:{}, file:{} };
      if (url === "/versions/v1/design-link") return { url:"/public/design/v1/design.sc?expires=1&signature=s" + ++links };
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
  // The editor opens a signed link, and each later call makes a fresh one.
  assert.equal(options.designUrl, "/public/design/v1/design.sc?expires=1&signature=s1");
  assert.equal(await options.designLink(), "/public/design/v1/design.sc?expires=1&signature=s2");
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
  const context = loadFunctions(["nativeVersionAction", "importVersionToSolidcraft"], {
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
  assert.match(notices.at(-1), /Already imported/);
});

// A small browser bridge double: storage and download bytes are real, while
// dialog elements expose only the DOM methods the editor host needs.
function editorHarness(config, { fetch:fetchImpl } = {}){
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
    setTimeout:() => 0, clearTimeout(){}, fetch:fetchImpl || (async () => ({ ok:true })),
    confirm:() => true, prompt:() => "Edit",
  });
  vm.runInContext(editors, context);
  return {
    open:() => window.PdmEditors.openSolidcraft(config),
    button:selector => dialog.querySelector(selector), downloads,
    get dialog(){ return dialog; },
    design:value => { design = value; }, failCollection:() => { collectError = true; },
  };
}

test("SolidCraft edited download releases busy state and saves collect fresh bytes", async () => {
  const saves = [];
  const checkin = async bytes => {
    saves.push(new TextDecoder().decode(bytes));
    return { seq:saves.length + 1 };
  };
  const harness = editorHarness({ version:{ seq:1 }, name:"design.sc", designUrl:"/design", checkin });
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

test("SolidCraft Download original fetches a fresh design link", async () => {
  const fetched = [];
  let links = 0;
  const harness = editorHarness({
    version:{ seq:1 }, name:"design.sc", checkin:null,
    designUrl:"/public/design/v1/design.sc?expires=1&signature=opened",
    designLink:async () => "/public/design/v1/design.sc?expires=2&signature=fresh" + ++links,
  }, { fetch:async url => {
    fetched.push(url);
    return { ok:true, arrayBuffer:async () => new TextEncoder().encode("original").buffer };
  } });
  await harness.open();
  await harness.button("[data-original]").onclick();
  assert.deepEqual(fetched.filter(url => url.startsWith("/public/")),
    ["/public/design/v1/design.sc?expires=2&signature=fresh1"]);
  assert.equal(harness.downloads.length, 1);
  assert.equal(await harness.downloads[0].blob.text(), "original");
});

test("failed SolidCraft saves retain downloadable bytes when the editor stops responding", async () => {
  const checkin = async () => { throw new Error("network offline"); };
  const harness = editorHarness({ version:{ seq:1 }, name:"design.sc", designUrl:"/design", checkin });
  await harness.open();
  await harness.button("[data-checkin]").onclick();
  assert.match(harness.button("[role=status]").textContent, /network offline/);
  harness.failCollection();
  await harness.button("[data-edited]").onclick();
  assert.equal(await harness.downloads[0].blob.text(), "first design");
  assert.equal(harness.button("[data-checkin]").disabled, false);
  assert.match(harness.button("[role=status]").textContent, /last collected design/);
});

test("SolidCraft new-design mode creates the item once, then checks in to it", async () => {
  const calls = [];
  const laterCheckin = async (bytes, message) => { calls.push(["checkin", message]); return { seq:2 }; };
  const createNew = async (bytes, into) => {
    calls.push(["createNew", new TextDecoder().decode(bytes), into]);
    return { item:{ part_number:"BE-000116" }, version:{ seq:1 }, checkin: laterCheckin };
  };
  const harness = editorHarness({ createNew });
  await harness.open();
  assert.equal(harness.button("strong").textContent, "SolidCraft — new design");
  assert.equal(harness.button("[data-original]").hidden, true);
  assert.equal(harness.button("[data-checkin]").disabled, false);
  assert.match(harness.button("[role=status]").textContent, /new item/);
  await harness.button("[data-checkin]").onclick();
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], "createNew");
  assert.equal(calls[0][1], "first design");
  assert.equal(calls[0][2], harness.dialog, "the item form renders inside the editor dialog");
  assert.equal(harness.button("strong").textContent, "SolidCraft — BE-000116 · v1");
  assert.match(harness.button("[role=status]").textContent, /Saved to PDM: BE-000116 v1/);
  harness.design("second design");
  await harness.button("[data-checkin]").onclick();
  assert.deepEqual(calls.slice(1), [["checkin", "Edit"]], "a second save checks in, not another item");
  assert.equal(harness.button("strong").textContent, "SolidCraft — BE-000116 · v2");
});

test("SolidCraft new-design save can be cancelled and retried", async () => {
  let attempts = 0;
  const createNew = async () => {
    attempts += 1;
    if (attempts === 1) return null;
    return { item:{ part_number:"BE-000200" }, version:{ seq:1 } };
  };
  const harness = editorHarness({ createNew });
  await harness.open();
  await harness.button("[data-checkin]").onclick();
  assert.match(harness.button("[role=status]").textContent, /cancelled/i);
  assert.equal(harness.button("[data-checkin]").disabled, false);
  await harness.button("[data-checkin]").onclick();
  assert.equal(attempts, 2);
  assert.equal(harness.button("strong").textContent, "SolidCraft — BE-000200 · v1");
});

test("new part in SolidCraft web creates the item and checks in its first version", async () => {
  const reqs = [];
  const context = loadFunctions(["newPartInSolidcraftWeb", "scCheckin", "makeScCheckin"], {
    api: async (method, url, body) => {
      reqs.push([method, url, body]);
      if (method === "GET" && url === "/projects/p1/folders") return { folders:[{ id:"f1", name:"Housings" }] };
      if (method === "POST" && url === "/projects/p1/items") return { item:{ id:"i1", part_number:"BE-000116" } };
      if (method === "PUT" && url === "/blobs") return { blob:{ sha256:"sc-bytes", size_bytes:6 } };
      if (method === "POST" && url === "/items/i1/checkout") return {};
      if (method === "POST" && url === "/items/i1/checkin") return { version:{ id:"v1", seq:1 } };
      if (method === "GET" && url === "/items/i1") return { head_version:null, lock:null };
      assert.fail("unexpected request " + method + " " + url);
    },
    openModal: async (title, fields, submitLabel, extra, into) => {
      assert.equal(title, "New part");
      return { desc:"Bracket", pn:"", cls:"BE", folder:"f1", msg:"Created in SolidCraft web" };
    },
    PdmEditors:{ openSolidcraft: async config => {
      assert.equal(config.version, undefined);
      const result = await config.createNew(new TextEncoder().encode("design"), "into");
      assert.equal(result.item.part_number, "BE-000116");
      assert.equal(typeof result.checkin, "function");
    } },
    isRole:() => true, toast:message => assert.fail(message),
    location:{ hostname:"test" }, Uint8Array, myId:() => "u", lockName:() => "other",
    ALLITEMS:[], ALLITEMS_AT:0, render(){},
  });
  await context.newPartInSolidcraftWeb("p1");
  const create = reqs.find(r => r[0] === "POST" && r[1] === "/projects/p1/items");
  assert.equal(create[2].part_number, null, "a blank part number auto-assigns from the org prefix");
  assert.equal(create[2].description, "Bracket");
  assert.equal(create[2].classification, "BE");
  assert.equal(create[2].folder_id, "f1");
  const checkin = reqs.find(r => r[0] === "POST" && r[1] === "/items/i1/checkin");
  assert.equal(checkin[2].base_version, null);
  assert.equal(checkin[2].filename, "BE-000116.sc");
  assert.equal(checkin[2].sha256, "sc-bytes");
  assert.equal(checkin[2].commit_message, "Created in SolidCraft web");
});

test("new part in SolidCraft web creates nothing when the modal is cancelled", async () => {
  const reqs = [];
  const context = loadFunctions(["newPartInSolidcraftWeb", "scCheckin", "makeScCheckin"], {
    api: async (method, url) => { reqs.push([method, url]); return { folders:[] }; },
    openModal: async () => null,
    PdmEditors:{ openSolidcraft: async config => {
      assert.equal(await config.createNew(new ArrayBuffer(1), null), null);
    } },
    isRole:() => true, toast:message => assert.fail(message),
    location:{ hostname:"test" }, Uint8Array, myId:() => "u", lockName:() => "other",
    ALLITEMS:[], ALLITEMS_AT:0, render(){},
  });
   await context.newPartInSolidcraftWeb("p1");
   assert.deepEqual(reqs, [["GET", "/projects/p1/folders"]]);
});

test("items bulk action opens the nest create panel with the parts preselected", async () => {
  const context = loadFunctions(["openLaserNestDialog"], {
    NC: { cands: [], lines: [], active: false },
    ncDefaultState: () => ({ cands: [], lines: [], active: false, tab: "sheets", selPart: 0,
      sheet: {}, parts: {}, partUnit: "auto" }),
    go(name){ context.__route = { name }; },
    toast(){},
  });
  context.openLaserNestDialog(["a", "b"]);
  assert.equal(context.__route.name, "laser-nests");
  assert.equal(context.NC.active, true, "panel marked active");
  assert.equal(context.NC.tab, "parts", "jumps to the Parts tab");
  const lines = JSON.parse(JSON.stringify(context.NC.lines));
  assert.equal(lines.length, 2, "each selected item becomes a part line");
  assert.equal(lines[0].item_id, "a");
  assert.equal(lines[1].item_id, "b");
  assert.equal(lines[0].mirror_mode, "both", "default mirror mode is 'both'");
});

test("nest create panel carries per-line + sheet options into the body", async () => {
  const calls = [];
  const input = v => ({ value: v, trim(){ return this.value; }, checked: false });
  const doc = {
    getElementById(id){
      return ({ nc_name: input("Nest X"), ncQueue: { disabled: false, textContent: "" } })[id]
        || input("");
    },
  };
  const context = loadFunctions(["queueLaserNest", "num"], {
    // Panel state: two lines (same item twice is allowed) + multi-sheet defs
    // + sheet/part options in the flat NC shape.
    NC: {
      cands: [],
      lines: [
        { item_id: "a", quantity: 7, mirror_mode: "mirror", grain: "along_width",
          step_angle: 45, priority: 2, label: "P1", use_bounding_box: true,
          common_edge: false, min_qty: 1, filler_qty: 0 },
        { item_id: "a", quantity: 3, mirror_mode: "original", grain: "any",
          step_angle: 90, priority: 1, label: "", use_bounding_box: false,
          common_edge: true, min_qty: 0, filler_qty: 2 },
      ],
      units: "in",
      sheets: [
        { width: 48, length: 96, asNeeded: false, quantity: 2 },
        { width: 24, length: 48, asNeeded: true, quantity: 1 },
      ],
      margin: 0.39, grain: "along_length", corner: "upper_left", fill: "lr",
      guillotine: true, autoOrigins: true, ssd: 1, originX: 0, originY: 0,
      spacingX: 0.2, spacingY: 0.15, addLabels: true, optPairs: false, fillAll: false,
      partUnit: "in",
    },
    document: doc,
    $: id => doc.getElementById(id),
    api: async (method, url, body) => {
      calls.push([method, url, body]);
      if (url === "/laser-nests/validate") return { ok: true, problems: [] };
      if (url === "/laser-nests") return { nest: { status: "queued" } };
      return {};
    },
    toast(){}, go(){}, ncDefaultState(){ return context.NC; },
  });
  await context.queueLaserNest();
  const create = calls.find(c => c[0] === "POST" && c[1] === "/laser-nests");
  assert.ok(create, "create was called");
  const b = create[2];
  // Mirror modes map to mirror/allow_mirror.
  assert.equal(
    JSON.stringify(b.parts),
    JSON.stringify([
      { item_id: "a", quantity: 7, mirror: true,  allow_mirror: false, grain: "along_width",
        step_angle: 45, priority: 2, label: "P1", use_bounding_box: true,
        common_edge: false, min_qty: 1, filler_qty: 0 },
      { item_id: "a", quantity: 3, mirror: false, allow_mirror: false, grain: "any",
        step_angle: 90, priority: 1, label: "", use_bounding_box: false,
        common_edge: true, min_qty: 0, filler_qty: 2 },
    ])
  );
  // Sheet options: directional spacing, grain, corner, origin, guillotine, labels.
  assert.equal(b.units, "in");
  assert.equal(b.stock_width_mm, 48, "width sent in the chosen unit");
  assert.equal(b.stock_height_mm, 96, "length sent in the chosen unit");
  assert.equal(b.grain_direction, "along_length");
  assert.equal(b.spacing_x_mm, 0.2, "horizontal spacing");
  assert.equal(b.spacing_y_mm, 0.15, "vertical spacing");
  assert.equal(b.nesting_corner, "upper_left");
  assert.equal(b.origin_x_mm, 0);
  assert.equal(b.sheet_sheet_distance_mm, 1);
  assert.equal(b.guillotine, true);
  assert.equal(b.add_part_labels, true);
  assert.equal(b.fill_direction, "lr");
  assert.equal(b.default_part_unit, "in", "non-auto part unit sent");
  // Multiple sheet-size definitions, in the chosen unit, with quantities.
  assert.equal(
    JSON.stringify(b.sheets),
    JSON.stringify([
      { width_mm: 48, length_mm: 96, quantity: 2 },
      { width_mm: 24, length_mm: 48, quantity: null },
    ])
  );
  assert.equal(b.stock_width_mm, 48, "primary sheet = first definition");
  assert.equal(b.stock_height_mm, 96);
});
