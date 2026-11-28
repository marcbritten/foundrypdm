//! End-to-end check of the release-approval setting.
//!
//! Requires:
//!   PDM_E2E=1
//!   PDM_E2E_BASE   (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN
//!
//! Run against a disposable server, never one people use: it turns the organization's
//! release approval on and off and creates users, a project and items.

use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;

fn base() -> String {
    std::env::var("PDM_E2E_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}

fn admin_token() -> String {
    std::env::var("PDM_E2E_ADMIN_TOKEN").expect("PDM_E2E_ADMIN_TOKEN must be set")
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client")
}

/// Send a request; returns the status and the JSON body (`Null` when there is none).
async fn send(
    c: &Client,
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = c
        .request(method, format!("{}{path}", base()))
        .bearer_auth(token);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let r = req.send().await.expect("request");
    let status = r.status();
    let body = r.json().await.unwrap_or(Value::Null);
    (status, body)
}

async fn upload(c: &Client, token: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let r = c
        .put(format!("{}/blobs", base()))
        .bearer_auth(token)
        .header("Content-Type", "application/octet-stream")
        .body(bytes)
        .send()
        .await
        .expect("PUT /blobs");
    let status = r.status();
    (status, r.json().await.unwrap_or(Value::Null))
}

/// A new user of the admin's organization with the given role, and a token for them.
async fn user_with_role(c: &Client, key: &str, role: &str) -> (String, String) {
    let (s, v) = send(
        c,
        Method::POST,
        "/users",
        &admin_token(),
        Some(json!({ "email": format!("{role}-{key}@test.invalid"), "role": role })),
    )
    .await;
    assert!(s.is_success(), "create {role}: {s}");
    let id = v["user"]["id"].as_str().expect("user id").to_string();
    let (s, v) = send(
        c,
        Method::POST,
        &format!("/users/{id}/tokens"),
        &admin_token(),
        Some(json!({ "name": "e2e-access" })),
    )
    .await;
    assert!(s.is_success(), "token for {role}: {s}");
    (id, v["token"].as_str().expect("token").to_string())
}

/// A new item in `project` with one checked-in version; returns (item id, version id).
async fn item_with_version(c: &Client, token: &str, project: &str) -> (String, String) {
    let (s, v) = send(
        c,
        Method::POST,
        &format!("/projects/{project}/items"),
        token,
        Some(json!({ "description": "access test" })),
    )
    .await;
    assert!(s.is_success(), "create item: {s}");
    let item = v["item"]["id"].as_str().unwrap().to_string();
    let (s, _) = send(
        c,
        Method::POST,
        &format!("/items/{item}/checkout"),
        token,
        Some(json!({ "workstation": "e2e" })),
    )
    .await;
    assert!(s.is_success(), "checkout: {s}");
    let (s, blob) = upload(c, token, format!("access test {item}").into_bytes()).await;
    assert!(s.is_success(), "upload: {s}");
    let (s, _) = send(
        c,
        Method::POST,
        &format!("/items/{item}/checkin"),
        token,
        Some(json!({ "sha256": blob["blob"]["sha256"], "filename": "part.step" })),
    )
    .await;
    assert!(s.is_success(), "checkin: {s}");
    let _ = send(
        c,
        Method::DELETE,
        &format!("/items/{item}/checkout"),
        token,
        None,
    )
    .await;
    let (_, detail) = send(c, Method::GET, &format!("/items/{item}"), token, None).await;
    let version = detail["versions"][0]["id"].as_str().unwrap().to_string();
    (item, version)
}

async fn gate(c: &Client, token: &str, on: bool) -> (StatusCode, Value) {
    send(
        c,
        Method::PUT,
        "/settings/release-approval",
        token,
        Some(json!({ "require_approval_on_release": on })),
    )
    .await
}

#[tokio::test]
#[ignore]
async fn e2e_admins_set_release_approval_and_releases_follow_it() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let c = client();
    let admin = admin_token();
    let key = uuid::Uuid::new_v4().to_string();
    let (_, editor) = user_with_role(&c, &key, "editor").await;
    let (_, viewer) = user_with_role(&c, &key, "viewer").await;

    // Anyone in the organization can read it; only an admin can change it.
    let (s, v) = send(&c, Method::GET, "/settings/release-approval", &viewer, None).await;
    assert_eq!(s, StatusCode::OK, "viewer reads the setting");
    assert!(v["require_approval_on_release"].is_boolean());
    // Put the setting back as it was at the end (this test changes it for the whole organization).
    let started_on = v["require_approval_on_release"] == json!(true);
    for token in [&editor, &viewer] {
        let (s, _) = gate(&c, token, true).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "a non-admin changed the setting");
    }

    let (s, v) = send(
        &c,
        Method::POST,
        "/projects",
        &editor,
        Some(json!({ "name": format!("approval-{key}") })),
    )
    .await;
    assert!(s.is_success(), "editor creates a project: {s}");
    let project = v["project"]["id"].as_str().unwrap().to_string();

    // On: a release waits for approval and the item is in review.
    let (s, v) = gate(&c, &admin, true).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["require_approval_on_release"], json!(true));
    let (item, _) = item_with_version(&c, &editor, &project).await;
    let (s, v) = send(
        &c,
        Method::POST,
        &format!("/items/{item}/release"),
        &editor,
        Some(json!({})),
    )
    .await;
    assert!(s.is_success(), "gated release: {s} {v}");
    assert_eq!(v["status"], json!("pending_approval"), "{v}");
    let (_, d) = send(&c, Method::GET, &format!("/items/{item}"), &editor, None).await;
    assert_eq!(d["item"]["lifecycle_state"], json!("in_review"), "{d}");

    // Off: a release makes the revision at once.
    let (s, v) = gate(&c, &admin, false).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["require_approval_on_release"], json!(false));
    let (item, _) = item_with_version(&c, &editor, &project).await;
    let (s, v) = send(
        &c,
        Method::POST,
        &format!("/items/{item}/release"),
        &editor,
        Some(json!({})),
    )
    .await;
    assert!(s.is_success(), "release: {s} {v}");
    assert!(v["revision"].is_object(), "{v}");

    let (s, _) = gate(&c, &admin, started_on).await;
    assert_eq!(s, StatusCode::OK, "restore the starting value");
}
