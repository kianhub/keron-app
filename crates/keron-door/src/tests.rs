//! The client against a fake door and authorization server on 127.0.0.1.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::*;

struct Seen {
    path: String,
    authorization: Option<String>,
    body: String,
}

#[derive(Default)]
struct FakeState {
    seen: Vec<Seen>,
    registrations: usize,
    refreshes: usize,
    issued: usize,
    valid_access: HashSet<String>,
    valid_refresh: Option<String>,
}

/// Plays the door and WorkOS: it issues `access-<n>` and `refresh-<n>`,
/// rotates the refresh token on every refresh, and refuses an old one.
struct Fake {
    origin: String,
    state: Arc<Mutex<FakeState>>,
}

impl Fake {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(FakeState::default()));
        let (shared, base) = (state.clone(), origin.clone());
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, shared.clone(), base.clone()));
            }
        });
        Self { origin, state }
    }

    fn config(&self) -> DoorConfig {
        DoorConfig {
            base_url: self.origin.clone(),
            resource: format!("{}/mcp", self.origin),
            client_name: "Keron app".into(),
            callback_port: 0,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap()
    }

    /// The form of the last token request.
    fn last_token_form(&self) -> Vec<(String, String)> {
        let state = self.state();
        let seen = state.seen.iter().rfind(|s| s.path == "/token").unwrap();
        form(&seen.body)
    }

    fn revoke_access(&self) {
        self.state().valid_access.clear();
    }

    fn revoke_refresh(&self) {
        self.state().valid_refresh = None;
    }
}

async fn serve(mut socket: tokio::net::TcpStream, state: Arc<Mutex<FakeState>>, origin: String) {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        data.extend_from_slice(&chunk[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let header = |name: &str| {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    };
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while data.len() < head_end + length {
        let n = socket.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&data[head_end..]).to_string();
    let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let path = target.split('?').next().unwrap().to_string();
    let authorization = header("authorization");
    let (status, answer) = {
        let mut state = state.lock().unwrap();
        state.seen.push(Seen {
            path: path.clone(),
            authorization: authorization.clone(),
            body: body.clone(),
        });
        route(&mut state, &origin, &path, authorization.as_deref(), &body)
    };
    if path == "/token" {
        // Slow enough that concurrent refreshes would overlap.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let answer = answer.to_string();
    let response = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
        answer.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
}

fn route(
    state: &mut FakeState,
    origin: &str,
    path: &str,
    authorization: Option<&str>,
    body: &str,
) -> (u16, Value) {
    let invalid_grant = json!({"error": "invalid_grant", "error_description": "Invalid grant."});
    match path {
        "/.well-known/oauth-protected-resource/mcp" => (
            200,
            json!({"resource": format!("{origin}/mcp"), "authorization_servers": [origin]}),
        ),
        "/.well-known/oauth-authorization-server" => (
            200,
            json!({
                "issuer": origin,
                "authorization_endpoint": format!("{origin}/authorize"),
                "token_endpoint": format!("{origin}/token"),
                "registration_endpoint": format!("{origin}/register"),
                "code_challenge_methods_supported": ["S256"],
            }),
        ),
        "/register" => {
            state.registrations += 1;
            (
                201,
                json!({"client_id": format!("client-{}", state.registrations)}),
            )
        }
        "/token" => {
            let form = form(body);
            let field = |name: &str| {
                form.iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, v)| v.as_str())
            };
            let ok = match field("grant_type") {
                Some("authorization_code") => field("code") == Some("the-code"),
                Some("refresh_token") => {
                    state.refreshes += 1;
                    field("refresh_token")
                        .is_some_and(|t| state.valid_refresh.as_deref() == Some(t))
                }
                _ => false,
            };
            if !ok {
                return (400, invalid_grant);
            }
            state.issued += 1;
            let (access, refresh) = (
                format!("access-{}", state.issued),
                format!("refresh-{}", state.issued),
            );
            state.valid_access.insert(access.clone());
            state.valid_refresh = Some(refresh.clone());
            (
                200,
                json!({"access_token": access, "refresh_token": refresh, "expires_in": 3600, "token_type": "Bearer"}),
            )
        }
        "/sources/gmail" => {
            let token = authorization.and_then(|a| a.strip_prefix("Bearer "));
            if token.is_some_and(|t| state.valid_access.contains(t)) {
                (200, json!({"messages": 3}))
            } else {
                (401, json!({"error": "invalid_token"}))
            }
        }
        "/forbidden" => (
            403,
            json!({"error": "forbidden", "error_description": "This account isn't the owner's."}),
        ),
        _ => (404, json!({"error": "not_found"})),
    }
}

fn form(body: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect()
}

fn query(url: &str) -> Vec<(String, String)> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

fn one(pairs: &[(String, String)], name: &str) -> String {
    let values: Vec<_> = pairs.iter().filter(|(key, _)| key == name).collect();
    assert_eq!(values.len(), 1, "{name} should appear exactly once");
    values[0].1.clone()
}

/// The browser coming back to the loopback listener.
async fn visit(url: &str) -> (u16, String) {
    let response = reqwest::get(url).await.unwrap();
    (response.status().as_u16(), response.text().await.unwrap())
}

/// A whole sign-in, the browser answering with `code` and `state`; the
/// authorize URL, the result and the page the browser got.
async fn sign_in_with(
    client: &DoorClient,
    state: Option<&str>,
) -> (String, Result<(), DoorError>, (u16, String)) {
    let pending = client.begin_sign_in().await.unwrap();
    let url = pending.authorize_url.clone();
    let params = query(&url);
    let redirect = one(&params, "redirect_uri");
    let state = state.map_or_else(|| one(&params, "state"), str::to_string);
    let callback = format!("{redirect}?code=the-code&state={state}");
    let (result, page) = tokio::join!(client.finish_sign_in(pending), visit(&callback));
    (url, result, page)
}

async fn sign_in(client: &DoorClient) -> String {
    let (url, result, (status, page)) = sign_in_with(client, None).await;
    result.unwrap();
    assert_eq!(status, 200);
    assert!(page.contains("You're signed in to Keron's memory."));
    url
}

fn client(fake: &Fake) -> (DoorClient, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::default());
    (DoorClient::new(fake.config(), store.clone()), store)
}

