//! End-to-end checks of the host admin role: what the whole server shares (CAD CLI and web
//! editor builds, the Dutchman release channel, the preview GPU policy) is changed only by a
//! host admin, and an org admin cannot take over a host admin's account.
//!
//! Requires:
//!   PDM_E2E=1
//!   PDM_E2E_BASE              (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN       a host admin (on a new server: its first admin)
//!   PDM_E2E_OTHER_ORG_TOKEN   an admin of a second organization (optional; that part
//!                             skips without it)
//!
//! Run against a disposable server, never one people use: it creates users and grants and
//! revokes host admin. It switches the server-wide GPU policy and Dutchman channel and puts
//! them back.

use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::time::Duration;

fn base() -> String {
    std::env::var("PDM_E2E_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}

fn admin_token() -> String {
    std::env::var("PDM_E2E_ADMIN_TOKEN").expect("PDM_E2E_ADMIN_TOKEN must be set")
}

fn enabled() -> bool {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return false;
    }
    true
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

/// A new user of the host admin's organization with the given role, and a token for them.
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
        Some(json!({ "name": "e2e-host-admin" })),
    )
    .await;
    assert!(s.is_success(), "token for {role}: {s}");
    (id, v["token"].as_str().expect("token").to_string())
}

async fn me(c: &Client, token: &str) -> Value {
    let (s, v) = send(c, Method::GET, "/me", token, None).await;
    assert_eq!(s, StatusCode::OK, "GET /me");
    v
}

