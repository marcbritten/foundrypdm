/* Self-hosted Craft web apps: document bytes cross the bridge, never API tokens. */
"use strict";
(() => {
  const APPS = {
    pdfcraft: { id:"pdfcraft", label:"PdfCraft", extension:"pdf", mime:"application/pdf" },
    cadcraft: { id:"cadcraft", label:"CADCraft", extension:"dxf", mime:"application/dxf" },
  };
  function forFile(mime, name){
    const ext = String(name || "").split(".").pop().toLowerCase();
    if (ext === "pdf" || String(mime || "").toLowerCase() === "application/pdf") return APPS.pdfcraft;
    if (ext === "dxf" || /^(application|image)\/(x-)?dxf$/i.test(mime || "")) return APPS.cadcraft;
    return null;
  }
  function download(bytes, name){
    const a = document.createElement("a");
    const url = URL.createObjectURL(new Blob([bytes]));
    a.href = url; a.download = name; a.click();
    setTimeout(() => URL.revokeObjectURL(url), 4000);
  }
  let active = false;
  async function open({ app, name, version, load, checkin, onSaved }){
    if (active) return;
    active = true;
    const dialog = document.createElement("dialog");
    dialog.className = "web-editor";
    dialog.setAttribute("aria-label", app.label + " web editor");
    dialog.innerHTML = "<div class='web-editor-layout'><div class='web-editor-bar'>" +
      "<strong></strong><button data-original>Download original</button>" +
      "<button data-edited disabled>Download edited file</button>" +
      "<button data-checkin disabled>Check in edited version</button>" +
      "<button data-close>Close editor</button></div>" +
      "<div class='web-editor-status' role='status'></div><iframe></iframe></div>";
    const find = s => dialog.querySelector(s);
    const frame = find("iframe"), status = find("[role=status]");
    const edited = find("[data-edited]"), save = find("[data-checkin]");
    find("strong").textContent = app.label + " — " + name + " · v" + version.seq;
    frame.title = app.label + " — " + name;
    let original = null, pending = null, dirty = false, busy = false, closed = false;
    let ready = false, opening = false, timer;
    const update = () => {
      edited.disabled = !pending;
      save.disabled = !pending || !checkin || busy || dirty;
    };
    const guard = e => { if (dirty || pending || busy){ e.preventDefault(); e.returnValue = ""; } };
    const close = () => {
      if (busy) return;
      if ((dirty || pending) && !confirm("Close the editor and discard changes not checked in? Download a copy first to keep them.")) return;
      closed = true; clearTimeout(timer);
      window.removeEventListener("message", receive);
      window.removeEventListener("beforeunload", guard);
      dialog.close(); dialog.remove(); active = false;
    };
    const send = async () => {
      if (!ready || opening || closed) return;
      opening = true;
      try {
        original = await load();
        if (closed) return;
        frame.contentWindow.postMessage({ type:app.id + ":open", name, bytes:original }, location.origin);
        status.textContent = "Use the editor’s Save command, then download the edited file" +
          (checkin ? " or check it in as a new PDM version." : ". Vault check-in requires an editor role.");
      } catch(e){ status.textContent = "Could not open file: " + e.message; opening = false; }
    };
    const receive = e => {
      if (closed || e.origin !== location.origin || e.source !== frame.contentWindow) return;
      const m = e.data;
      if (!m || typeof m.type !== "string") return;
      if (m.type === app.id + ":ready"){
        clearTimeout(timer); if (!ready){ ready = true; send(); }
      } else if (m.type === app.id + ":failed"){
        clearTimeout(timer); status.textContent = app.label + " failed to start: " + String(m.error || "unknown error");
      } else if (m.type === app.id + ":dirty"){
        dirty = m.dirty === true; update();
      } else if (m.type === app.id + ":save"){
        if (!(m.bytes instanceof ArrayBuffer) && !(m.bytes instanceof Uint8Array)) return;
        const savedName = typeof m.name === "string" ? m.name : name;
        // Save As, other open tabs and drawing exports are downloads, not replacements of this item.
        if (savedName !== name || m.saveAs === true){ download(m.bytes, savedName); return; }
        pending = m.bytes; dirty = false;
        status.textContent = "Edited file received. Download it" + (checkin ? " or check it in to PDM." : ".");
        update();
      }
    };
    find("[data-close]").onclick = close;
    dialog.addEventListener("cancel", e => { e.preventDefault(); close(); });
    find("[data-original]").onclick = async () => {
      try { download(original || await load(), name); }
      catch(e){ status.textContent = "Download failed: " + e.message; }
    };
    edited.onclick = () => { if (pending) download(pending, name); };
    save.hidden = !checkin;
    save.onclick = async () => {
      if (!pending || busy || dirty) return;
      const message = prompt("Commit message", "Edited in " + app.label);
      if (message === null) return;
      const snapshot = pending;
      busy = true; update();
      status.textContent = "Checking in edited version…";
      try {
        const result = await checkin(snapshot, message);
        if (pending === snapshot) pending = null;
        status.textContent = "Checked in v" + result.seq + ".";
        if (onSaved) onSaved();
      } catch(e){
        status.textContent = "Check-in failed: " + e.message + "\nYour edited file is still available to download or retry.";
      } finally { busy = false; update(); }
    };
    document.body.appendChild(dialog); dialog.showModal();
    window.addEventListener("message", receive);
    window.addEventListener("beforeunload", guard);
    status.textContent = "Loading " + app.label + "…";
    const url = "/static/editors/" + app.id + "/index.html?host=parent";
    timer = setTimeout(() => {
      status.textContent = app.label + " has not started. Check WebGL support and that the web editor is installed on this PDM server. Download original is still available.";
    }, 60000);
    try {
      const response = await fetch(url, { method:"HEAD" });
      if (closed) return;
      if (!response.ok) throw new Error("web build is not installed (HTTP " + response.status + ")");
      frame.src = url;
    } catch(e){ clearTimeout(timer); status.textContent = "Cannot load " + app.label + ": " + e.message; }
  }
  // ── SolidCraft web: the CAD app itself, pinned into the image and run in a
  // same-origin iframe. The app fetches the version's design bytes from PDM's
  // public design route (its design-link fetch sends no credentials, and the URL
  // names the exact immutable file), so no token crosses into the app. Saving
  // back goes through the browser storage the app and this portal share: the
  // app keeps stored designs in localStorage on insecure origins and in OPFS on
  // secure ones.
  function scStorageBytes(name){
    const enc = window.localStorage.getItem("solidcraft.file." + name);
    if (!enc || enc.length < 2) return null;
    if (enc[0] === "t") return new TextEncoder().encode(enc.slice(1));
    if (enc[0] === "x"){
      const hex = enc.slice(1);
      if (hex.length % 2) return null;
      const out = new Uint8Array(hex.length / 2);
      for (let i = 0; i < out.length; i++){
        const n = parseInt(hex.slice(2 * i, 2 * i + 2), 16);
        if (isNaN(n)) return null;
        out[i] = n;
      }
      return out;
    }
    return null;
  }
  async function scOpfsBytes(name){
    const sm = navigator.storage;
    if (!sm || typeof sm.getDirectory !== "function") return null;
    try {
      const dir = await sm.getDirectory();
      const handle = await dir.getFileHandle(name);
      const file = await handle.getFile();
      return new Uint8Array(await file.arrayBuffer());
    } catch(e){ return null; }
  }
  async function scReadFile(name){
    const local = scStorageBytes(name);
    if (local && local.byteLength) return local;
    return await scOpfsBytes(name);
  }
  async function scRemoveFile(name){
    try { window.localStorage.removeItem("solidcraft.file." + name); } catch(e){}
    const sm = navigator.storage;
    if (sm && typeof sm.getDirectory === "function"){
      sm.getDirectory().then(dir => dir.removeEntry(name).catch(() => {})).catch(() => {});
    }
  }
  // Ask the app to write the open design to its browser storage under a name that
  // only this save attempt uses; its storage flush lands a frame after the engine
  // write, so poll until the bytes arrive.
  async function scEditedBytes(frame, name){
    const win = frame.contentWindow;
    if (!win || !win.solidcraft)
      throw new Error("SolidCraft is still starting. Wait for the model to appear, then try again.");
    try {
      await win.solidcraft.execute("file.save_as", { path: "/opfs/" + name });
    } catch(e){ throw new Error(String(e.message || e)); }
    for (let i = 0; ; i++){
      const bytes = await scReadFile(name);
      if (bytes && bytes.byteLength){
        await scRemoveFile(name);
        return bytes;
      }
      if (i >= 75)
        throw new Error("The edited design did not arrive in browser storage. Check SolidCraft's status line, or use File ▸ Download design there and upload it as a new version.");
      await new Promise(r => setTimeout(r, 400));
    }
  }
  async function openSolidcraft({ version, name, designUrl, designLink, checkin, createNew, onSaved }){
    if (active) return;
    active = true;
    const isNew = !version; // blank session: Save to PDM creates the item (createNew)
    const dialog = document.createElement("dialog");
    dialog.className = "web-editor";
    dialog.setAttribute("aria-label", "SolidCraft web editor");
    dialog.innerHTML = "<div class='web-editor-layout'><div class='web-editor-bar'>" +
      "<strong></strong><button data-original>Download original</button>" +
      "<button data-edited disabled>Download edited file</button>" +
      "<button data-checkin disabled>Save to PDM</button>" +
      "<button data-close>Close editor</button></div>" +
      "<div class='web-editor-status' role='status'></div><iframe></iframe></div>";
    const find = s => dialog.querySelector(s);
    const frame = find("iframe"), status = find("[role=status]");
    const edited = find("[data-edited]"), save = find("[data-checkin]");
    const downloadName = name || "new-part.sc";
    let displayName = name;
    find("strong").textContent = isNew ? "SolidCraft — new design" : "SolidCraft — " + displayName + " · v" + version.seq;
    frame.title = isNew ? "SolidCraft — new design" : "SolidCraft — " + displayName;
    find("[data-original]").hidden = isNew;
    let original = null, pending = null, busy = false, closed = false;
    const update = () => {
      edited.disabled = busy;
      save.disabled = busy || !(checkin || createNew);
    };
    const close = () => {
      if (busy) return;
      if (pending && !confirm("Close the editor and discard the design not checked in? Download a copy first to keep it.")) return;
      closed = true;
      dialog.close(); dialog.remove(); active = false;
    };
    find("[data-close]").onclick = close;
    dialog.addEventListener("cancel", e => { e.preventDefault(); close(); });
    find("[data-original]").onclick = async () => {
      // Design links expire, so ask for a fresh one when the host can make it.
      try { download(original || await fetch(designLink ? await designLink() : designUrl).then(r => { if (!r.ok) throw new Error("HTTP " + r.status); return r.arrayBuffer(); }), name); }
      catch(e){ status.textContent = "Download failed: " + e.message; }
    };
    const grab = async () => {
      if (busy) return;
      busy = true; update();
      try {
        const snapshot = await scEditedBytes(frame, "pdm-checkin-" + Date.now() + ".sc");
        download(snapshot, downloadName);
        status.textContent = "Edited file downloaded." + (checkin ? " Use Save to PDM to check in the current design." : "");
      } catch(e){
        if (pending){
          download(pending, downloadName);
          status.textContent = "Downloaded the last collected design. Could not collect the latest edits: " + e.message;
        } else status.textContent = "Download failed: " + e.message;
      } finally { busy = false; update(); }
    };
    edited.onclick = grab;
    save.hidden = !(checkin || createNew);
    save.onclick = async () => {
      if (busy) return;
      if (isNew && createNew){
        busy = true; update();
        status.textContent = "Collecting the edited design…";
        let snapshot;
        try { snapshot = await scEditedBytes(frame, "pdm-checkin-" + Date.now() + ".sc"); }
        catch(e){
          busy = false; update();
          status.textContent = "Could not collect the design: " + e.message;
          return;
        }
        pending = snapshot;
        status.textContent = "Saving to PDM…";
        try {
          const result = await createNew(snapshot, dialog);
          if (result === null){
            busy = false; update();
            status.textContent = "Save cancelled. Use Download edited file to keep a copy, or Save to PDM again.";
            return;
          }
          if (pending === snapshot) pending = null;
          // First save made the item: later saves check in new versions of it.
          createNew = null;
          checkin = result.checkin || null;
          displayName = result.item.part_number;
          status.textContent = "Saved to PDM: " + result.item.part_number + " v" + result.version.seq + "." +
            (result.checkin ? " Save to PDM again to check in more versions of this item." : "");
          find("strong").textContent = "SolidCraft — " + result.item.part_number + " · v" + result.version.seq;
          if (onSaved) onSaved();
        } catch(e){
          busy = false; update();
          status.textContent = "Save failed: " + e.message + "\nYour edited file is still available to download or retry.";
          return;
        }
        busy = false; update();
        return;
      }
      if (!checkin) return;
      const message = prompt("Commit message", "Edited in SolidCraft web");
      if (message === null) return;
      busy = true; update();
      status.textContent = "Collecting the edited design…";
      try {
        const snapshot = await scEditedBytes(frame, "pdm-checkin-" + Date.now() + ".sc");
        pending = snapshot;
        status.textContent = "Checking in edited version…";
        const result = await checkin(snapshot, message);
        if (pending === snapshot) pending = null;
        status.textContent = "Saved to PDM as v" + result.seq + ".";
        find("strong").textContent = "SolidCraft — " + displayName + " · v" + result.seq;
        if (onSaved) onSaved();
      } catch(e){
        busy = false; update();
        status.textContent = "Check-in failed: " + e.message + "\nYour edited file is still available to download or retry.";
        return;
      }
      busy = false; update();
    };
    document.body.appendChild(dialog); dialog.showModal(); update();
    status.textContent = "Loading SolidCraft…";
    // A blank session (no ?open=) starts on the app's home screen, where the
    // user creates the new design before saving it to PDM.
    const url = isNew
      ? "/static/editors/solidcraft/index.html"
      : "/static/editors/solidcraft/index.html?open=" + encodeURIComponent(designUrl);
    try {
      const response = await fetch("/static/editors/solidcraft/index.html", { method:"HEAD" });
      if (closed) return;
      if (!response.ok) throw new Error("web build is not installed (HTTP " + response.status + ")");
      frame.src = url;
      if (isNew) status.textContent = "Create a design in SolidCraft, then use Save to PDM to store it as a new item (auto part number when left blank).";
    } catch(e){ status.textContent = "Cannot load SolidCraft: " + e.message + (isNew ? "" : " — Download original is still available."); }
    dialog.addEventListener("close", () => { active = false; });
  }

  // Use the same DXF renderer as the editor, inside the Preview tab. The host
  // bridge does not expose a read-only mode: local edits are temporary here;
  // vault writes are only offered by the separate editor workspace.
  function preview({ container, name, load, onStatus }){
    const frame = document.createElement("iframe");
    frame.title = "DXF drawing preview (CADCraft)";
    let disposed = false, started = false;
    const controller = new AbortController();
    const status = message => { if (!disposed && container.isConnected) onStatus(message); };
    const dispose = () => {
      if (disposed) return;
      disposed = true; controller.abort(); clearTimeout(timer);
      window.removeEventListener("message", receive);
      observer.disconnect(); frame.remove();
    };
    const observer = new MutationObserver(() => { if (!container.isConnected) dispose(); });
    const receive = async e => {
      if (disposed || !container.isConnected || e.origin !== location.origin || e.source !== frame.contentWindow) return;
      const m = e.data;
      if (!m || typeof m.type !== "string") return;
      if (m.type === "cadcraft:ready" && !started){
        started = true; clearTimeout(timer);
        try {
          const bytes = await load(controller.signal);
          if (disposed || !container.isConnected) return;
          frame.contentWindow.postMessage({ type:"cadcraft:open", name, bytes }, location.origin);
          status("DXF drawing — use CADCraft’s pan, zoom and fit tools. To save edits, use Open in web editor.");
        } catch(e){ if (!disposed) status("DXF preview failed: " + e.message + ". Try Open in web editor or Download file."); }
      } else if (m.type === "cadcraft:failed"){
        clearTimeout(timer);
        status("DXF preview failed: " + String(m.error || "CADCraft could not start") + ". Try Open in web editor or Download file.");
      } else if (m.type === "cadcraft:dirty" && m.dirty === true){
        status("Changes in this preview are temporary. Use Open in web editor to edit and check in a new version.");
      } else if (m.type === "cadcraft:save"){
        status("This preview does not check in changes. Use Open in web editor to edit, download and save a new version.");
      }
    };
    const timer = setTimeout(() => status("DXF preview has not started. Check browser WebGL support, or use Open in web editor or Download file."), 60000);
    container.replaceChildren(frame);
    observer.observe(document.body, { childList:true, subtree:true });
    window.addEventListener("message", receive);
    status("Loading DXF drawing in CADCraft…");
    fetch("/static/editors/cadcraft/index.html", { method:"HEAD", signal:controller.signal }).then(r => {
      if (disposed) return;
      if (!r.ok) throw new Error("CADCraft web build is unavailable (HTTP " + r.status + ")");
      frame.src = "/static/editors/cadcraft/index.html?host=parent";
    }).catch(e => { if (!disposed){ clearTimeout(timer); status("DXF preview failed: " + e.message); } });
    return dispose;
  }
  window.PdmEditors = { forFile, open, openSolidcraft, preview };
})();