#[tokio::test]
async fn sign_in_registers_once_and_keeps_the_refresh_token() {
    let fake = Fake::start().await;
    let (client, store) = client(&fake);
    let resource = format!("{}/mcp", fake.origin);
    assert!(!client.has_session());

    let url = sign_in(&client).await;

    let params = query(&url);
    assert!(url.starts_with(&format!("{}/authorize?", fake.origin)));
    assert_eq!(one(&params, "resource"), resource);
    assert_eq!(one(&params, "response_type"), "code");
    assert_eq!(one(&params, "client_id"), "client-1");
    assert_eq!(one(&params, "scope"), "openid offline_access");
    assert_eq!(one(&params, "code_challenge_method"), "S256");
    let redirect = one(&params, "redirect_uri");
    assert!(redirect.starts_with("http://127.0.0.1:") && redirect.ends_with("/callback"));

    let registered: Value = {
        let state = fake.state();
        let seen = state.seen.iter().find(|s| s.path == "/register").unwrap();
        serde_json::from_str(&seen.body).unwrap()
    };
    assert_eq!(
        registered,
        json!({
            "client_name": "Keron app",
            "redirect_uris": [redirect],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })
    );

    let exchange = fake.last_token_form();
    assert_eq!(one(&exchange, "grant_type"), "authorization_code");
    assert_eq!(one(&exchange, "code"), "the-code");
    assert_eq!(one(&exchange, "client_id"), "client-1");
    assert_eq!(one(&exchange, "redirect_uri"), redirect);
    assert_eq!(one(&exchange, "resource"), resource);
    let verifier = one(&exchange, "code_verifier");
    assert_eq!(
        one(&params, "code_challenge"),
        BASE64_URL.encode(Sha256::digest(verifier.as_bytes()))
    );

    assert!(client.has_session());
    assert_eq!(
        store.get(REFRESH_ACCOUNT).unwrap().as_deref(),
        Some("refresh-1")
    );
    let stored: Value =
        serde_json::from_str(&store.get(REGISTRATION_ACCOUNT).unwrap().unwrap()).unwrap();
    assert_eq!(stored["client_id"], "client-1");
    assert_eq!(stored["resource"], resource.as_str());
    assert_eq!(stored["issuer"], fake.origin.as_str());

    // The access token from the sign-in is used as it is.
    assert_eq!(
        client.get_json("/sources/gmail").await.unwrap(),
        json!({"messages": 3})
    );
    assert_eq!(fake.state().refreshes, 0);

    // A second sign-in reuses the registration and its port.
    let again = sign_in(&client).await;
    assert_eq!(fake.state().registrations, 1);
    assert_eq!(one(&query(&again), "redirect_uri"), redirect);
}

