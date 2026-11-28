//! End-to-end check of each person's own menu (`/me/menu`).
//!
//! Requires:
//!   PDM_E2E=1
//!   PDM_E2E_BASE   (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN
//!
//! Run against a disposable server, never one people use: it creates two users.

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

/// A new user of the admin's organization with the given role, and a token for them.
async fn user_with_role(c: &Client, key: &str, role: &str) -> String {
    let (s, v) = send(
        c,
        Method::POST,
        "/users",
        &admin_token(),
        Some(json!({ "email": format!("menu-{role}-{key}@test.invalid"), "role": role })),
    )
    .await;
    assert!(s.is_success(), "create {role}: {s}");
    let id = v["user"]["id"].as_str().expect("user id").to_string();
    let (s, v) = send(
        c,
        Method::POST,
        &format!("/users/{id}/tokens"),
        &admin_token(),
        Some(json!({ "name": "e2e-menu" })),
    )
    .await;
    assert!(s.is_success(), "token for {role}: {s}");
    v["token"].as_str().expect("token").to_string()
}

#[tokio::test]
#[ignore]
async fn e2e_each_person_keeps_their_own_menu_and_access_does_not_change() {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return;
    }
    let c = client();
    let key = uuid::Uuid::new_v4().to_string();
    let viewer = user_with_role(&c, &key, "viewer").await;
    let editor = user_with_role(&c, &key, "editor").await;

    // A new person has the default menu.
    let (s, v) = send(&c, Method::GET, "/me/menu", &viewer, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({ "items": null, "close_others": false }));

    // A viewer stores a menu with their own groups, three levels deep, a hidden link and icons.
    let menu = json!([
        { "group": "Daily", "icon": "solid/star", "items": [
            { "nav": "items", "hidden": true },
            { "group": "Shop", "items": [{ "group": "Laser", "items": [{ "nav": "laser-nests", "icon": "regular/bell" }] }] }
        ] },
        { "group": "Workspace", "key": "workspace", "items": [{ "nav": "dashboard" }, { "nav": "admin" }] }
    ]);
    let (s, v) = send(
        &c,
        Method::PUT,
        "/me/menu",
        &viewer,
        Some(json!({ "items": menu })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["items"], menu);
    let (_, v) = send(&c, Method::GET, "/me/menu", &viewer, None).await;
    assert_eq!(v["items"], menu, "the menu is kept on the server");

    // Each person has their own: the editor still has the default.
    let (_, v) = send(&c, Method::GET, "/me/menu", &editor, None).await;
    assert_eq!(v["items"], Value::Null);

    // The option is stored on its own and keeps the menu.
    let (s, v) = send(
        &c,
        Method::PUT,
        "/me/menu",
        &viewer,
        Some(json!({ "close_others": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({ "items": menu, "close_others": true }));

    // Bad menus are refused and change nothing.
    let bad = [
        json!({ "items": [{ "nav": "not-a-page" }] }),
        json!({ "items": [{ "nav": "items" }, { "nav": "items" }] }),
        json!({ "items": [{ "group": "x".repeat(61) }] }),
        json!({ "items": [{ "group": "<script>" }] }),
        json!({ "items": [{ "nav": "items", "icon": "solid/not-an-icon" }] }),
        json!({ "items": [{ "group": "1", "items": [{ "group": "2", "items": [{ "group": "3", "items": [{ "group": "4" }] }] }] }] }),
        json!({ "items": [], "theme": "dark" }),
        json!({ "items": [{ "group": "Shop" }, { "group": "shop" }] }),
        json!({ "close_others": "yes" }),
        json!({}),
        json!([]),
    ];
    for body in bad {
        let (s, v) = send(&c, Method::PUT, "/me/menu", &viewer, Some(body.clone())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}: {v}");
    }
    let r = c
        .put(format!("{}/me/menu", base()))
        .bearer_auth(&viewer)
        .header("Content-Type", "application/json")
        .body("{")
        .send()
        .await
        .expect("PUT");
    assert_eq!(r.status(), StatusCode::BAD_REQUEST, "broken JSON");
    let r = c
        .put(format!("{}/me/menu", base()))
        .bearer_auth(&viewer)
        .header("Content-Type", "application/json")
        .body(format!(
            "{{\"items\":[{{\"group\":\"{}\"}}]}}",
            "x".repeat(70_000)
        ))
        .send()
        .await
        .expect("PUT");
    assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE, "large body");
    let (_, v) = send(&c, Method::GET, "/me/menu", &viewer, None).await;
    assert_eq!(
        v,
        json!({ "items": menu, "close_others": true }),
        "unchanged after refusals"
    );

    // Items is hidden in this viewer's menu: the viewer still opens Items. Administration shows in
    // the menu: the viewer still cannot change an admin setting.
    let (s, _) = send(&c, Method::GET, "/items?limit=1", &viewer, None).await;
    assert_eq!(s, StatusCode::OK, "the viewer still opens Items");
    let (s, _) = send(
        &c,
        Method::PUT,
        "/settings/reading-font",
        &viewer,
        Some(json!({ "ui": "system", "mono": "system" })),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "an admin link in the menu grants nothing"
    );

    // Reset puts the default back and keeps the option.
    let (s, v) = send(&c, Method::DELETE, "/me/menu", &viewer, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({ "items": null, "close_others": true }));
    let (s, v) = send(
        &c,
        Method::PUT,
        "/me/menu",
        &viewer,
        Some(json!({ "items": null, "close_others": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({ "items": null, "close_others": false }));

    // A request without a token is refused.
    let r = c
        .get(format!("{}/me/menu", base()))
        .send()
        .await
        .expect("GET");
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}
