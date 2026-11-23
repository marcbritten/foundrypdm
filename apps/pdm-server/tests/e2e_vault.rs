//! End-to-end vault test for Phase 1.
//!
//! Requires:
//!   PDM_E2E=1
//!   PDM_E2E_BASE   (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN
//!
//! Verifies the full acceptance flow from toDo §28:
//!   1. Two authenticated users
//!   2. One shared engineering project
//!   3. Upload of a `.solvecraft` design
//!   4. Creation of immutable file versions
//!   5. Exclusive checkout
//!   6. Successful check-in
//!   7. Retrieval of a previous version
//!   8. Audit history
//!   9. Verified file integrity after download

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

async fn create_user_and_token(
    client: &Client,
    email: &str,
    name: &str,
    role: &str,
) -> (String, String) {
    let r = client
        .post(format!("{}/users", base()))
        .bearer_auth(admin_token())
        .json(&json!({
            "email": email,
            "display_name": name,
            "role": role,
        }))
        .send()
        .await
        .expect("POST /users");
    let status = r.status();
    assert!(status.is_success(), "create user {email}: {status}");
    let body: serde_json::Value = r.json().await.expect("user JSON");
    let user_id = body["user"]["id"].as_str().unwrap().to_string();

    let r2 = client
        .post(format!("{}/users/{user_id}/tokens", base()))
        .bearer_auth(admin_token())
        .json(&json!({ "name": "e2e" }))
        .send()
        .await
        .expect("POST /users/{id}/tokens");
    let status2 = r2.status();
    assert!(status2.is_success(), "create token: {status2}");
    let tok_body: serde_json::Value = r2.json().await.expect("token JSON");
    let token = tok_body["token"].as_str().unwrap().to_string();
    (user_id, token)
}

