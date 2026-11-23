//! End-to-end Manufacturing/CAM workflow test (ToDoMFGCAM "demonstrate one complete workflow").
//!
//! Requires: PDM_E2E=1, PDM_E2E_BASE (default http://127.0.0.1:8080), PDM_E2E_ADMIN_TOKEN.
//!
//! Walks the full chain: part + released CAD revision → machine/tool/post-processor →
//! mfg setup (pinned to the revision) → CAM project (tool selection) → NC program
//! (pinned to revision + machine + post-processor version) → release → production job
//! (pending → in_progress → done) → passing inspection → part status "completed"; then a
//! new design revision flags the NC program as outdated (the data-integrity invariant).

use reqwest::Client;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Duration;

fn base() -> String {
    std::env::var("PDM_E2E_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}
fn admin_token() -> String {
    std::env::var("PDM_E2E_ADMIN_TOKEN").expect("PDM_E2E_ADMIN_TOKEN must be set")
}
fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

async fn post(client: &Client, path: &str, body: serde_json::Value) -> serde_json::Value {
    let r = client
        .post(format!("{}{}", base(), path))
        .bearer_auth(admin_token())
        .json(&body)
        .send()
        .await
        .expect("request");
    let status = r.status();
    assert!(status.is_success(), "POST {path}: {status}");
    r.json().await.expect("json")
}

async fn get(client: &Client, path: &str) -> serde_json::Value {
    let r = client
        .get(format!("{}{}", base(), path))
        .bearer_auth(admin_token())
        .send()
        .await
        .expect("request");
    let status = r.status();
    assert!(status.is_success(), "GET {path}: {status}");
    r.json().await.expect("json")
}

async fn put_blob(client: &Client, bytes: &[u8]) -> String {
    let r = client
        .put(format!("{}/blobs", base()))
        .bearer_auth(admin_token())
        .header("Content-Type", "application/octet-stream")
        .body(bytes.to_vec())
        .send()
        .await
        .expect("upload");
    let status = r.status();
    assert!(status.is_success(), "PUT /blobs: {status}");
    let body: serde_json::Value = r.json().await.expect("blob json");
    body["blob"]["sha256"].as_str().unwrap().to_string()
}

#[tokio::test]
#[ignore]
async fn e2e_mfg_full_workflow() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    // Unique suffix so re-runs against a shared host don't hit the (org, name) uniques.
    let run = std::process::id();

    // 1. Project + part.
    let proj = post(
        &client,
        "/projects",
        json!({ "name": format!("mfg-e2e-{run}"), "description": "MFG e2e" }),
    )
    .await;
    let project_id = proj["project"]["id"].as_str().unwrap().to_string();
    let item = post(
        &client,
        &format!("/projects/{project_id}/items"),
        json!({ "description": "MFG bracket", "classification": "BE" }),
    )
    .await;
    let item_id = item["item"]["id"].as_str().unwrap().to_string();

    // 2. Upload a design + check in a released revision.
    let design = br#"{"format":"solvecraft/5","name":"mfg-e2e","units":"mm"}"#;
    let sha = sha256_hex(design);
    assert_eq!(put_blob(&client, design).await, sha, "upload integrity");
    let co = post(
        &client,
        &format!("/items/{item_id}/checkout"),
        json!({ "workstation": "mfg-e2e" }),
    )
    .await;
    assert!(co["lock"].is_object(), "no lock on checkout");
    let v1 = post(&client, &format!("/items/{item_id}/checkin"), json!({ "sha256": sha, "commit_message": "bracket rev A", "base_version": null, "filename": "bracket.solidcraft" })).await;
    assert_eq!(v1["version"]["seq"], 1, "first version is seq 1");
    let v1_id = v1["version"]["id"].as_str().unwrap().to_string();

    // 3. Manufacturing reference data.
    let machine = post(&client, "/machines", json!({ "name": format!("Milling cell {run}"), "kind": "cnc_mill", "manufacturer": "Haas", "model": "VF-2" })).await;
    let machine_id = machine["machine"]["id"].as_str().unwrap().to_string();
    let tool = post(&client, "/tools", json!({ "name": format!("6mm 3FL endmill {run}"), "kind": "end_mill", "dimensions": { "diameter_mm": 6.0 } })).await;
    let tool_id = tool["tool"]["id"].as_str().unwrap().to_string();
    let pp = post(&client, "/post-processors", json!({ "name": format!("Haas VF-2 G-code {run}"), "dialect": "gcode", "machine_kind": "cnc_mill" })).await;
    let pp_id = pp["post_processor"]["id"].as_str().unwrap().to_string();

    // 4. Mfg setup (pinned to the released revision) → CAM project (tool selection).
    let setup = post(&client, "/mfg-setups", json!({ "item_id": item_id, "file_version_id": v1_id, "name": format!("Vise + G54 {run}"), "stock": { "width_mm": 100 }, "wcs": "G54" })).await;
    let setup_id = setup["mfg_setup"]["id"].as_str().unwrap().to_string();
    let cam = post(&client, "/cam-projects", json!({ "mfg_setup_id": setup_id, "name": format!("2D pockets {run}"), "source": "external", "tool_ids": [tool_id] })).await;
    let cam_id = cam["cam_project"]["id"].as_str().unwrap().to_string();

    // 5. NC program pinned to the exact revision + machine + post-processor version → release.
    let nc = post(&client, "/nc-programs", json!({
        "item_id": item_id, "file_version_id": v1_id, "mfg_setup_id": setup_id, "cam_project_id": cam_id,
        "machine_id": machine_id, "post_processor_id": pp_id, "post_processor_version": "1.0", "gcode_sha256": "abc123"
    })).await;
    let nc_id = nc["nc_program"]["id"].as_str().unwrap().to_string();
    let rel = post(&client, &format!("/nc-programs/{nc_id}/release"), json!({})).await;
    assert_eq!(rel["nc_program"]["released"], true, "NC released");

    // 6. Production job: pending → in_progress → done.
    let job = post(
        &client,
        "/production-jobs",
        json!({ "nc_program_id": nc_id, "machine_id": machine_id, "operator": "test-operator" }),
    )
    .await;
    let job_id = job["production_job"]["id"].as_str().unwrap().to_string();
    assert_eq!(job["production_job"]["status"], "pending");
    let j2 = post(
        &client,
        &format!("/production-jobs/{job_id}/status"),
        json!({ "status": "in_progress" }),
    )
    .await;
    assert_eq!(j2["production_job"]["status"], "in_progress");
    let j3 = post(
        &client,
        &format!("/production-jobs/{job_id}/status"),
        json!({ "status": "done" }),
    )
    .await;
    assert_eq!(j3["production_job"]["status"], "done");
    assert!(
        j3["production_job"]["completed_at"].is_string(),
        "completed_at set on done"
    );

    // 7. Passing inspection → part is "completed".
    post(&client, "/inspections", json!({ "production_job_id": job_id, "item_id": item_id, "passed": true, "notes": "within tolerance" })).await;
    let status = get(&client, &format!("/items/{item_id}/manufacturing")).await;
    assert_eq!(
        status["status"], "completed",
        "part completed after passing inspection"
    );

    // 8. Org dashboard reflects it.
    let dash = get(&client, "/manufacturing/dashboard").await;
    assert_eq!(
        dash["buckets"]["completed"], 1,
        "dashboard counts the completed part"
    );

    // 9. A new design *major version* flags the NC program as outdated (data
    //    integrity). Plain revisions within a version stay compatible.
    let design2 = br#"{"format":"solvecraft/5","name":"mfg-e2e-v2","units":"mm"}"#;
    let sha2 = sha256_hex(design2);
    assert_eq!(
        put_blob(&client, design2).await,
        sha2,
        "upload v2 integrity"
    );
    post(
        &client,
        &format!("/items/{item_id}/checkout"),
        json!({ "workstation": "mfg-e2e" }),
    )
    .await;
    let v2 = post(&client, &format!("/items/{item_id}/checkin"), json!({ "sha256": sha2, "commit_message": "bracket rev B", "base_version": 1, "filename": "bracket.solidcraft", "new_version": true })).await;
    assert_eq!(v2["version"]["seq"], 2, "second save is seq 2");
    assert_eq!(
        v2["version"]["version"], 2,
        "new_version save bumps the major version"
    );
    assert_eq!(
        v2["version"]["revision"], 1,
        "new_version save resets the revision"
    );
    let ncs = get(&client, "/nc-programs").await;
    let outdated = ncs["nc_programs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["id"] == nc_id && n["outdated"] == json!(true));
    assert!(
        outdated,
        "NC program flagged outdated after a new design revision"
    );

    eprintln!(
        "MFG e2e: PASS — part → setup → CAM → NC → job → inspection → completed; NC flagged outdated on rev B"
    );
}

/// Sheet-metal standards library (migration 0009): a fresh org is seeded with the
/// common material × thickness catalog on first GET; POST adds an org-specific row.
#[tokio::test]
#[ignore]
async fn e2e_sheet_material_standards() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let run = std::process::id();

    // 1. First GET seeds the org's library from the global catalog.
    let lib = get(&client, "/sheet-materials").await;
    let mats = lib["materials"].as_array().expect("materials array");
    assert!(
        !mats.is_empty(),
        "seeded standards library must not be empty"
    );

    // A well-known seed row must be present with sane bend properties.
    let steel = mats.iter().find(|m| m["material"] == "Mild steel A36");
    let Some(steel) = steel else {
        panic!("seed catalog missing 'Mild steel A36'");
    };
    assert_eq!(
        steel["density_g_cm3"].as_f64().expect("density"),
        7.85,
        "steel density"
    );
    let t15 = steel["thicknesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["thickness_mm"].as_f64() == Some(1.5))
        .expect("1.5 mm row");
    assert_eq!(
        t15["thickness_gauge"].as_str(),
        Some("16 ga"),
        "US gauge label"
    );
    assert_eq!(t15["k_factor"].as_f64().expect("k"), 0.4, "steel K-factor");
    assert!(
        (1.0..=4.0).contains(&t15["bend_allowance_mm"].as_f64().expect("ba")),
        "1.5 mm steel 90° bend allowance in a sane range, got {}",
        t15["bend_allowance_mm"]
    );

    // Org isolation: every row's group labels match a known material group.
    let groups: std::collections::HashSet<&str> =
        mats.iter().filter_map(|m| m["group"].as_str()).collect();
    assert!(
        groups
            .iter()
            .all(|g| matches!(*g, "Aluminum" | "Steel" | "Stainless" | "Brass" | "Copper")),
        "unexpected material group: {groups:?}"
    );

    // 2. POST adds an org-specific standard.
    let exotic = format!("e2e Exotic alloy {run}");
    let created = post(
        &client,
        "/sheet-materials",
        json!({
            "material_group": "e2e",
            "material": exotic,
            "thickness_mm": 2.2,
            "thickness_gauge": null,
            "density_g_cm3": 4.5,
            "min_bend_radius_mm": 1.1,
            "k_factor": 0.42,
            "bend_deduction_mm": 3.5,
            "bend_allowance_mm": 4.1,
            "notes": "e2e test standard"
        }),
    )
    .await;
    let std_row = &created["standard"];
    assert_eq!(std_row["material"].as_str(), Some(exotic.as_str()));
    assert_eq!(std_row["thickness_mm"].as_f64(), Some(2.2));
    assert_eq!(std_row["k_factor"].as_f64(), Some(0.42));

    // 3. Unit filter: ?unit=saes returns only rows carrying a US gauge label,
    //    each also reporting thickness_in; ?unit=metric is the full catalog.
    let saes = get(&client, "/sheet-materials?unit=saes").await;
    assert_eq!(saes["unit"].as_str(), Some("saes"), "response unit tag");
    let saes_mats = saes["materials"].as_array().expect("saes materials array");
    // Count total thickness rows (not materials): every material carries at
    // least one gauge row, but the full catalog also has gauge-less decimals.
    let count_rows = |ms: &[serde_json::Value]| -> usize {
        ms.iter()
            .map(|m| m["thicknesses"].as_array().map(|a| a.len()).unwrap_or(0))
            .sum()
    };
    let saes_row_count = count_rows(saes_mats);
    let all_row_count = count_rows(mats);
    assert!(
        saes_row_count < all_row_count,
        "SAE view ({saes_row_count} rows) must be a strict subset of the full catalog ({all_row_count} rows)"
    );
    let saes_rows: Vec<serde_json::Value> = saes_mats
        .iter()
        .flat_map(|m| m["thicknesses"].as_array().cloned().unwrap_or_default())
        .collect();
    assert!(!saes_rows.is_empty(), "SAE view must not be empty");
    for t in &saes_rows {
        assert!(
            t["thickness_gauge"].as_str().is_some(),
            "SAE row without gauge label"
        );
        let mm = t["thickness_mm"].as_f64().expect("mm");
        let inch = t["thickness_in"].as_f64().expect("in");
        assert!(
            (mm / 25.4 - inch).abs() < 1e-6,
            "thickness_in inconsistent: {mm} mm vs {inch} in"
        );
    }
    // Compare against a fresh unfiltered listing (not the stale pre-POST `mats`):
    // the metric view must equal the full catalog, including gauge-less rows.
    let metric = get(&client, "/sheet-materials?unit=metric").await;
    assert_eq!(metric["unit"].as_str(), Some("metric"), "response unit tag");
    let full = get(&client, "/sheet-materials").await;
    assert_eq!(
        metric["materials"].as_array().unwrap().len(),
        full["materials"].as_array().unwrap().len(),
        "metric view is the full catalog"
    );

    // 4. It shows up in the listing, and the seed is stable (GET is idempotent).
    let again = get(&client, "/sheet-materials").await;
    let mats2 = again["materials"].as_array().expect("materials array");
    assert!(
        mats2.iter().any(|m| m["material"] == exotic),
        "org-added standard listed"
    );
    assert_eq!(
        mats2.len(),
        mats.len() + 1,
        "seed did not duplicate on second access"
    );

    eprintln!(
        "MFG e2e: PASS — sheet-metal standards seeded ({} materials) + org row created",
        mats.len()
    );
}