#[tokio::test]
#[ignore]
async fn e2e_org_admin_cannot_change_the_server() {
    if !enabled() {
        return;
    }
    let c = client();
    let host = admin_token();
    let host_me = me(&c, &host).await;
    assert_eq!(
        host_me["host_admin"],
        json!(true),
        "the admin token is a host admin"
    );
    let host_id = host_me["user"]["id"].as_str().expect("id").to_string();
    let key = uuid::Uuid::new_v4().simple().to_string();
    let (_, org_admin) = user_with_role(&c, &key, "admin").await;
    assert_eq!(me(&c, &org_admin).await["host_admin"], json!(false));

    let (_, gp) = send(&c, Method::GET, "/settings/gpu-policy", &org_admin, None).await;
    let gpu = gp["gpu_policy"].as_str().expect("gpu_policy").to_string();
    let (_, dm) = send(&c, Method::GET, "/settings/dutchman", &org_admin, None).await;
    let channel = dm["settings"]["channel"]
        .as_str()
        .expect("channel")
        .to_string();
    let other_channel = if channel == "nightly" {
        "latest"
    } else {
        "nightly"
    };

    // Server-wide: refused for an org admin, with a message naming who can.
    for (method, path, body) in [
        (
            Method::PUT,
            "/settings/gpu-policy",
            json!({ "gpu_policy": gpu }),
        ),
        (
            Method::PUT,
            "/settings/dutchman",
            json!({ "channel": other_channel }),
        ),
        (Method::POST, "/settings/dutchman/update", json!({})),
        (Method::POST, "/settings/cad-cli/update", json!({})),
        (Method::POST, "/settings/solidcraft-web/update", json!({})),
    ] {
        let (s, v) = send(&c, method.clone(), path, &org_admin, Some(body)).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {path}: {v}");
        assert!(
            v.to_string().contains("host admin"),
            "{path} names who can: {v}"
        );
    }
    let (_, dm) = send(&c, Method::GET, "/settings/dutchman", &org_admin, None).await;
    assert_eq!(
        dm["settings"]["channel"],
        json!(channel),
        "channel unchanged"
    );

    // The org's own conversion options stay with org admins, also when the client re-sends
    // the unchanged channel.
    let preset = dm["settings"]["preset"]
        .as_str()
        .expect("preset")
        .to_string();
    let (s, _) = send(
        &c,
        Method::PUT,
        "/settings/dutchman",
        &org_admin,
        Some(json!({ "channel": channel, "preset": preset })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "org admin saves conversion settings");

    // A host admin's account is out of an org admin's reach.
    for (method, path, body) in [
        (
            Method::POST,
            format!("/users/{host_id}/password"),
            json!({ "password": format!("pw-{key}") }),
        ),
        (
            Method::POST,
            format!("/users/{host_id}/tokens"),
            json!({ "name": "takeover" }),
        ),
        (
            Method::PUT,
            format!("/users/{host_id}"),
            json!({ "display_name": "renamed" }),
        ),
        (
            Method::PUT,
            format!("/users/{host_id}/host-admin"),
            json!({ "host_admin": false }),
        ),
        (Method::GET, "/host-admins".to_string(), Value::Null),
    ] {
        let body = (!body.is_null()).then_some(body);
        let (s, v) = send(&c, method.clone(), &path, &org_admin, body).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {path}: {v}");
    }

    // The host admin still can. It switches the GPU policy and the channel to other values,
    // another organization (and the host admin's own) reads the new ones, then both go back.
    let other_gpu = if gpu == "cpu" { "auto" } else { "cpu" };
    let set_both = |g: String, ch: String| {
        let c = c.clone();
        let host = host.clone();
        async move {
            let (s, v) = send(
                &c,
                Method::PUT,
                "/settings/gpu-policy",
                &host,
                Some(json!({ "gpu_policy": g })),
            )
            .await;
            assert_eq!(s, StatusCode::OK, "host admin sets the GPU policy: {v}");
            let (s, v) = send(
                &c,
                Method::PUT,
                "/settings/dutchman",
                &host,
                Some(json!({ "channel": ch })),
            )
            .await;
            assert_eq!(s, StatusCode::OK, "host admin sets the channel: {v}");
        }
    };
    set_both(other_gpu.to_string(), other_channel.to_string()).await;
    let mut readers = vec![("own organization", org_admin.clone())];
    match std::env::var("PDM_E2E_OTHER_ORG_TOKEN") {
        Ok(other) => {
            // Another organization's admin is refused too.
            let (s, _) = send(
                &c,
                Method::PUT,
                "/settings/gpu-policy",
                &other,
                Some(json!({ "gpu_policy": gpu })),
            )
            .await;
            assert_eq!(
                s,
                StatusCode::FORBIDDEN,
                "other org admin sets the GPU policy"
            );
            readers.push(("another organization", other));
        }
        Err(_) => eprintln!("PDM_E2E_OTHER_ORG_TOKEN not set; cross-organization part skipped"),
    }
    for (who, token) in &readers {
        let (_, v) = send(&c, Method::GET, "/settings/gpu-policy", token, None).await;
        assert_eq!(
            v["gpu_policy"],
            json!(other_gpu),
            "{who} reads the new policy"
        );
        let (_, v) = send(&c, Method::GET, "/settings/dutchman", token, None).await;
        assert_eq!(
            v["settings"]["channel"],
            json!(other_channel),
            "{who} reads the new channel"
        );
    }
    set_both(gpu.clone(), channel.clone()).await;
    for path in [
        "/settings/dutchman/update",
        "/settings/cad-cli/update",
        "/settings/solidcraft-web/update",
    ] {
        // Only the check is asserted: the download itself may fail on a test stack.
        let (s, v) = send(&c, Method::POST, path, &host, Some(json!({}))).await;
        assert_ne!(s, StatusCode::FORBIDDEN, "host admin {path}: {v}");
    }
}

#[tokio::test]
#[ignore]
async fn e2e_host_admin_grant_and_revoke() {
    if !enabled() {
        return;
    }
    let c = client();
    let host = admin_token();
    let host_id = me(&c, &host).await["user"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let key = uuid::Uuid::new_v4().simple().to_string();
    let (admin_id, admin) = user_with_role(&c, &key, "admin").await;
    let (editor_id, _) = user_with_role(&c, &key, "editor").await;
    let (_, gp) = send(&c, Method::GET, "/settings/gpu-policy", &admin, None).await;
    let gpu = gp["gpu_policy"].clone();
    let set_gpu = |token: String| {
        let c = c.clone();
        let gpu = gpu.clone();
        async move {
            send(
                &c,
                Method::PUT,
                "/settings/gpu-policy",
                &token,
                Some(json!({ "gpu_policy": gpu })),
            )
            .await
            .0
        }
    };

    let (s, v) = send(
        &c,
        Method::PUT,
        &format!("/users/{editor_id}/host-admin"),
        &host,
        Some(json!({ "host_admin": true })),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "only an admin can be granted it: {v}"
    );

    let (s, v) = send(
        &c,
        Method::PUT,
        &format!("/users/{admin_id}/host-admin"),
        &host,
        Some(json!({ "host_admin": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "grant: {v}");
    assert_eq!(me(&c, &admin).await["host_admin"], json!(true));
    assert_eq!(set_gpu(admin.clone()).await, StatusCode::OK);
    let (_, audit) = send(&c, Method::GET, "/audit?limit=50", &host, None).await;
    let granted = audit["events"]
        .as_array()
        .expect("events")
        .iter()
        .any(|e| e["action"] == "user.host_admin_granted" && e["object_id"] == json!(admin_id));
    assert!(granted, "grant is in the audit log");
    let (s, v) = send(&c, Method::GET, "/host-admins", &admin, None).await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&Value> = v["host_admins"]
        .as_array()
        .expect("list")
        .iter()
        .map(|u| &u["id"])
        .collect();
    assert!(ids.contains(&&json!(admin_id)) && ids.contains(&&json!(host_id)));

    // A host admin is demoted or disabled only after host admin is revoked.
    for body in [json!({ "role": "editor" }), json!({ "active": false })] {
        let (s, v) = send(
            &c,
            Method::PUT,
            &format!("/users/{admin_id}"),
            &host,
            Some(body),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT, "revoke first: {v}");
    }

    let (s, v) = send(
        &c,
        Method::PUT,
        &format!("/users/{admin_id}/host-admin"),
        &host,
        Some(json!({ "host_admin": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "revoke: {v}");
    assert_eq!(me(&c, &admin).await["host_admin"], json!(false));
    assert_eq!(set_gpu(admin.clone()).await, StatusCode::FORBIDDEN);

    // The last host admin cannot be revoked (checked when the caller is the only one).
    let (_, v) = send(&c, Method::GET, "/host-admins", &host, None).await;
    let active = v["host_admins"]
        .as_array()
        .expect("list")
        .iter()
        .filter(|u| u["active"] == json!(true))
        .count();
    if active == 1 {
        let (s, v) = send(
            &c,
            Method::PUT,
            &format!("/users/{host_id}/host-admin"),
            &host,
            Some(json!({ "host_admin": false })),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT, "last host admin: {v}");
        assert_eq!(me(&c, &host).await["host_admin"], json!(true));
    } else {
        eprintln!("{active} host admins on this server; last-one check skipped");
    }
}
