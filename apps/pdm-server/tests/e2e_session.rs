//! End-to-end checks that web sign-in tokens expire and API tokens do not.
//!
//! Start the server with short limits, for example
//!   PDM_SESSION_IDLE_MINUTES=1 PDM_SESSION_LIFETIME_MINUTES=2
//! and run with:
//!   PDM_E2E=1
//!   PDM_E2E_BASE           (default http://127.0.0.1:8080)
//!   PDM_E2E_ADMIN_TOKEN    an admin's API token (from `pdm-cli create-token`, not a sign-in)
//!   PDM_E2E_SESSION_IDLE_MINUTES, PDM_E2E_SESSION_LIFETIME_MINUTES
//!                          the server's values (default 1 and 2; the lifetime must be at
//!                          least the idle limit + 45 s, so each test proves its own rule)
//!   cargo test -p pdm-server --test e2e_session -- --ignored
//!
//! The tests wait for the limits to pass, so they take a little over the lifetime.

use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn base() -> String {
    std::env::var("PDM_E2E_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".into())
}

fn admin_token() -> String {
    std::env::var("PDM_E2E_ADMIN_TOKEN").expect("PDM_E2E_ADMIN_TOKEN must be set")
}

fn minutes(var: &str, default: u64) -> Duration {
    let m = std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    Duration::from_secs(m * 60)
}

fn idle() -> Duration {
    minutes("PDM_E2E_SESSION_IDLE_MINUTES", 1)
}

fn lifetime() -> Duration {
    minutes("PDM_E2E_SESSION_LIFETIME_MINUTES", 2)
}

/// Whether to run (`PDM_E2E=1`), after checking the limits leave each rule room to show.
fn enabled() -> bool {
    if std::env::var("PDM_E2E").ok().as_deref() != Some("1") {
        eprintln!("PDM_E2E not set; skipping");
        return false;
    }
    assert!(
        idle() >= Duration::from_secs(60) && lifetime() >= idle() + Duration::from_secs(45),
        "need an idle limit of at least 1 minute and a lifetime at least 45 s longer"
    );
    true
}

/// A new user with a password; returns (user id, email, password).
async fn create_user(client: &Client, tag: &str) -> (String, String, String) {
    let email = format!("session-{tag}-{}@example.test", uuid::Uuid::new_v4());
    let password = format!("pw-{}", uuid::Uuid::new_v4());
    let r = client
        .post(format!("{}/users", base()))
        .bearer_auth(admin_token())
        .json(
            &json!({ "email": email, "display_name": tag, "role": "editor", "password": password }),
        )
        .send()
        .await
        .expect("POST /users");
    assert!(r.status().is_success(), "create user: {}", r.status());
    let body: Value = r.json().await.expect("user JSON");
    let id = body["user"]["id"].as_str().expect("user id").to_string();
    (id, email, password)
}

async fn api_token(client: &Client, user_id: &str, name: &str) -> String {
    let r = client
        .post(format!("{}/users/{user_id}/tokens", base()))
        .bearer_auth(admin_token())
        .json(&json!({ "name": name }))
        .send()
        .await
        .expect("POST /users/{id}/tokens");
    assert!(r.status().is_success(), "create token: {}", r.status());
    let body: Value = r.json().await.expect("token JSON");
    body["token"].as_str().expect("token").to_string()
}

async fn sign_in(client: &Client, email: &str, password: &str) -> String {
    let r = client
        .post(format!("{}/auth/login", base()))
        .json(&json!({ "username": email, "password": password }))
        .send()
        .await
        .expect("POST /auth/login");
    assert!(r.status().is_success(), "sign in: {}", r.status());
    let body: Value = r.json().await.expect("login JSON");
    body["token"].as_str().expect("token").to_string()
}

/// `GET /me` with a token: the status and the error code, if any.
async fn me(client: &Client, token: &str) -> (StatusCode, Option<String>) {
    let r = client
        .get(format!("{}/me", base()))
        .bearer_auth(token)
        .send()
        .await
        .expect("GET /me");
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    let code = body["error"]["code"].as_str().map(str::to_string);
    (status, code)
}

#[tokio::test]
#[ignore]
async fn idle_sign_in_ends_but_api_tokens_keep_working() {
    if !enabled() {
        return;
    }
    let client = Client::new();
    let (user_id, email, password) = create_user(&client, "idle").await;
    // An API token named like a sign-in must not be taken for one.
    let named_like_sign_in = api_token(&client, &user_id, "web-login").await;
    let plain_api = api_token(&client, &user_id, "e2e-api").await;
    let session = sign_in(&client, &email, &password).await;
    assert_eq!(me(&client, &session).await.0, StatusCode::OK);

    // Nobody uses any of the three tokens for longer than the idle limit.
    tokio::time::sleep(idle() + Duration::from_secs(15)).await;

    let (status, code) = me(&client, &session).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "idle sign-in still accepted"
    );
    assert_eq!(code.as_deref(), Some("session_expired"));
    // The expired sign-in was deleted: it stays refused, as an unknown token.
    let (status, code) = me(&client, &session).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code.as_deref(), Some("auth_failed"));

    assert_eq!(me(&client, &named_like_sign_in).await.0, StatusCode::OK);
    assert_eq!(me(&client, &plain_api).await.0, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn sign_in_ends_at_its_lifetime_even_when_used() {
    if !enabled() {
        return;
    }
    let client = Client::new();
    let (user_id, email, password) = create_user(&client, "lifetime").await;
    let api = api_token(&client, &user_id, "e2e-api").await;
    let session = sign_in(&client, &email, &password).await;
    let start = Instant::now();
    let step = (idle() / 4).max(Duration::from_secs(5));

    // Used well within the idle limit until shortly before the lifetime ends.
    while start.elapsed() + step < lifetime().saturating_sub(Duration::from_secs(5)) {
        assert_eq!(
            me(&client, &session).await.0,
            StatusCode::OK,
            "active sign-in refused after {:?}",
            start.elapsed()
        );
        tokio::time::sleep(step).await;
    }
    // Past the lifetime, still within the idle limit of the last use.
    let past = lifetime() + Duration::from_secs(10);
    tokio::time::sleep(past.saturating_sub(start.elapsed())).await;

    let (status, code) = me(&client, &session).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "sign-in outlived its lifetime"
    );
    assert_eq!(code.as_deref(), Some("session_expired"));
    // An API token made at the same moment has no lifetime.
    assert_eq!(me(&client, &api).await.0, StatusCode::OK);
}

#[tokio::test]
#[ignore]
async fn sign_out_revokes_the_sign_in() {
    if !enabled() {
        return;
    }
    let client = Client::new();
    let (_, email, password) = create_user(&client, "logout").await;
    let session = sign_in(&client, &email, &password).await;
    assert_eq!(me(&client, &session).await.0, StatusCode::OK);

    let r = client
        .post(format!("{}/auth/logout", base()))
        .bearer_auth(&session)
        .send()
        .await
        .expect("POST /auth/logout");
    assert!(r.status().is_success(), "logout: {}", r.status());

    assert_eq!(me(&client, &session).await.0, StatusCode::UNAUTHORIZED);
}
