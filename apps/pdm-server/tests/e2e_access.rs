//! End-to-end checks of who may write what.
//!
//! Requires:
//!   PDM_E2E=1
//!   PDM_E2E_BASE   (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN
//!   PDM_E2E_OTHER_ORG_TOKEN  (an editor of a second organization; the cross-organization
//!                             test skips without it)
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

#[tokio::test]
#[ignore]
async fn e2e_other_org_cannot_write_to_this_orgs_records() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let Ok(other) = std::env::var("PDM_E2E_OTHER_ORG_TOKEN") else {
        eprintln!("PDM_E2E_OTHER_ORG_TOKEN not set; skipping");
        return;
    };
    let c = client();
    let key = uuid::Uuid::new_v4().to_string();
    let (_, editor) = user_with_role(&c, &key, "editor").await;
    let (s, v) = send(
        &c,
        Method::POST,
        "/projects",
        &editor,
        Some(json!({ "name": format!("owner-{key}") })),
    )
    .await;
    assert!(s.is_success(), "owner project: {s}");
    let project = v["project"]["id"].as_str().unwrap().to_string();
    let (item, version) = item_with_version(&c, &editor, &project).await;
    let (_, v) = send(
        &c,
        Method::POST,
        "/machines",
        &editor,
        Some(json!({ "name": format!("m-{key}"), "kind": "printer" })),
    )
    .await;
    let machine = v["machine"]["id"].as_str().unwrap().to_string();
    let (_, v) = send(
        &c,
        Method::POST,
        "/tools",
        &editor,
        Some(json!({ "name": format!("t-{key}") })),
    )
    .await;
    let tool = v["tool"]["id"].as_str().unwrap().to_string();
    let owner_png = b"\x89PNG\r\n\x1a\nowner".to_vec();
    let r = c
        .put(format!("{}/versions/{version}/thumbnail", base()))
        .bearer_auth(&editor)
        .body(owner_png.clone())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "owner thumbnail: {}", r.status());

    // The other organization's own records to point from.
    let (s, v) = send(
        &c,
        Method::POST,
        "/projects",
        &other,
        Some(json!({ "name": format!("other-{key}") })),
    )
    .await;
    assert!(s.is_success(), "other org project: {s}");
    let other_project = v["project"]["id"].as_str().unwrap().to_string();
    let (other_item, _) = item_with_version(&c, &other, &other_project).await;
    let (_, v) = send(
        &c,
        Method::POST,
        "/mfg-setups",
        &other,
        Some(json!({ "item_id": other_item, "name": "own" })),
    )
    .await;
    let other_setup = v["mfg_setup"]["id"].as_str().unwrap().to_string();

    // Each write naming this organization's ids answers 404, as for an id that does not exist.
    for (method, path, body) in [
        (
            Method::PUT,
            format!("/items/{item}/properties/onshape.document_id"),
            json!({ "value": "other" }),
        ),
        (
            Method::POST,
            format!("/items/{item}/release"),
            json!({ "label": "X" }),
        ),
        (
            Method::POST,
            format!("/items/{item}/comments"),
            json!({ "body": "other" }),
        ),
        (
            Method::POST,
            "/sheet-metal".into(),
            json!({ "item_id": item, "material": "other" }),
        ),
        (
            Method::POST,
            "/mfg-setups".into(),
            json!({ "item_id": item, "name": "other" }),
        ),
        (
            Method::POST,
            "/cam-projects".into(),
            json!({ "mfg_setup_id": other_setup, "name": "x", "tool_ids": [tool] }),
        ),
        (
            Method::POST,
            "/nc-programs".into(),
            json!({ "item_id": other_item, "machine_id": machine }),
        ),
        (
            Method::POST,
            "/print-jobs".into(),
            json!({ "item_id": other_item, "file_version_id": version }),
        ),
        (
            Method::POST,
            "/inspections".into(),
            json!({ "item_id": item, "passed": true }),
        ),
    ] {
        let (s, _) = send(&c, method.clone(), &path, &other, Some(body)).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "other org {method} {path}");
    }
    let r = c
        .put(format!("{}/versions/{version}/thumbnail", base()))
        .bearer_auth(&other)
        .body(b"\x89PNG\r\n\x1a\nother".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND, "other org thumbnail");

    // Nothing changed on the owner's side.
    let (_, props) = send(
        &c,
        Method::GET,
        &format!("/items/{item}/properties"),
        &editor,
        None,
    )
    .await;
    assert_eq!(props["properties"].as_array().map(Vec::len), Some(0));
    let (_, revs) = send(
        &c,
        Method::GET,
        &format!("/items/{item}/revisions"),
        &editor,
        None,
    )
    .await;
    assert_eq!(revs["revisions"].as_array().map(Vec::len), Some(0));
    let (_, comments) = send(
        &c,
        Method::GET,
        &format!("/items/{item}/comments"),
        &editor,
        None,
    )
    .await;
    assert_eq!(comments["comments"].as_array().map(Vec::len), Some(0));
    let (_, detail) = send(&c, Method::GET, &format!("/items/{item}"), &editor, None).await;
    assert_ne!(
        detail["item"]["sheet_metal"],
        json!(true),
        "no sheet-metal spec"
    );
    let r = c
        .get(format!("{}/versions/{version}/thumbnail", base()))
        .bearer_auth(&editor)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.bytes().await.unwrap().to_vec(),
        owner_png,
        "the owner's thumbnail is unchanged"
    );

    // The same writes on its own records still work for the other organization.
    for (path, body) in [
        (
            format!("/items/{other_item}/comments"),
            json!({ "body": "own" }),
        ),
        (
            "/sheet-metal".to_string(),
            json!({ "item_id": other_item, "material": "own" }),
        ),
        (
            "/nc-programs".to_string(),
            json!({ "item_id": other_item, "mfg_setup_id": other_setup }),
        ),
    ] {
        let (s, _) = send(&c, Method::POST, &path, &other, Some(body)).await;
        assert!(s.is_success(), "other org on its own records: {path}: {s}");
    }
    eprintln!("PASS: another organization's ids are refused on every write checked");
}

