//! End-to-end check of who sees what on `GET /system-info` and the CAD CLI, Dutchman and web
//! editor cards: what describes the whole server (infrastructure, uptime, server-wide counts,
//! host paths, the GPU survey) only for host admins; the build, CLI version, GPU policy and the
//! caller's own organization's counts for everyone.
//!
//! Requires:
//!   PDM_E2E=1
//!   PDM_E2E_BASE              (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN       a host admin (on a new server: its first admin)
//!   PDM_E2E_OTHER_ORG_TOKEN   an admin of a second organization (optional)
//!
//! Run against a disposable server, never one people use: it creates users.

use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;

fn base() -> String {
    std::env::var("PDM_E2E_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}

fn admin_token() -> String {
    std::env::var("PDM_E2E_ADMIN_TOKEN").expect("PDM_E2E_ADMIN_TOKEN must be set")
}

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
    (status, r.json().await.unwrap_or(Value::Null))
}

/// A new user of the host admin's organization with the given role, and a token for them.
async fn user_with_role(c: &Client, key: &str, role: &str) -> String {
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
        Some(json!({ "name": "e2e-system-info" })),
    )
    .await;
    assert!(s.is_success(), "token for {role}: {s}");
    v["token"].as_str().expect("token").to_string()
}

const SERVER_WIDE: [&str; 3] = ["postgres", "storage", "uptime_seconds"];

#[tokio::test]
#[ignore]
async fn e2e_system_info_server_wide_parts_for_host_admins_only() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let c = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let key = uuid::Uuid::new_v4().simple().to_string();

    // The host admin sees everything.
    let host = admin_token();
    let (s, v) = send(&c, Method::GET, "/system-info", &host, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["host_details"], json!(true), "{v}");
    for k in SERVER_WIDE {
        assert!(!v[k].is_null(), "host admin gets {k}: {v}");
    }
    assert!(v["counts"]["organizations"].is_i64() && v["counts"]["users"].is_i64());
    assert!(v["solidcraft_cli"]["path"].is_string());
    assert!(
        v["gpu"].get("survey").is_some(),
        "host admin gets the survey field"
    );
    for (path, field) in [
        ("/settings/cad-cli", "path"),
        ("/settings/dutchman", "path"),
    ] {
        let (s, v) = send(&c, Method::GET, path, &host, None).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert!(v[field].is_string(), "host admin gets {path} {field}: {v}");
    }

    // Everyone else: the build, CLI version, GPU policy and own-org counts; nothing server-wide.
    let mut others = vec![
        ("viewer", user_with_role(&c, &key, "viewer").await),
        ("org admin", user_with_role(&c, &key, "admin").await),
    ];
    match std::env::var("PDM_E2E_OTHER_ORG_TOKEN") {
        Ok(t) => others.push(("another organization's admin", t)),
        Err(_) => eprintln!("PDM_E2E_OTHER_ORG_TOKEN not set; cross-organization part skipped"),
    }
    for (who, token) in &others {
        let (s, v) = send(&c, Method::GET, "/system-info", token, None).await;
        assert_eq!(s, StatusCode::OK, "{who}");
        assert_eq!(v["host_details"], json!(false), "{who}: {v}");
        for k in SERVER_WIDE {
            assert!(v.get(k).is_none(), "{who} must not get {k}: {v}");
        }
        assert!(v["counts"].get("organizations").is_none(), "{who}: {v}");
        assert!(v["counts"].get("users").is_none(), "{who}: {v}");
        assert!(
            v["counts"]["projects"].is_i64(),
            "{who} keeps own-org counts"
        );
        assert!(v["solidcraft_cli"].get("path").is_none(), "{who}: {v}");
        assert!(v["solidcraft_cli"].get("present").is_some(), "{who}: {v}");
        assert!(v["gpu"].get("survey").is_none(), "{who}: {v}");
        assert!(v["gpu"]["policy"].is_string(), "{who} keeps the GPU policy");
        assert!(v["pdm"]["version"].is_string(), "{who} keeps the build");
        assert!(v["pdm"].get("releases_url").is_none(), "{who}: {v}");
        for (path, field) in [
            ("/settings/cad-cli", "path"),
            ("/settings/dutchman", "path"),
            ("/settings/solidcraft-web", "dir"),
        ] {
            let (s, v) = send(&c, Method::GET, path, token, None).await;
            assert_eq!(s, StatusCode::OK, "{who} {path}");
            assert!(v[field].is_null(), "{who} must not get {path} {field}: {v}");
            // No release lookup on GitHub for them.
            assert!(v["latest"].is_null(), "{who} {path} latest: {v}");
            if path == "/settings/dutchman" {
                let channels = v["channels"].as_array().expect("channels");
                assert_eq!(
                    channels.len(),
                    1,
                    "{who} gets only the current channel: {v}"
                );
                assert_eq!(channels[0]["value"], v["settings"]["channel"]);
            }
        }
    }
}
