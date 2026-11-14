//! Onshape → PDM: export STEP (or STL/PDF) via Onshape's public REST API
//! and check the file in as a version so manufacturing sees the current model.
use std::time::Duration;

use pdm_core::PdmError;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct OnshapeSettings {
    pub base_url: String,
    pub access_key: String,
    pub secret: String,
    pub default_project_id: Option<Uuid>,
    pub folder_sync_enabled: bool,
}

impl OnshapeSettings {
    pub fn from_json(v: &Value) -> Self {
        Self {
            base_url: v
                .get("base_url")
                .and_then(Value::as_str)
                .unwrap_or("https://cad.onshape.com")
                .trim()
                .trim_end_matches('/')
                .to_string(),
            access_key: v
                .get("access_key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            secret: v
                .get("secret")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            default_project_id: v
                .get("default_project_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok()),
            folder_sync_enabled: v.get("folder_sync_enabled").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    pub fn configured(&self) -> bool {
        !self.access_key.is_empty() && !self.secret.is_empty()
    }

    pub fn public(&self) -> Value {
        json!({
            "base_url": self.base_url,
            "access_key": self.access_key,
            "secret_set": !self.secret.is_empty(),
            "configured": self.configured(),
            "default_project_id": self.default_project_id,
            "folder_sync_enabled": self.folder_sync_enabled,
        })
    }
}

pub struct SendReq {
    pub document_id: String,
    pub workspace_id: String,
    pub element_id: String,
    pub element_type: String,
    pub project_id: Option<Uuid>,
    pub item_id: Option<Uuid>,
    pub part_number: Option<String>,
    pub description: Option<String>,
    pub format: String,
    pub part_id: Option<String>,
    pub part_name: Option<String>,
    pub source_element_id: Option<String>,
    /// BOM sub-assembly picks live in another document (usually a version):
    /// these override the panel's document/workspace for the export.
    pub source_document_id: Option<String>,
    pub source_workspace_id: Option<String>,
    pub source_workspace_type: Option<String>,
    pub source_configuration: Option<String>,
}

fn http_err(e: reqwest::Error) -> PdmError {
    PdmError::Internal(format!("onshape: {e}"))
}

/// Where the export actually happens: the panel's element, or — for BOM
/// sub-assembly picks — the source document/version the item lives in.
fn export_target(req: &SendReq) -> (String, String, String, String) {
    let doc = req
        .source_document_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(req.document_id.as_str());
    let ws = req
        .source_workspace_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(req.workspace_id.as_str());
    let ws_seg = req
        .source_workspace_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("w");
    let eid = req
        .source_element_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(req.element_id.as_str());
    (
        doc.to_string(),
        ws.to_string(),
        ws_seg.to_string(),
        eid.to_string(),
    )
}

fn translation_kind(element_type: &str) -> &'static str {
    match element_type.to_ascii_lowercase().as_str() {
        "assembly" | "assemblies" => "assemblies",
        "drawing" | "drawings" => "drawings",
        _ => "partstudios",
    }
}

fn translation_url(
    base: &str,
    kind: &str,
    doc: &str,
    context: &str,
    context_id: &str,
    element: &str,
) -> String {
    format!("{base}/api/{kind}/d/{doc}/{context}/{context_id}/e/{element}/translations")
}

async fn onshape_json(
    client: &reqwest::Client,
    settings: &OnshapeSettings,
    method: reqwest::Method,
    url: &str,
    body: Option<&Value>,
) -> Result<Value, PdmError> {
    let mut req = client
        .request(method, url)
        .basic_auth(&settings.access_key, Some(&settings.secret))
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(b) = body {
        req = req.json(b);
    }
    let res = req.send().await.map_err(http_err)?;
    let status = res.status();
    let text = res.text().await.map_err(http_err)?;
    if !status.is_success() {
        let snippet: String = text.chars().take(280).collect();
        return Err(PdmError::Internal(format!("onshape {status}: {snippet}")));
    }
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| PdmError::Internal(format!("onshape json: {e}")))
}

/// Render the exact selected part or element using the organization's Onshape credentials.
#[derive(serde::Deserialize)]
pub struct PreviewQuery {
    pub document_id: String,
    pub context: String,
    pub context_id: String,
    pub element_id: String,
    pub part_id: Option<String>,
    pub element_type: String,
    pub configuration: Option<String>,
}

pub async fn preview(state: &AppState, org: Uuid, query: &PreviewQuery) -> Result<Value, PdmError> {
    let document = query.document_id.as_str();
    let context = query.context.as_str();
    let context_id = query.context_id.as_str();
    let element = query.element_id.as_str();
    let part = query.part_id.as_deref();
    let kind = query.element_type.as_str();
    let configuration = query.configuration.as_deref();
    if !matches!(context, "w" | "v") || !matches!(kind, "partstudio" | "assembly") {
        return Err(PdmError::Validation(
            "Invalid Onshape preview context or type".into(),
        ));
    }
    for id in [Some(document), Some(context_id), Some(element), part]
        .into_iter()
        .flatten()
    {
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(PdmError::Validation(
                "Invalid Onshape preview identifier".into(),
            ));
        }
    }
    let settings =
        OnshapeSettings::from_json(&pdm_db::get_onshape_settings(&state.pool, org).await?);
    if !settings.configured() {
        return Err(PdmError::BadRequest(
            "Onshape API keys are not configured — set them in Settings".into(),
        ));
    }
    let path = if let Some(part_id) = part.filter(|s| !s.is_empty()) {
        format!(
            "parts/d/{document}/{context}/{context_id}/e/{element}/partid/{part_id}/shadedviews"
        )
    } else {
        let resource = if kind == "assembly" {
            "assemblies"
        } else {
            "partstudios"
        };
        format!("{resource}/d/{document}/{context}/{context_id}/e/{element}/shadedviews")
    };
    let mut url = reqwest::Url::parse(&format!("{}/api/{path}", settings.base_url))
        .map_err(|e| PdmError::Internal(format!("onshape URL: {e}")))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("viewMatrix", "isometric")
            .append_pair("outputWidth", "360")
            .append_pair("outputHeight", "300");
        if let Some(configuration) = configuration.filter(|s| !s.is_empty()) {
            query.append_pair("configuration", configuration);
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| PdmError::Internal(format!("http client: {e}")))?;
    let result = onshape_json(&client, &settings, reqwest::Method::GET, url.as_str(), None).await?;
    let image = result
        .get("images")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .ok_or_else(|| PdmError::Internal("Onshape returned no preview image".into()))?;
    Ok(json!({"image": image}))
}

async fn export_bytes(
    client: &reqwest::Client,
    settings: &OnshapeSettings,
    req: &SendReq,
    format: &str,
) -> Result<(Vec<u8>, String), PdmError> {
    let format_name = format.to_ascii_uppercase();
    let part_id = req
        .part_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (export_doc, export_ws, ws_seg, export_eid) = export_target(req);
    let kind = if part_id.is_some() {
        "partstudios"
    } else {
        translation_kind(&req.element_type)
    };
    let url = translation_url(
        &settings.base_url,
        kind,
        &export_doc,
        &ws_seg,
        &export_ws,
        &export_eid,
    );
    let mut body = json!({
        "formatName": format_name,
        "storeInDocument": false,
    });
    if let Some(pid) = part_id {
        body["partIds"] = json!(pid);
    }
    if let Some(configuration) = req
        .source_configuration
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        body["configuration"] = json!(configuration);
    }
    let start = onshape_json(client, settings, reqwest::Method::POST, &url, Some(&body)).await?;
    let tid = start
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| PdmError::Internal("onshape translation had no id".into()))?
        .to_string();

    let mut result = start;
    for _ in 0..45 {
        let state = result
            .get("requestState")
            .and_then(Value::as_str)
            .unwrap_or("");
        if state.eq_ignore_ascii_case("DONE") {
            break;
        }
        if state.eq_ignore_ascii_case("FAILED") {
            let msg = result
                .get("failureReason")
                .and_then(Value::as_str)
                .unwrap_or("translation failed");
            return Err(PdmError::Internal(format!("onshape export: {msg}")));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        result = onshape_json(
            client,
            settings,
            reqwest::Method::GET,
            &format!("{}/api/translations/{tid}", settings.base_url),
            None,
        )
        .await?;
    }
    let fid = result
        .get("resultExternalDataIds")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .ok_or_else(|| PdmError::Internal("onshape export produced no file".into()))?;
    let bytes = client
        .get(format!(
            "{}/api/documents/d/{}/externaldata/{fid}",
            settings.base_url, export_doc
        ))
        .basic_auth(&settings.access_key, Some(&settings.secret))
        .send()
        .await
        .map_err(http_err)?
        .error_for_status()
        .map_err(http_err)?
        .bytes()
        .await
        .map_err(http_err)?;
    let ext = match format_name.as_str() {
        "STL" => "stl",
        "PDF" => "pdf",
        _ => "step",
    };
    Ok((bytes.to_vec(), ext.into()))
}