/// The id of the one record a create route returns (`{"tool": {"id": …}}` and the like).
async fn create(c: &Client, token: &str, path: &str, body: Value, kind: &str) -> String {
    let (s, v) = send(c, Method::POST, path, token, Some(body)).await;
    assert!(s.is_success(), "create {kind}: {s} {v}");
    v[kind]["id"].as_str().expect("id").to_string()
}

#[tokio::test]
#[ignore]
async fn e2e_operator_records_floor_work_and_nothing_else() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let c = client();
    let key = uuid::Uuid::new_v4().to_string();
    let (_, editor) = user_with_role(&c, &key, "editor").await;
    let (_, viewer) = user_with_role(&c, &key, "viewer").await;
    let (operator_id, operator) = user_with_role(&c, &key, "operator").await;

    let project = create(
        &c,
        &editor,
        "/projects",
        json!({ "name": format!("operator-{key}") }),
        "project",
    )
    .await;
    let item = create(
        &c,
        &editor,
        &format!("/projects/{project}/items"),
        json!({ "description": "operator test" }),
        "item",
    )
    .await;
    let tool = create(
        &c,
        &editor,
        "/tools",
        json!({ "name": format!("tool-{key}") }),
        "tool",
    )
    .await;
    let nc = create(
        &c,
        &editor,
        "/nc-programs",
        json!({ "item_id": item }),
        "nc_program",
    )
    .await;
    let mut jobs = Vec::new();
    for _ in 0..3 {
        let job = json!({ "nc_program_id": nc });
        jobs.push(create(&c, &editor, "/production-jobs", job, "production_job").await);
    }
    let mut prints = Vec::new();
    for _ in 0..2 {
        let print = json!({ "item_id": item });
        prints.push(create(&c, &editor, "/print-jobs", print, "print_job").await);
    }
    let floor = |job: &str, print: &str| {
        vec![
            (
                format!("/production-jobs/{job}/status"),
                json!({ "status": "in_progress" }),
            ),
            (
                format!("/print-jobs/{print}/status"),
                json!({ "status": "printing" }),
            ),
            (
                "/inspections".to_string(),
                json!({ "production_job_id": job, "passed": true }),
            ),
        ]
    };

    // A viewer still may not record floor work.
    for (path, body) in floor(&jobs[0], &prints[0]) {
        let (s, _) = send(&c, Method::POST, &path, &viewer, Some(body)).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "viewer POST {path}");
    }
    // An operator may.
    for (path, body) in floor(&jobs[1], &prints[1]) {
        let (s, _) = send(&c, Method::POST, &path, &operator, Some(body)).await;
        assert_eq!(s, StatusCode::OK, "operator POST {path}");
    }
    let (s, _) = send(
        &c,
        Method::POST,
        &format!("/production-jobs/{}/status", jobs[1]),
        &operator,
        Some(json!({ "status": "done" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "operator marks the job done");

    // Everything else stays editor or admin.
    let refused: Vec<(Method, String, Option<Value>)> = vec![
        (Method::POST, format!("/nc-programs/{nc}/release"), None),
        (
            Method::POST,
            "/tools".into(),
            Some(json!({ "name": format!("operator-{key}") })),
        ),
        (Method::DELETE, format!("/tools/{tool}"), None),
        (
            Method::POST,
            "/machines".into(),
            Some(json!({ "name": "operator machine" })),
        ),
        (
            Method::POST,
            "/production-jobs".into(),
            Some(json!({ "nc_program_id": nc })),
        ),
        (
            Method::POST,
            "/print-jobs".into(),
            Some(json!({ "item_id": item })),
        ),
        (
            Method::POST,
            "/nc-programs".into(),
            Some(json!({ "item_id": item })),
        ),
        (
            Method::PUT,
            format!("/items/{item}/properties/finish"),
            Some(json!({ "value": "operator" })),
        ),
        (
            Method::POST,
            format!("/items/{item}/checkout"),
            Some(json!({ "workstation": "operator" })),
        ),
        (Method::PATCH, format!("/items/{item}/archive"), None),
        (
            Method::POST,
            format!("/projects/{project}/items"),
            Some(json!({ "description": "operator" })),
        ),
        (
            Method::POST,
            "/users".into(),
            Some(json!({ "email": format!("made-{key}@test.invalid"), "role": "admin" })),
        ),
        (
            Method::PUT,
            format!("/users/{operator_id}"),
            Some(json!({ "role": "admin" })),
        ),
        (
            Method::POST,
            format!("/users/{operator_id}/tokens"),
            Some(json!({ "name": "operator" })),
        ),
        (
            Method::POST,
            format!("/users/{operator_id}/password"),
            Some(json!({ "password": "operator-pass" })),
        ),
        (
            Method::PUT,
            "/settings/mail".into(),
            Some(json!({ "host": "mail.invalid" })),
        ),
    ];
    for (method, path, body) in refused {
        let (s, _) = send(&c, method.clone(), &path, &operator, body).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "operator {method} {path}");
    }

    let (_, v) = send(&c, Method::GET, "/production-jobs", &editor, None).await;
    let status = |id: &str| {
        v["production_jobs"]
            .as_array()
            .and_then(|a| a.iter().find(|j| j["id"] == id))
            .map(|j| j["status"].as_str().unwrap_or("").to_string())
    };
    assert_eq!(status(&jobs[0]).as_deref(), Some("pending"), "viewer's job");
    assert_eq!(status(&jobs[1]).as_deref(), Some("done"), "operator's job");
    let (s, _) = send(&c, Method::GET, &format!("/tools/{tool}"), &editor, None).await;
    assert_eq!(s, StatusCode::OK, "the tool is still there");

    // An editor keeps floor work and NC release.
    for (path, body) in floor(&jobs[2], &prints[0])
        .into_iter()
        .chain([(format!("/nc-programs/{nc}/release"), Value::Null)])
    {
        let body = (!body.is_null()).then_some(body);
        let (s, _) = send(&c, Method::POST, &path, &editor, body).await;
        assert!(s.is_success(), "editor POST {path}: {s}");
    }
    eprintln!("PASS: an operator records floor work and is refused everything else");
}