#[tokio::test]
#[ignore]
async fn e2e_vault_full_flow() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");

    // 0. Health check
    let r = client
        .get(format!("{}/healthz", base()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "healthz failed");

    // 1. Two authenticated users
    let (_uid_a, token_a) =
        create_user_and_token(&client, "alice@test.dev", "Alice", "editor").await;
    let (_uid_b, token_b) = create_user_and_token(&client, "bob@test.dev", "Bob", "editor").await;

    // Verify /me for both
    for (label, tok) in [("A", &token_a), ("B", &token_b)] {
        let r = client
            .get(format!("{}/me", base()))
            .bearer_auth(tok)
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "GET /me user {label} failed");
    }

    // 2. One shared project
    let proj_name = format!("e2e-proj-{}", std::process::id());
    let r = client
        .post(format!("{}/projects", base()))
        .bearer_auth(admin_token())
        .json(&json!({ "name": proj_name, "description": "E2E test" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "POST /projects: {}", r.status());
    let proj: serde_json::Value = r.json().await.unwrap();
    let project_id = proj["project"]["id"].as_str().unwrap().to_string();

    // 3. Upload a .solvecraft design
    let design_v1 = br#"{"format":"solvecraft/5","name":"e2e","units":"mm","params":[],"features":[],"next_id":1,"components":[],"occurrences":[],"assembly":{},"parts":{},"configs":{},"favorites":[],"materials":{},"appearances":{},"body_components":{},"body_offsets":{},"browser_groups":[],"browser_order":{},"named_views":[]}"#;
    let sha_v1 = sha256_hex(design_v1);

    // Create an item
    let r = client
        .post(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&token_a)
        .json(&json!({ "description": "E2E test part", "classification": "BE" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "create item: {}", r.status());
    let item_resp: serde_json::Value = r.json().await.unwrap();
    let item_id = item_resp["item"]["id"].as_str().unwrap().to_string();
    let part_number = item_resp["item"]["part_number"]
        .as_str()
        .unwrap()
        .to_string();
    eprintln!("created item {part_number} ({item_id})");
    assert!(
        part_number.starts_with("BE-") && part_number != "BE-000000",
        "auto part number should be BE-000001 or later, got {part_number}"
    );

    // Upload blob
    let r = client
        .put(format!("{}/blobs", base()))
        .bearer_auth(&token_a)
        .header("Content-Type", "application/octet-stream")
        .body(design_v1.to_vec())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "PUT /blobs: {}", r.status());
    let blob_resp: serde_json::Value = r.json().await.unwrap();
    let uploaded_sha = blob_resp["blob"]["sha256"].as_str().unwrap().to_string();
    assert_eq!(uploaded_sha, sha_v1, "sha mismatch on upload");

    // 5. Exclusive checkout
    let r = client
        .post(format!("{}/items/{item_id}/checkout", base()))
        .bearer_auth(&token_a)
        .json(&json!({ "workstation": "e2e-a" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "checkout A: {}", r.status());
    let co_a: serde_json::Value = r.json().await.unwrap();
    assert!(co_a["lock"].is_object(), "no lock in checkout response");

    // User B tries to check out → 409
    let r = client
        .post(format!("{}/items/{item_id}/checkout", base()))
        .bearer_auth(&token_b)
        .json(&json!({ "workstation": "e2e-b" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        409,
        "checkout B should conflict, got {}",
        r.status()
    );

    // 6. Successful check-in (version 1)
    let r = client
        .post(format!("{}/items/{item_id}/checkin", base()))
        .bearer_auth(&token_a)
        .json(&json!({
            "sha256": sha_v1,
            "commit_message": "Initial import",
            "base_version": null,
            "filename": "e2e_part.solvecraft",
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "checkin v1: {}", r.status());
    let v1: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v1["version"]["seq"], 1, "first version should be seq 1");
    assert_eq!(v1["version"]["version"], 1, "first save is major version 1");
    assert_eq!(v1["version"]["revision"], 1, "first save is revision 1");
    let v1_id = v1["version"]["id"].as_str().unwrap().to_string();

    // 4. Immutable file versions — upload same blob again, should dedup
    let r = client
        .put(format!("{}/blobs", base()))
        .bearer_auth(&token_a)
        .header("Content-Type", "application/octet-stream")
        .body(design_v1.to_vec())
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "PUT /blobs (dedup): {}",
        r.status()
    );
    let blob2: serde_json::Value = r.json().await.unwrap();
    assert_eq!(
        blob2["blob"]["sha256"].as_str().unwrap(),
        sha_v1,
        "dedup should return same sha"
    );

    // Create version 2: a plain save is a new *revision* of the current version
    let design_v2 = br#"{"format":"solvecraft/5","name":"e2e_v2","units":"mm","params":[],"features":[],"next_id":1,"components":[],"occurrences":[],"assembly":{},"parts":{},"configs":{},"favorites":[],"materials":{},"appearances":{},"body_components":{},"body_offsets":{},"browser_groups":[],"browser_order":{},"named_views":[]}"#;
    let sha_v2 = sha256_hex(design_v2);
    let r = client
        .put(format!("{}/blobs", base()))
        .bearer_auth(&token_a)
        .body(design_v2.to_vec())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "upload v2 blob: {}", r.status());

    let r = client
        .post(format!("{}/items/{item_id}/checkout", base()))
        .bearer_auth(&token_a)
        .json(&json!({ "workstation": "e2e-a" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "re-checkout A: {}", r.status());

    let r = client
        .post(format!("{}/items/{item_id}/checkin", base()))
        .bearer_auth(&token_a)
        .json(&json!({
            "sha256": sha_v2,
            "commit_message": "Second revision",
            "base_version": 1,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "checkin v2: {}", r.status());
    let v2: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v2["version"]["seq"], 2, "second save should be seq 2");
    assert_eq!(
        v2["version"]["version"], 1,
        "plain save stays in major version 1"
    );
    assert_eq!(v2["version"]["revision"], 2, "plain save is revision 2");

    // Save as a new major version (V2 - R1)
    let design_v3 = br#"{"format":"solvecraft/5","name":"e2e_v3","units":"mm","params":[],"features":[],"next_id":1,"components":[],"occurrences":[],"assembly":{},"parts":{},"configs":{},"favorites":[],"materials":{},"appearances":{},"body_components":{},"body_offsets":{},"browser_groups":[],"browser_order":{},"named_views":[]}"#;
    let sha_v3 = sha256_hex(design_v3);
    let r = client
        .put(format!("{}/blobs", base()))
        .bearer_auth(&token_a)
        .body(design_v3.to_vec())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "upload v3 blob: {}", r.status());
    let _ = client
        .post(format!("{}/items/{item_id}/checkout", base()))
        .bearer_auth(&token_a)
        .json(&json!({ "workstation": "e2e-a" }))
        .send()
        .await
        .unwrap();
    let r = client
        .post(format!("{}/items/{item_id}/checkin", base()))
        .bearer_auth(&token_a)
        .json(&json!({
            "sha256": sha_v3,
            "commit_message": "New major version",
            "base_version": 2,
            "new_version": true,
        }))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "checkin new version: {}",
        r.status()
    );
    let v3: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v3["version"]["seq"], 3, "third save should be seq 3");
    assert_eq!(
        v3["version"]["version"], 2,
        "new_version save bumps the major version"
    );
    assert_eq!(
        v3["version"]["revision"], 1,
        "new_version save resets the revision"
    );

    // Stale check-in: base_version=1 but head is 3
    let _ = client
        .post(format!("{}/items/{item_id}/checkout", base()))
        .bearer_auth(&token_a)
        .json(&json!({ "workstation": "e2e-a" }))
        .send()
        .await
        .unwrap();
    let r = client
        .post(format!("{}/items/{item_id}/checkin", base()))
        .bearer_auth(&token_a)
        .json(&json!({
            "sha256": sha_v1,
            "commit_message": "stale",
            "base_version": 1,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        409,
        "stale checkin should 409, got {}",
        r.status()
    );
    let stale_body: serde_json::Value = r.json().await.unwrap();
    eprintln!(
        "stale checkin error: {}",
        stale_body["error"]["message"].as_str().unwrap_or("")
    );

    // Clean up the extra checkout
    let _ = client
        .delete(format!("{}/items/{item_id}/checkout", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();

    // 7. Retrieve previous version
    let r = client
        .get(format!("{}/versions/{v1_id}/blob", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "download v1: {}", r.status());
    let v1_bytes = r.bytes().await.unwrap();
    let v1_downloaded_sha = sha256_hex(&v1_bytes);
    assert_eq!(v1_downloaded_sha, sha_v1, "v1 blob integrity failed");
    eprintln!("v1 blob integrity verified: {v1_downloaded_sha}");

    // 8. Audit history
    let r = client
        .get(format!("{}/audit?item={item_id}", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "audit: {}", r.status());
    let audit: serde_json::Value = r.json().await.unwrap();
    let events = audit["events"].as_array().expect("audit events array");
    let actions: Vec<&str> = events.iter().filter_map(|e| e["action"].as_str()).collect();
    eprintln!("audit actions: {actions:?}");
    assert!(
        actions.contains(&"item.created"),
        "no item.created in audit"
    );
    assert!(
        actions.contains(&"item.checkout"),
        "no item.checkout in audit"
    );
    assert!(
        actions.contains(&"item.checkin"),
        "no item.checkin in audit"
    );

    // 9. Search finds the item
    let r = client
        .get(format!(
            "{}/search?q={}",
            base(),
            part_number.replace('-', "%2D")
        ))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "search: {}", r.status());
    let search: serde_json::Value = r.json().await.unwrap();
    let results = search["results"].as_array().expect("search results array");
    assert!(
        results
            .iter()
            .any(|x| x["id"].as_str() == Some(item_id.as_str())),
        "search did not find item {part_number}"
    );

    // 10. Item summaries carry the denormalized owner/modified/revision fields.
    let r = client
        .get(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "list items: {}", r.status());
    let list: serde_json::Value = r.json().await.unwrap();
    let arr = list["items"].as_array().expect("items array");
    let summary = arr
        .iter()
        .find(|x| x["id"].as_str() == Some(item_id.as_str()))
        .expect("item present in project listing");
    assert_eq!(summary["owner_name"].as_str(), Some("Alice"));
    assert_eq!(summary["modified_by"].as_str(), Some("Alice"));
    assert!(
        summary["modified_at"].is_string(),
        "modified_at should be set after check-in, got {}",
        summary["modified_at"]
    );
    assert_eq!(summary["head_version_seq"].as_i64(), Some(3));
    assert_eq!(
        summary["head_version"].as_i64(),
        Some(2),
        "head is major version 2"
    );
    assert_eq!(
        summary["head_revision"].as_i64(),
        Some(1),
        "head is revision 1 of V2"
    );
    assert!(
        summary["latest_revision"].is_null(),
        "no revision released yet, got {}",
        summary["latest_revision"]
    );

    // Release a revision; the summary must expose its label.
    let r = client
        .post(format!("{}/items/{item_id}/release", base()))
        .bearer_auth(&token_a)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "release: {}", r.status());
    let rel: serde_json::Value = r.json().await.unwrap();
    assert_eq!(rel["revision"]["label"].as_str(), Some("A"));

    let r = client
        .get(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let list2: serde_json::Value = r.json().await.unwrap();
    let summary2 = list2["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"].as_str() == Some(item_id.as_str()))
        .expect("item present after release");
    assert_eq!(summary2["latest_revision"].as_str(), Some("A"));

    // GET /items/{id} must carry the same summary fields.
    let r = client
        .get(format!("{}/items/{item_id}", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let detail: serde_json::Value = r.json().await.unwrap();
    assert_eq!(detail["item"]["owner_name"].as_str(), Some("Alice"));
    assert!(detail["item"]["modified_at"].is_string());
    assert_eq!(detail["item"]["latest_revision"].as_str(), Some("A"));

    eprintln!(
        "\nE2E VAULT PASSED — item {part_number}, 3 saves (V1-R1, V1-R2, V2-R1), audit verified, integrity OK, summary fields verified"
    );
}

fn project_present(body: &serde_json::Value, id: &str) -> bool {
    body["projects"]
        .as_array()
        .map(|a| a.iter().any(|p| p["id"].as_str() == Some(id)))
        .unwrap_or(false)
}

fn item_present(body: &serde_json::Value, id: &str) -> bool {
    body["items"]
        .as_array()
        .map(|a| a.iter().any(|p| p["id"].as_str() == Some(id)))
        .unwrap_or(false)
}

#[tokio::test]
#[ignore]
async fn e2e_archive_delete_flow() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let admin = admin_token();

    // Create a project.
    let proj_name = format!("e2e-arch-proj-{}", std::process::id());
    let r = client
        .post(format!("{}/projects", base()))
        .bearer_auth(&admin)
        .json(&json!({ "name": proj_name, "description": "archive test" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "create project: {}", r.status());
    let proj: serde_json::Value = r.json().await.unwrap();
    let project_id = proj["project"]["id"].as_str().unwrap().to_string();
    assert!(
        proj["project"]["archived_at"].is_null(),
        "new project should not be archived"
    );

    // Archive the project → hidden from the default GET /projects.
    let r = client
        .patch(format!("{}/projects/{project_id}/archive", base()))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "archive project: {}", r.status());

    let r = client
        .get(format!("{}/projects", base()))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(
        !project_present(&body, &project_id),
        "archived project should be hidden from GET /projects"
    );

    // …but present with ?include_archived=true, carrying a real archived_at.
    let r = client
        .get(format!("{}/projects?include_archived=true", base()))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(
        project_present(&body, &project_id),
        "archived project should show with include_archived"
    );
    let entry = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"].as_str() == Some(project_id.as_str()))
        .unwrap();
    assert!(
        entry["archived_at"].is_string(),
        "archived_at should be set, got {}",
        entry["archived_at"]
    );

    // Unarchive the project → visible again in the default listing.
    let r = client
        .patch(format!("{}/projects/{project_id}/unarchive", base()))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "unarchive project: {}", r.status());
    let r = client
        .get(format!("{}/projects", base()))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(
        project_present(&body, &project_id),
        "unarchived project should be visible again"
    );

    // Users for item operations.
    let (_uid_a, token_a) =
        create_user_and_token(&client, "archalice@test.dev", "ArchAlice", "editor").await;

    // Archive an item → hidden from the project's item list.
    let r = client
        .post(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&token_a)
        .json(&json!({ "description": "archive item", "classification": "BE" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "create item: {}", r.status());
    let item_resp: serde_json::Value = r.json().await.unwrap();
    let item_id = item_resp["item"]["id"].as_str().unwrap().to_string();

    let r = client
        .patch(format!("{}/items/{item_id}/archive", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "archive item: {}", r.status());

    let r = client
        .get(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(
        !item_present(&body, &item_id),
        "archived item should be hidden from project items"
    );

    let r = client
        .get(format!(
            "{}/projects/{project_id}/items?include_archived=true",
            base()
        ))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(
        item_present(&body, &item_id),
        "archived item should show with include_archived"
    );

    // Unarchive the item → visible in the default listing again.
    let r = client
        .patch(format!("{}/items/{item_id}/unarchive", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "unarchive item: {}", r.status());
    let r = client
        .get(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(
        item_present(&body, &item_id),
        "unarchived item should be visible again"
    );

    // Hard-delete an item that holds an active checkout lock → 409.
    let (_uid_b, token_b) =
        create_user_and_token(&client, "archbob@test.dev", "ArchBob", "editor").await;
    let r = client
        .post(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&token_b)
        .json(&json!({ "description": "locked item", "classification": "BE" }))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "create locked item: {}",
        r.status()
    );
    let item2: serde_json::Value = r.json().await.unwrap();
    let item2_id = item2["item"]["id"].as_str().unwrap().to_string();

    let r = client
        .post(format!("{}/items/{item2_id}/checkout", base()))
        .bearer_auth(&token_b)
        .json(&json!({ "workstation": "archlock" }))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "checkout locked item: {}",
        r.status()
    );

    let r = client
        .delete(format!("{}/items/{item2_id}", base()))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        409,
        "hard-delete a locked item should 409, got {}",
        r.status()
    );
    // Release the lock so the project can later be deleted.
    let _ = client
        .delete(format!("{}/items/{item2_id}/checkout", base()))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();

    // Delete the item → GET /items/{id} returns 404.
    let r = client
        .delete(format!("{}/items/{item_id}", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "delete item: {}", r.status());
    let r = client
        .get(format!("{}/items/{item_id}", base()))
        .bearer_auth(&token_a)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        404,
        "deleted item should be 404, got {}",
        r.status()
    );

    // Delete the project → its items cascade away.
    let r = client
        .delete(format!("{}/projects/{project_id}", base()))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "delete project: {}", r.status());
    let r = client
        .get(format!("{}/items/{item2_id}", base()))
        .bearer_auth(&token_b)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        404,
        "cascaded item should be 404 after project delete, got {}",
        r.status()
    );
    let r = client
        .get(format!("{}/projects", base()))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(
        !project_present(&body, &project_id),
        "deleted project should be gone"
    );

    eprintln!("\nE2E ARCHIVE/DELETE PASSED — project + item archive/unarchive/delete verified");
}

/// A 2 mm pin as STEP AP214 (generated, not from any commercial CAD product).
const PIN_STEP: &str = include_str!("fixtures/pin.step");

/// Run only against a disposable server/database with a real SolidCraft CLI.
#[tokio::test]
#[ignore]
async fn e2e_native_conversion_and_transparent_preview() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1")
        || std::env::var("PDM_E2E_CLI").ok().as_deref() != Some("1")
    {
        eprintln!("PDM_E2E=1 and PDM_E2E_CLI=1 required");
        return;
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    let (_, editor) = create_user_and_token(
        &client,
        &format!("convert-{key}@test.invalid"),
        "Conversion test",
        "editor",
    )
    .await;
    let (_, viewer) = create_user_and_token(
        &client,
        &format!("viewer-{key}@test.invalid"),
        "Viewer test",
        "viewer",
    )
    .await;
    let project: serde_json::Value = client
        .post(format!("{}/projects", base()))
        .bearer_auth(admin_token())
        .json(&json!({"name":format!("conversion-{key}")}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let project_id = project["project"]["id"].as_str().unwrap();
    let item: serde_json::Value = client
        .post(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&editor)
        .json(&json!({"description":"Native conversion fixture","classification":"BE"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let item_id = item["item"]["id"].as_str().unwrap();
    let uploaded: serde_json::Value = client
        .put(format!("{}/blobs", base()))
        .bearer_auth(&editor)
        .body(PIN_STEP.to_string())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let sha = uploaded["blob"]["sha256"].as_str().unwrap();
    client
        .post(format!("{}/items/{item_id}/checkout", base()))
        .bearer_auth(&editor)
        .json(&json!({"workstation":"conversion-e2e"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let checked: serde_json::Value = client
        .post(format!("{}/items/{item_id}/checkin", base()))
        .bearer_auth(&editor)
        .json(&json!({"sha256":sha,"filename":"pin.step","base_version":null}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let version_id = checked["version"]["id"].as_str().unwrap();
    let endpoint = format!("{}/versions/{version_id}/solidcraft", base());
    assert_eq!(
        client
            .post(&endpoint)
            .bearer_auth(&viewer)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let a = client.post(&endpoint).bearer_auth(&editor).send();
    let b = client.post(&endpoint).bearer_auth(&editor).send();
    let (a, b) = tokio::join!(a, b);
    let a: serde_json::Value = a.unwrap().error_for_status().unwrap().json().await.unwrap();
    let b: serde_json::Value = b.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(
        a["artifact"]["job_id"], b["artifact"]["job_id"],
        "concurrent requests must reuse one job"
    );
    let mut artifact = json!({});
    for _ in 0..180 {
        artifact = client
            .get(&endpoint)
            .bearer_auth(&editor)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            !matches!(artifact["job"]["status"].as_str(), Some("failed" | "dead")),
            "{artifact}"
        );
        if artifact["artifact"]["sha256"].is_string() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        artifact["artifact"]["sha256"].is_string(),
        "native artifact not produced: {artifact}"
    );
    assert_eq!(artifact["filename"], "pin-v1.sc");
    let native = client
        .get(format!("{endpoint}/blob"))
        .bearer_auth(&viewer)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(
        native
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("pin-v1.sc")
    );
    let native = native.bytes().await.unwrap();
    assert!(
        native.starts_with(b"PK\x03\x04"),
        "native artifact must be a design container"
    );
    assert_eq!(
        sha256_hex(&native),
        artifact["artifact"]["sha256"].as_str().unwrap()
    );
    let again: serde_json::Value = client
        .post(&endpoint)
        .bearer_auth(&editor)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(again["artifact"]["job_id"], artifact["artifact"]["job_id"]);
    let source = client
        .get(format!("{}/versions/{version_id}/blob", base()))
        .bearer_auth(&editor)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(source.as_ref(), PIN_STEP.as_bytes(), "source is immutable");
    // The bundled SolidCraft web editor opens versions through the unauthenticated
    // public design link, named exactly after the version's SolidCraft file.
    let public = client
        .get(format!("{}/public/design/{version_id}/pin-v1.sc", base()))
        .send()
        .await
        .unwrap();
    assert_eq!(public.status(), 200, "public design link needs no token");
    let public = public.bytes().await.unwrap();
    assert_eq!(
        sha256_hex(&public),
        artifact["artifact"]["sha256"].as_str().unwrap()
    );
    assert_eq!(
        client
            .get(format!("{}/public/design/{version_id}/pin.step", base()))
            .send()
            .await
            .unwrap()
            .status(),
        404,
        "with an artifact, the source name must not match"
    );
    assert_eq!(
        client
            .get(format!("{}/public/design/{version_id}/wrong.sc", base()))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let detail: serde_json::Value = client
        .get(format!("{}/items/{item_id}", base()))
        .bearer_auth(&editor)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["versions"].as_array().unwrap().len(), 1);
    assert_eq!(detail["head_version"]["id"], version_id);
    let render: serde_json::Value = client
        .post(format!("{}/versions/{version_id}/preview", base()))
        .bearer_auth(&editor)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let job_id = render["job"]["id"].as_str().unwrap();
    let mut finished = false;
    for _ in 0..180 {
        let status: serde_json::Value = client
            .get(format!("{}/jobs/{job_id}", base()))
            .bearer_auth(&editor)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            !matches!(status["job"]["status"].as_str(), Some("failed" | "dead")),
            "{status}"
        );
        if status["job"]["status"] == "done" {
            finished = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(finished, "render did not finish");
    let png = client
        .get(format!("{}/versions/{version_id}/thumbnail", base()))
        .bearer_auth(&editor)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let decoder = png::Decoder::new(std::io::Cursor::new(png));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (512, 512));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    let pixels = &pixels[..info.buffer_size()];
    for i in [0, 511, 512 * 511, 512 * 512 - 1] {
        assert_eq!(
            pixels[i * 4 + 3],
            0,
            "background corner must be transparent"
        );
    }
    assert!(
        pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
        "the actual part must remain visible"
    );
    eprintln!(
        "PASS: native artifact, idempotency, viewer permissions, immutable source, alpha PNG and regeneration"
    );
}

/// The worker's preview pipeline: a STEP file checked in through the portal flow
/// gets a server-rendered PNG thumbnail (`preview.render`), a restated STEP
/// (`cad.convert`) and measurements (`design.analyze`).
///
/// Gated on PDM_E2E_CLI — the running server must have been started with a
/// CAD CLI (e2e.sh exports it when one is on the PATH or PDM_E2E_CLI_BIN).
#[tokio::test]
#[ignore]
async fn e2e_preview_for_uploaded_cad_files() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    if std::env::var("PDM_E2E_CLI").ok().as_deref() != Some("1") {
        eprintln!(
            "PDM_E2E_CLI not set; skipping preview assertions (no CAD CLI in the e2e server)"
        );
        return;
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("client");

    // A project + item of its own (the vault test's items stay untouched).
    let proj_name = format!("e2e-preview-{}", std::process::id());
    let r = client
        .post(format!("{}/projects", base()))
        .bearer_auth(admin_token())
        .json(&json!({ "name": proj_name, "description": "E2E preview" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "POST /projects: {}", r.status());
    let proj: serde_json::Value = r.json().await.unwrap();
    let project_id = proj["project"]["id"].as_str().unwrap().to_string();

    let (_uid, token) = create_user_and_token(&client, "carol@test.dev", "Carol", "editor").await;

    let r = client
        .post(format!("{}/projects/{project_id}/items", base()))
        .bearer_auth(&token)
        .json(&json!({ "description": "Preview pipeline part", "classification": "BE" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "create item: {}", r.status());
    let item_resp: serde_json::Value = r.json().await.unwrap();
    let item_id = item_resp["item"]["id"].as_str().unwrap().to_string();

    // Upload + check in the STEP.
    let sha = sha256_hex(PIN_STEP.as_bytes());
    let r = client
        .put(format!("{}/blobs", base()))
        .bearer_auth(&token)
        .header("Content-Type", "application/octet-stream")
        .body(PIN_STEP.to_string())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "PUT /blobs: {}", r.status());
    let r = client
        .post(format!("{}/items/{item_id}/checkout", base()))
        .bearer_auth(&token)
        .json(&json!({ "workstation": "e2e-preview" }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "checkout: {}", r.status());
    let r = client
        .post(format!("{}/items/{item_id}/checkin", base()))
        .bearer_auth(&token)
        .json(&json!({
            "sha256": sha,
            "commit_message": "pin.step import",
            "base_version": null,
            "filename": "pin.step",
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "checkin: {}", r.status());
    let v1: serde_json::Value = r.json().await.unwrap();
    let version_id = v1["version"]["id"].as_str().unwrap().to_string();

    // Wait for the worker: the version gains a thumbnail, a restated STEP and
    // measurements (the in-process worker polls roughly every second).
    let mut head: serde_json::Value = json!({});
    for _ in 0..120 {
        let d: serde_json::Value = client
            .get(format!("{}/items/{item_id}", base()))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        head = d["head_version"].clone();
        if head["thumbnail_sha256"].is_string()
            && head["derived_step_sha256"].is_string()
            && head["measurements"].is_object()
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(
        head["thumbnail_sha256"].is_string(),
        "thumbnail not rendered: {head}"
    );
    assert!(
        head.get("thumbnail_error")
            .map(|e| e.is_null())
            .unwrap_or(true),
        "thumbnail_error set: {head}"
    );
    assert!(
        head["derived_step_sha256"].is_string(),
        "no restated STEP: {head}"
    );
    let measurements = head["measurements"].clone();
    assert!(
        measurements.get("skipped").and_then(|s| s.as_bool()) != Some(true),
        "measurements were skipped: {measurements}"
    );
    assert!(
        measurements.get("body_count").and_then(|c| c.as_u64()) == Some(1),
        "the pin is one body: {measurements}"
    );

    // The thumbnail is a real PNG, and the restated STEP is a real STEP.
    let r = client
        .get(format!("{}/versions/{version_id}/thumbnail", base()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "GET thumbnail: {}", r.status());
    assert!(
        r.headers()
            .get("content-type")
            .is_some_and(|c| c.to_str().is_ok_and(|ct| ct.contains("image/png"))),
        "thumbnail content-type: {:?}",
        r.headers().get("content-type")
    );
    let png = r.bytes().await.unwrap();
    assert!(png.len() > 100, "tiny thumbnail: {} bytes", png.len());
    assert_eq!(
        &png[..8],
        [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        "not a PNG"
    );

    let r = client
        .get(format!("{}/versions/{version_id}/derived-step", base()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "GET derived-step: {}", r.status());
    let step = r.text().await.unwrap();
    assert!(step.starts_with("ISO-10303-21"), "derived file is not STEP");

    eprintln!(
        "\nE2E PREVIEW PASSED — {item_id} v1: thumbnail {} bytes, derived STEP {} bytes, measurements OK",
        png.len(),
        step.len()
    );
}

#[tokio::test]
#[ignore]
async fn e2e_admin_edit_user() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let admin = admin_token();
    let tag = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );

    // Admin creates a user and mints a token for them.
    let (user_id, token) = create_user_and_token(
        &client,
        &format!("edit-{tag}@example.com"),
        "Edit Target",
        "editor",
    )
    .await;

    // A non-admin (editor) cannot edit users.
    let r = client
        .put(format!("{}/users/{user_id}", base()))
        .bearer_auth(&token)
        .json(&json!({ "role": "admin" }))
        .send()
        .await
        .expect("PUT as editor");
    assert_eq!(r.status(), 403, "editor should not edit users");

    // Admin edits the user: rename and demote to viewer.
    let r = client
        .put(format!("{}/users/{user_id}", base()))
        .bearer_auth(&admin)
        .json(&json!({ "display_name": "Edited Name", "role": "viewer" }))
        .send()
        .await
        .expect("PUT /users/{id}");
    let status = r.status();
    assert!(status.is_success(), "admin edit user: {status}");
    let body: serde_json::Value = r.json().await.expect("user JSON");
    assert_eq!(body["user"]["display_name"], "Edited Name");
    assert_eq!(body["user"]["role"], "viewer");
    assert_eq!(body["user"]["active"], true);

    // The edit is visible in the user list.
    let r = client
        .get(format!("{}/users", base()))
        .bearer_auth(&admin)
        .send()
        .await
        .expect("GET /users");
    let body: serde_json::Value = r.json().await.expect("users JSON");
    let entry = body["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"].as_str() == Some(user_id.as_str()))
        .expect("edited user in list");
    assert_eq!(entry["display_name"], "Edited Name");
    assert_eq!(entry["role"], "viewer");

    // Admin deactivates the user.
    let r = client
        .put(format!("{}/users/{user_id}", base()))
        .bearer_auth(&admin)
        .json(&json!({ "active": false }))
        .send()
        .await
        .expect("PUT deactivate");
    assert!(r.status().is_success(), "deactivate: {}", r.status());

    // An inactive user's token is rejected.
    let r = client
        .put(format!("{}/users/{user_id}", base()))
        .bearer_auth(&token)
        .json(&json!({ "role": "admin" }))
        .send()
        .await
        .expect("PUT as inactive");
    assert_eq!(r.status(), 403, "inactive user should be rejected");

    // An invalid role is rejected with a validation error.
    let r = client
        .put(format!("{}/users/{user_id}", base()))
        .bearer_auth(&admin)
        .json(&json!({ "role": "owner" }))
        .send()
        .await
        .expect("PUT bad role");
    assert_eq!(r.status(), 422, "invalid role should be a validation error");

    eprintln!("\nE2E ADMIN EDIT USER PASSED — {user_id}");
}