/// Picker payload: the document's tabs plus each Part Studio's bodies and each
/// Assembly's top-level BOM entries, so the panel can offer a full picker.
pub async fn picker(
    state: &AppState,
    org: Uuid,
    document_id: &str,
    workspace_id: &str,
) -> Result<Value, PdmError> {
    let settings =
        OnshapeSettings::from_json(&pdm_db::get_onshape_settings(&state.pool, org).await?);
    if !settings.configured() {
        return Err(PdmError::BadRequest(
            "Onshape API keys are not configured — set them in Settings".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| PdmError::Internal(format!("http client: {e}")))?;
    let elements = onshape_json(
        &client,
        &settings,
        reqwest::Method::GET,
        &format!(
            "{}/api/documents/d/{}/w/{}/elements",
            settings.base_url, document_id, workspace_id
        ),
        None,
    )
    .await?;
    let elements = elements.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for el in &elements {
        let etype_raw = el
            .get("elementType")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let etype = etype_raw.to_lowercase().replace('_', "");
        if etype != "assembly" && etype != "partstudio" {
            continue; // skip blobs, BOMs, drawings
        }
        let id = el
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = el
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut parts = Vec::new();
        if etype == "partstudio" && !id.is_empty() {
            let ps = onshape_json(
                &client,
                &settings,
                reqwest::Method::GET,
                &format!(
                    "{}/api/parts/d/{}/w/{}/e/{}",
                    settings.base_url, document_id, workspace_id, id
                ),
                None,
            )
            .await?;
            {
                parts = ps
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|p| {
                        Some(json!({
                            "id": p.get("partId").and_then(Value::as_str)?,
                            "name": p.get("name").and_then(Value::as_str)?,
                        }))
                    })
                    .collect();
            }
        }
        let mut subassemblies = Vec::new();
        if etype == "assembly"
            && !id.is_empty()
            && let Ok(bom) = onshape_json(
                &client,
                &settings,
                reqwest::Method::GET,
                &format!(
                    "{}/api/assemblies/d/{}/w/{}/e/{}/bom",
                    settings.base_url, document_id, workspace_id, id
                ),
                None,
            )
            .await
        {
            let rows = bom
                .get("bomTable")
                .and_then(|t| t.get("items"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for r in rows {
                if r.get("itemSource")
                    .and_then(|s| s.get("indentLevel"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    != 0
                {
                    continue;
                }
                let src = r.get("itemSource");
                let Some(document_id_s) = src
                    .and_then(|s| s.get("documentId"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(element_id_s) =
                    src.and_then(|s| s.get("elementId")).and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(wvm_id) = src.and_then(|s| s.get("wvmId")).and_then(Value::as_str) else {
                    continue;
                };
                subassemblies.push(json!({
                        "name": r.get("name").and_then(Value::as_str).unwrap_or(""),
                        "document_id": document_id_s,
                        "wvm_type": src.and_then(|s| s.get("wvmType")).and_then(Value::as_str).unwrap_or("v"),
                        "wvm_id": wvm_id,
                        "element_id": element_id_s,
                        "part_id": src.and_then(|s| s.get("partId")).and_then(Value::as_str).unwrap_or(""),
                        "configuration": src.and_then(|s| s.get("configuration")).and_then(Value::as_str).unwrap_or(""),
                    }));
            }
        }
        out.push(json!({
            "id": id,
            "name": name,
            "type": etype,
            "parts": parts,
            "subassemblies": subassemblies,
        }));
    }
    Ok(json!({ "elements": out }))
}

pub async fn folder_name(settings: &OnshapeSettings, folder: &str) -> Result<String, PdmError> {
    if !settings.configured() || folder.is_empty() || !folder.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PdmError::Validation("Configure Onshape and enter a valid folder ID".into()));
    }
    let client = reqwest::Client::new();
    let data = onshape_json(&client, settings, reqwest::Method::GET,
        &format!("{}/api/folders/{folder}", settings.base_url), None).await?;
    Ok(data.get("name").and_then(Value::as_str).unwrap_or(folder).to_string())
}

fn is_direct_folder_document(document: &Value, folder: &str) -> bool {
    document.get("jsonType").and_then(Value::as_str) == Some("document-summary")
        && document.get("parentId").and_then(Value::as_str) == Some(folder)
}

/// Import direct child documents; a document's parentId is checked explicitly
/// because Onshape's search can include unrelated results.
pub async fn sync_folder(state: &AppState, org: Uuid, user: Uuid, project: Uuid, folder: &str) -> Result<Value, PdmError> {
    let settings = OnshapeSettings::from_json(&pdm_db::get_onshape_settings(&state.pool, org).await?);
    if !settings.folder_sync_enabled { return Err(PdmError::BadRequest("Onshape folder sync is disabled".into())); }
    folder_name(&settings, folder).await?;
    let client = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().map_err(http_err)?;
    let mut documents = Vec::new();
    let mut offset = 0;
    loop {
        let url = format!("{}/api/documents?filter=5&parentId={folder}&limit=20&offset={offset}", settings.base_url);
        let page = onshape_json(&client, &settings, reqwest::Method::GET, &url, None).await?;
        let entries = page.get("items").and_then(Value::as_array).ok_or_else(|| PdmError::Internal("Onshape document listing has no items".into()))?;
        documents.extend(entries.iter().filter(|d| is_direct_folder_document(d, folder)).cloned());
        if page.get("next").and_then(Value::as_str).filter(|s| !s.is_empty()).is_none() { break; }
        offset += 20;
        if offset > 20000 { return Err(PdmError::Internal("Onshape folder listing exceeds the sync limit".into())); }
    }
    let mut imported = 0usize;
    let mut errors = Vec::new();
    let mut conflicts = Vec::new();
    for doc in &documents {
        let Some(did) = doc.get("id").and_then(Value::as_str) else { continue };
        let full = match onshape_json(&client, &settings, reqwest::Method::GET, &format!("{}/api/documents/{did}", settings.base_url), None).await {
            Ok(v) => v, Err(e) => { errors.push(format!("{did}: {e}")); continue; }
        };
        let Some(wid) = full.get("defaultWorkspace").and_then(|x| x.get("id")).and_then(Value::as_str) else {
            errors.push(format!("{did}: no default workspace")); continue;
        };
        let entries = match picker(state, org, did, wid).await {
            Ok(v) => v, Err(e) => { errors.push(format!("{did}: {e}")); continue; }
        };
        for el in entries.get("elements").and_then(Value::as_array).into_iter().flatten() {
            let eid = el.get("id").and_then(Value::as_str).unwrap_or("");
            let typ = el.get("type").and_then(Value::as_str).unwrap_or("");
            let mut targets: Vec<(Option<&str>, &str)> = Vec::new();
            if typ == "assembly" { targets.push((None, el.get("name").and_then(Value::as_str).unwrap_or("Assembly"))); }
            if typ == "partstudio" {
                for p in el.get("parts").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(id) = p.get("id").and_then(Value::as_str) { targets.push((Some(id), p.get("name").and_then(Value::as_str).unwrap_or(id))); }
                }
            }
            for (part, name) in targets {
                let current = OnshapeSettings::from_json(&pdm_db::get_onshape_settings(&state.pool, org).await?);
                if !current.folder_sync_enabled { return Err(PdmError::BadRequest("Onshape folder sync was disabled".into())); }
                let existing = pdm_db::find_item_by_onshape(&state.pool, org, did, eid, part).await?;
                if let Some(item) = &existing {
                    if item.project_id != project { conflicts.push(format!("{name}: already in another project ({})", item.part_number)); continue; }
                }
                let request = SendReq { document_id: did.into(), workspace_id: wid.into(), element_id: eid.into(), element_type: typ.into(),
                    project_id: Some(project), item_id: existing.map(|i| i.id), part_number: None, description: Some(name.into()),
                    format: "STEP".into(), part_id: part.map(str::to_string), part_name: None, source_element_id: None,
                    source_document_id: None, source_workspace_id: None, source_workspace_type: None, source_configuration: None };
                match send_to_pdm(state, org, user, &settings, request).await {
                    Ok(_) => imported += 1,
                    Err(e) => errors.push(format!("{name}: {e}")),
                }
            }
        }
    }
    Ok(json!({"documents": documents.len(), "imported": imported, "conflicts": conflicts, "errors": errors}))
}

/// Export the Onshape element and check it in as a PDM version.
pub async fn send_to_pdm(
    state: &AppState,
    org: Uuid,
    user_id: Uuid,
    settings: &OnshapeSettings,
    req: SendReq,
) -> Result<Value, PdmError> {
    if let Some(kind) = req
        .source_workspace_type
        .as_deref()
        .filter(|s| !s.is_empty())
        && kind != "w"
        && kind != "v"
    {
        return Err(PdmError::Validation(
            "Onshape source must be a workspace or version".into(),
        ));
    }
    if !settings.configured() {
        return Err(PdmError::BadRequest(
            "Onshape API keys are not configured — set them in Settings".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| PdmError::Internal(format!("http client: {e}")))?;
    let format = if req.format.trim().is_empty() {
        "STEP"
    } else {
        req.format.trim()
    };
    let (bytes, ext) = export_bytes(&client, settings, &req, format).await?;
    if bytes.is_empty() {
        return Err(PdmError::Internal("onshape export was empty".into()));
    }

    let classification = if req.element_type.to_ascii_lowercase().contains("assembl") {
        "ASM"
    } else {
        "BE"
    };
    let (lookup_doc, _lookup_ws, _lookup_ws_seg, lookup_element) = export_target(&req);
    let item_id = if let Some(id) = req.item_id {
        id
    } else if let Some(existing) = pdm_db::find_item_by_onshape(
        &state.pool,
        org,
        &lookup_doc,
        &lookup_element,
        req.part_id.as_deref(),
    )
    .await?
    {
        existing.id
    } else {
        let project_id = req.project_id.ok_or_else(|| {
            PdmError::BadRequest(
                "pick a PDM project (or set a default in Settings ▸ Onshape)".into(),
            )
        })?;
        let prefixes = pdm_db::list_part_number_prefixes(&state.pool, org).await?;
        let prefix = pdm_core::prefix_for_category(&prefixes, classification);
        let part_number = match req
            .part_number
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(pn) => pn.to_string(),
            None => match req
                .part_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(name) => name.to_string(),
                None => {
                    let seq = pdm_db::next_part_number(&state.pool, org, &prefix).await?;
                    pdm_core::generate_part_number(&prefix, seq)
                }
            },
        };
        let desc = req.description.clone().unwrap_or_else(|| {
            req.part_name
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("Onshape {}/{}", lookup_doc, lookup_element))
        });
        pdm_db::create_item(
            &state.pool,
            org,
            project_id,
            None,
            &part_number,
            &desc,
            classification,
            Some(user_id),
        )
        .await?
        .id
    };

    let (sha, size) = state
        .store
        .put_content_addressed(&bytes)
        .await
        .map_err(|e| PdmError::Storage(e.to_string()))?;
    let blob = pdm_db::upload_blob(
        &state.pool,
        org,
        &sha,
        i64::try_from(size).map_err(|_| PdmError::Validation("file too large".into()))?,
        &sha,
    )
    .await?;

    let item = pdm_db::get_item(&state.pool, item_id, org)
        .await?
        .ok_or_else(|| PdmError::NotFound(format!("item {item_id}")))?;
    let filename = format!("{}.{}", item.part_number, ext);
    let mime = match ext.as_str() {
        "stl" => "model/stl",
        "pdf" => "application/pdf",
        _ => "application/step",
    };

    pdm_db::acquire_lock(
        &state.pool,
        item_id,
        user_id,
        "onshape",
        None,
        state.lease_hours,
    )
    .await?;
    let file = pdm_db::ensure_file(&state.pool, item_id, &filename, mime).await?;
    let head = pdm_db::get_head_version(&state.pool, file.id).await?;
    let version = pdm_db::create_version(
        &state.pool,
        file.id,
        blob.id,
        user_id,
        &format!("Onshape {} {}/{}", format, req.document_id, req.element_id),
        head.as_ref().map(|v| v.id),
        "",
    )
    .await?;
    let job_payload = json!({ "version_id": version.id, "item_id": item_id });
    let _ = pdm_db::enqueue_job(&state.pool, org, "design.analyze", &job_payload).await;
    let _ = pdm_db::enqueue_job(&state.pool, org, "preview.render", &job_payload).await;
    let _ = pdm_db::release_lock(&state.pool, item_id, user_id).await;

    for (k, v) in [
        ("source", json!("onshape")),
        ("onshape.document_id", json!(lookup_doc)),
        ("onshape.workspace_id", json!(_lookup_ws)),
        ("onshape.workspace_type", json!(_lookup_ws_seg)),
        ("onshape.element_id", json!(lookup_element)),
        ("onshape.format", json!(format)),
        (
            "onshape.part_id",
            json!(req.part_id.clone().unwrap_or_default()),
        ),
        (
            "onshape.part_name",
            json!(req.part_name.clone().unwrap_or_default()),
        ),
    ] {
        let _ = pdm_db::upsert_property(&state.pool, item_id, k, "string", &v).await;
    }

    Ok(json!({
        "item_id": item_id,
        "part_number": item.part_number,
        "version": version,
        "filename": filename,
        "size_bytes": size,
        "sha256": sha,
    }))
}

#[cfg(test)]
mod tests {
    use super::{translation_url, is_direct_folder_document};
    use serde_json::json;

    #[test]
    fn folder_membership_excludes_unrelated_documents_and_folders() {
        assert!(is_direct_folder_document(&json!({"jsonType":"document-summary","parentId":"folder"}), "folder"));
        assert!(!is_direct_folder_document(&json!({"jsonType":"document-summary","parentId":"nested"}), "folder"));
        assert!(!is_direct_folder_document(&json!({"jsonType":"folder","parentId":"folder"}), "folder"));
        assert!(!is_direct_folder_document(&json!({"jsonType":"document-summary"}), "folder"));
    }

    #[test]
    fn translation_paths_separate_context_from_id() {
        assert_eq!(
            translation_url(
                "https://cad.onshape.com",
                "partstudios",
                "doc",
                "w",
                "workspace",
                "element"
            ),
            "https://cad.onshape.com/api/partstudios/d/doc/w/workspace/e/element/translations"
        );
        assert_eq!(
            translation_url(
                "https://cad.onshape.com",
                "assemblies",
                "doc",
                "v",
                "version",
                "element"
            ),
            "https://cad.onshape.com/api/assemblies/d/doc/v/version/e/element/translations"
        );
    }
}