#[tokio::test]
async fn a_refused_access_token_is_refreshed_once_and_the_rotation_kept() {
    let fake = Fake::start().await;
    let (client, store) = client(&fake);
    sign_in(&client).await;
    fake.revoke_access();

    // Both callers get a 401; only one refresh may run, since the second
    // would present a refresh token that rotation has already retired.
    let other = client.clone();
    let (a, b) = tokio::join!(
        client.get_json("/sources/gmail"),
        other.get_json("/sources/gmail")
    );
    assert_eq!(a.unwrap(), json!({"messages": 3}));
    assert_eq!(b.unwrap(), json!({"messages": 3}));
    assert_eq!(fake.state().refreshes, 1);
    assert_eq!(
        store.get(REFRESH_ACCOUNT).unwrap().as_deref(),
        Some("refresh-2")
    );

    let refresh = fake.last_token_form();
    assert_eq!(one(&refresh, "grant_type"), "refresh_token");
    assert_eq!(one(&refresh, "refresh_token"), "refresh-1");
    assert_eq!(one(&refresh, "client_id"), "client-1");
    assert_eq!(one(&refresh, "resource"), format!("{}/mcp", fake.origin));
    let last = fake
        .state()
        .seen
        .iter()
        .rfind(|s| s.path == "/sources/gmail")
        .and_then(|s| s.authorization.clone());
    assert_eq!(last.as_deref(), Some("Bearer access-2"));
}

#[tokio::test]
async fn a_rotation_is_kept_when_the_caller_gives_up_mid_refresh() {
    let fake = Fake::start().await;
    let (client, store) = client(&fake);
    sign_in(&client).await;
    fake.revoke_access();

    // Drop the request once the token endpoint has rotated the refresh
    // token but before its answer arrives (Home drops fetches like this).
    tokio::select! {
        _ = client.get_json("/sources/gmail") => panic!("the refresh answered too soon"),
        _ = async {
            while fake.state().refreshes == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        } => {}
    }

    assert_eq!(
        client.get_json("/sources/gmail").await.unwrap(),
        json!({"messages": 3})
    );
    assert_eq!(fake.state().refreshes, 1);
    assert_eq!(
        store.get(REFRESH_ACCOUNT).unwrap().as_deref(),
        Some("refresh-2")
    );
}

#[tokio::test]
async fn a_revoked_sign_in_expires_and_is_forgotten() {
    let fake = Fake::start().await;
    let (client, store) = client(&fake);
    sign_in(&client).await;
    fake.revoke_access();
    fake.revoke_refresh();

    assert_eq!(
        client.get_json("/sources/gmail").await,
        Err(DoorError::Expired)
    );
    assert_eq!(store.get(REFRESH_ACCOUNT).unwrap(), None);
    assert!(!client.has_session());
    assert!(store.get(REGISTRATION_ACCOUNT).unwrap().is_some());
    assert_eq!(
        client.get_json("/sources/gmail").await,
        Err(DoorError::SignedOut)
    );
}

#[tokio::test]
async fn a_callback_with_another_state_refuses_the_sign_in() {
    let fake = Fake::start().await;
    let (client, store) = client(&fake);

    let (_, result, (status, _)) = sign_in_with(&client, Some("someone-else")).await;

    assert!(matches!(result, Err(DoorError::Refused(_))));
    assert_eq!(status, 400);
    assert!(!fake.state().seen.iter().any(|s| s.path == "/token"));
    assert_eq!(store.get(REFRESH_ACCOUNT).unwrap(), None);
}

#[tokio::test]
async fn door_errors_carry_the_error_description() {
    let fake = Fake::start().await;
    let (client, _) = client(&fake);
    sign_in(&client).await;

    assert_eq!(
        client.get_json("/forbidden").await,
        Err(DoorError::Http {
            status: 403,
            error: Some("forbidden".into()),
            message: "This account isn't the owner's.".into(),
        })
    );
}
