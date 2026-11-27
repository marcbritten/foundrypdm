//! End-to-end checks of who may write what.
//!
//! Requires:
//!   PDM_E2E=1
//!   PDM_E2E_BASE   (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN
//!
//! Run against a disposable server, never one people use: it creates users,
//! projects, items, tools and manufacturing records.

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

#[tokio::test]
#[ignore]
async fn e2e_viewer_cannot_change_the_vault() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let c = client();
    let key = uuid::Uuid::new_v4().to_string();
    let (_, editor) = user_with_role(&c, &key, "editor").await;
    let (viewer_id, viewer) = user_with_role(&c, &key, "viewer").await;

    let (s, v) = send(
        &c,
        Method::POST,
        "/projects",
        &editor,
        Some(json!({ "name": format!("access-{key}") })),
    )
    .await;
    assert!(s.is_success(), "editor creates a project: {s}");
    let project = v["project"]["id"].as_str().unwrap().to_string();
    let (item, version) = item_with_version(&c, &editor, &project).await;
    let (s, v) = send(
        &c,
        Method::POST,
        "/tools",
        &editor,
        Some(json!({ "name": format!("tool-{key}") })),
    )
    .await;
    assert!(s.is_success(), "editor creates a tool: {s}");
    let tool = v["tool"]["id"].as_str().unwrap().to_string();

    // Every one of these is refused for a viewer and changes nothing.
    let refused: Vec<(Method, String, Option<Value>)> = vec![
        (Method::DELETE, format!("/items/{item}"), None),
        (Method::PATCH, format!("/items/{item}/archive"), None),
        (
            Method::POST,
            format!("/items/{item}/checkout"),
            Some(json!({ "workstation": "viewer" })),
        ),
        (
            Method::PUT,
            format!("/items/{item}/properties/finish"),
            Some(json!({ "value": "viewer" })),
        ),
        (
            Method::POST,
            format!("/items/{item}/release"),
            Some(json!({ "label": "V" })),
        ),
        (
            Method::POST,
            "/approvals".into(),
            Some(json!({ "item_id": item, "assignee_ids": [viewer_id] })),
        ),
        (
            Method::POST,
            format!("/projects/{project}/items"),
            Some(json!({ "description": "viewer" })),
        ),
        (
            Method::PATCH,
            format!("/projects/{project}"),
            Some(json!({ "name": format!("viewer-{key}") })),
        ),
        (Method::PATCH, format!("/projects/{project}/archive"), None),
        (Method::DELETE, format!("/projects/{project}"), None),
        (
            Method::POST,
            "/projects".into(),
            Some(json!({ "name": format!("viewer-{key}") })),
        ),
        (Method::DELETE, format!("/tools/{tool}"), None),
        (
            Method::POST,
            "/machines".into(),
            Some(json!({ "name": "viewer machine" })),
        ),
        (
            Method::POST,
            "/mfg-setups".into(),
            Some(json!({ "item_id": item, "name": "viewer" })),
        ),
        (
            Method::POST,
            "/sheet-metal".into(),
            Some(json!({ "item_id": item, "material": "viewer" })),
        ),
    ];
    for (method, path, body) in refused {
        let (s, _) = send(&c, method.clone(), &path, &viewer, body).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "viewer {method} {path}");
    }
    let (s, _) = upload(&c, &viewer, b"viewer upload".to_vec()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "viewer PUT /blobs");
    let r = c
        .put(format!("{}/versions/{version}/thumbnail", base()))
        .bearer_auth(&viewer)
        .body(b"\x89PNG\r\n\x1a\nviewer".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN, "viewer thumbnail");

    let (s, detail) = send(&c, Method::GET, &format!("/items/{item}"), &editor, None).await;
    assert_eq!(s, StatusCode::OK, "the item is still there");
    assert!(detail["item"]["archived_at"].is_null(), "not archived");
    assert!(detail["lock"].is_null(), "not checked out");
    let (_, revs) = send(
        &c,
        Method::GET,
        &format!("/items/{item}/revisions"),
        &editor,
        None,
    )
    .await;
    assert_eq!(revs["revisions"].as_array().map(Vec::len), Some(0));
    let (s, _) = send(&c, Method::GET, &format!("/tools/{tool}"), &editor, None).await;
    assert_eq!(s, StatusCode::OK, "the tool is still there");

    // A viewer still reads, comments and opens a version.
    let (s, _) = send(
        &c,
        Method::POST,
        &format!("/items/{item}/comments"),
        &viewer,
        Some(json!({ "body": "looks good" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "viewer comments");
    let (s, _) = send(
        &c,
        Method::POST,
        &format!("/versions/{version}/design-link"),
        &viewer,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "viewer opens a design link");

    // An editor keeps every one of these.
    for (method, path, body) in [
        (
            Method::PUT,
            format!("/items/{item}/properties/finish"),
            Some(json!({ "value": "oak" })),
        ),
        (Method::PATCH, format!("/items/{item}/archive"), None),
        (Method::PATCH, format!("/items/{item}/unarchive"), None),
        (
            Method::POST,
            format!("/items/{item}/release"),
            Some(json!({ "label": "A" })),
        ),
        (Method::DELETE, format!("/tools/{tool}"), None),
        (Method::DELETE, format!("/items/{item}"), None),
        (Method::DELETE, format!("/projects/{project}"), None),
    ] {
        let (s, _) = send(&c, method.clone(), &path, &editor, body).await;
        assert!(s.is_success(), "editor {method} {path}: {s}");
    }
    eprintln!("PASS: a viewer is refused every vault and library write; an editor keeps them");
}
