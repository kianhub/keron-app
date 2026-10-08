//! The OAuth side of the door: discovery (RFC 9728, RFC 8414), dynamic
//! client registration (RFC 7591), PKCE, and the token endpoint with one
//! `resource` parameter (RFC 8707). Mirrors the Python client the mini's
//! jobs use (memory/src/keron_memory/door_client.py in the Keron repo).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::store::SecretStore;
use crate::{DoorConfig, DoorError, REGISTRATION_ACCOUNT};

pub(crate) const SCOPE: &str = "openid offline_access";
/// How long an access token lasts when its answer says nothing about expiry.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(300);
/// Refresh an access token this long before it expires.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The authorization server the door names, with its endpoints.
pub(crate) struct AuthServer {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
}

/// This app's client registration, for one door and one authorization
/// server. Stored as JSON under [`REGISTRATION_ACCOUNT`].
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Registration {
    pub resource: String,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub client_id: String,
    pub redirect_uri: String,
    /// Only if the server insists on one; a public client has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

impl Registration {
    /// The stored registration; `None` when there is none or it doesn't parse.
    pub fn load(store: &dyn SecretStore) -> Result<Option<Self>, DoorError> {
        Ok(store
            .get(REGISTRATION_ACCOUNT)?
            .and_then(|raw| serde_json::from_str(&raw).ok()))
    }

    pub fn save(&self, store: &dyn SecretStore) -> Result<(), DoorError> {
        let raw = serde_json::to_string(self)
            .map_err(|_| DoorError::Store("couldn't encode the registration".into()))?;
        store.set(REGISTRATION_ACCOUNT, &raw)
    }

    pub fn redirect_port(&self) -> Option<u16> {
        Url::parse(&self.redirect_uri).ok()?.port()
    }
}

/// What a token endpoint gave back.
pub(crate) struct Grant {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// When to refresh: the expiry, less a margin.
    pub refresh_at: Instant,
}

/// Follow the door's metadata to its authorization server.
pub(crate) async fn discover(
    http: &reqwest::Client,
    config: &DoorConfig,
) -> Result<AuthServer, DoorError> {
    let resource = config.resource.trim_end_matches('/');
    let base = config.base_url.trim_end_matches('/');
    // RFC 9728 puts the resource's path after the well-known name; FastMCP
    // serves only that one, and Keron's door adds the bare one as well.
    let suffix = Url::parse(resource)
        .map(|url| url.path().trim_end_matches('/').to_string())
        .unwrap_or_default();
    let mut urls = vec![format!(
        "{base}/.well-known/oauth-protected-resource{suffix}"
    )];
    if !suffix.is_empty() {
        urls.push(format!("{base}/.well-known/oauth-protected-resource"));
    }
    let mut meta = None;
    let mut last = String::new();
    for url in &urls {
        match get_object(http, url).await? {
            Ok(object) => {
                meta = Some(object);
                break;
            }
            Err(why) => last = format!("the door's metadata at {url} {why}"),
        }
    }
    let meta = meta.ok_or(DoorError::Protocol(last))?;
    let named = meta.get("resource").and_then(Value::as_str).unwrap_or("");
    if named.trim_end_matches('/') != resource {
        return Err(DoorError::Protocol(format!(
            "the door's metadata names another resource than {resource}"
        )));
    }
    let issuer = meta
        .get("authorization_servers")
        .and_then(Value::as_array)
        .and_then(|servers| servers.first())
        .and_then(Value::as_str)
        .filter(|issuer| secure_url(issuer))
        .ok_or_else(|| {
            DoorError::Protocol("the door's metadata names no https authorization server".into())
        })?
        .trim_end_matches('/')
        .to_string();

    let url = well_known(&issuer, "oauth-authorization-server")?;
    let data = get_object(http, &url)
        .await?
        .map_err(|why| DoorError::Protocol(format!("the sign-in server's metadata {why}")))?;
    let named = data.get("issuer").and_then(Value::as_str).unwrap_or("");
    if named.trim_end_matches('/') != issuer {
        return Err(DoorError::Protocol(format!(
            "the sign-in server's metadata names another issuer than {issuer}"
        )));
    }
    let s256 = data
        .get("code_challenge_methods_supported")
        .and_then(Value::as_array)
        .is_some_and(|methods| methods.iter().any(|m| m == "S256"));
    if !s256 {
        return Err(DoorError::Protocol(
            "the sign-in server doesn't offer PKCE with S256".into(),
        ));
    }
    let endpoint = |key: &str| match data.get(key).and_then(Value::as_str) {
        Some(url) if secure_url(url) => Ok(url.to_string()),
        Some(_) => Err(DoorError::Protocol(format!(
            "the sign-in server's {key} isn't an https URL"
        ))),
        None => Err(DoorError::Protocol(format!(
            "the sign-in server's metadata lacks {key}"
        ))),
    };
    Ok(AuthServer {
        authorization_endpoint: endpoint("authorization_endpoint")?,
        token_endpoint: endpoint("token_endpoint")?,
        registration_endpoint: endpoint("registration_endpoint")?,
        issuer,
    })
}

/// RFC 7591 registration as a public client with one loopback redirect.
pub(crate) async fn register(
    http: &reqwest::Client,
    server: &AuthServer,
    config: &DoorConfig,
    redirect_uri: &str,
) -> Result<Registration, DoorError> {
    let body = json!({
        "client_name": config.client_name,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let response = http
        .post(&server.registration_endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| network_error(&e, &server.registration_endpoint))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| network_error(&e, &server.registration_endpoint))?;
    tracing::debug!(
        status = status.as_u16(),
        "door client registration answered"
    );
    if !status.is_success() {
        return Err(http_error(status, &bytes));
    }
    let data: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let client_id = data
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| DoorError::Protocol("the registration answer has no client_id".into()))?;
    Ok(Registration {
        resource: config.resource.trim_end_matches('/').to_string(),
        issuer: server.issuer.clone(),
        authorization_endpoint: server.authorization_endpoint.clone(),
        token_endpoint: server.token_endpoint.clone(),
        client_id: client_id.to_string(),
        redirect_uri: redirect_uri.to_string(),
        client_secret: data
            .get("client_secret")
            .and_then(Value::as_str)
            .filter(|secret| !secret.is_empty())
            .map(str::to_string),
    })
}

/// 32 random bytes (two v4 uuids, OS randomness), base64url without padding.
pub(crate) fn random_url_token() -> String {
    let raw: Vec<u8> = uuid::Uuid::new_v4()
        .as_bytes()
        .iter()
        .chain(uuid::Uuid::new_v4().as_bytes())
        .copied()
        .collect();
    BASE64_URL.encode(&raw)
}

/// PKCE: a random verifier and its S256 challenge.
pub(crate) fn pkce_pair() -> (String, String) {
    let verifier = random_url_token();
    let challenge = BASE64_URL.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// The browser's sign-in link. Any query the endpoint already has is kept,
/// except our own parameters, so `resource` appears exactly once.
pub(crate) fn authorize_url(
    registration: &Registration,
    challenge: &str,
    state: &str,
    resource: &str,
) -> Result<String, DoorError> {
    const OURS: [&str; 8] = [
        "response_type",
        "client_id",
        "redirect_uri",
        "scope",
        "state",
        "code_challenge",
        "code_challenge_method",
        "resource",
    ];
    let mut url = Url::parse(&registration.authorization_endpoint)
        .map_err(|_| DoorError::Protocol("the stored authorization endpoint isn't a URL".into()))?;
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !OURS.contains(&key.as_ref()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(kept)
        .append_pair("response_type", "code")
        .append_pair("client_id", &registration.client_id)
        .append_pair("redirect_uri", &registration.redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", resource);
    Ok(url.into())
}

pub(crate) async fn exchange_code(
    http: &reqwest::Client,
    registration: &Registration,
    code: &str,
    verifier: &str,
    resource: &str,
) -> Result<Grant, DoorError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &registration.redirect_uri),
        ("client_id", &registration.client_id),
        ("code_verifier", verifier),
        ("resource", resource),
    ];
    token_request(http, registration, &form).await
}

pub(crate) async fn refresh_grant(
    http: &reqwest::Client,
    registration: &Registration,
    refresh_token: &str,
    resource: &str,
) -> Result<Grant, DoorError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", &registration.client_id),
        ("resource", resource),
    ];
    token_request(http, registration, &form).await
}

/// A token-endpoint POST. Neither the form nor the answer's body ever goes
/// into an error or a log: both can carry token material.
async fn token_request(
    http: &reqwest::Client,
    registration: &Registration,
    form: &[(&str, &str)],
) -> Result<Grant, DoorError> {
    let endpoint = &registration.token_endpoint;
    let mut form = form.to_vec();
    if let Some(secret) = &registration.client_secret {
        form.push(("client_secret", secret));
    }
    let asked = Instant::now();
    let response = http
        .post(endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| network_error(&e, endpoint))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| network_error(&e, endpoint))?;
    tracing::debug!(
        status = status.as_u16(),
        host = host_of(endpoint),
        "token endpoint answered"
    );
    if !status.is_success() {
        return Err(http_error(status, &bytes));
    }
    let data: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let access_token = data
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| DoorError::Protocol("the token answer has no access token".into()))?
        .to_string();
    let lifetime = match data.get("expires_in").and_then(Value::as_f64) {
        Some(seconds) if seconds > 0.0 => Duration::from_secs_f64(seconds),
        _ => jwt_lifetime(&access_token).unwrap_or(DEFAULT_LIFETIME),
    };
    Ok(Grant {
        refresh_token: data
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_string),
        refresh_at: asked + lifetime.saturating_sub(EXPIRY_MARGIN),
        access_token,
    })
}

/// How long a JWT has left by its `exp`, read without checking the
/// signature (the door does the checking). `None` for anything else.
fn jwt_lifetime(token: &str) -> Option<Duration> {
    let mut parts = token.split('.');
    let (_, payload, _, None) = (parts.next()?, parts.next()?, parts.next()?, parts.next()) else {
        return None;
    };
    let claims: Value = serde_json::from_slice(&BASE64_URL.decode(payload).ok()?).ok()?;
    let exp = claims.get("exp")?.as_f64()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    Some(Duration::from_secs_f64((exp - now).max(0.0)))
}

/// `GET` a JSON object: `Ok(Err(why))` for an answer that isn't one.
async fn get_object(
    http: &reqwest::Client,
    url: &str,
) -> Result<Result<Map<String, Value>, String>, DoorError> {
    let response = http
        .get(url)
        .send()
        .await
        .map_err(|e| network_error(&e, url))?;
    let status = response.status();
    if !status.is_success() {
        return Ok(Err(format!("answered {}", status.as_u16())));
    }
    let bytes = response.bytes().await.map_err(|e| network_error(&e, url))?;
    Ok(match serde_json::from_slice(&bytes) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err("isn't a JSON object".into()),
    })
}

/// RFC 8414: `/.well-known/<name>` goes between the host and the path.
fn well_known(url: &str, name: &str) -> Result<String, DoorError> {
    let mut parsed =
        Url::parse(url).map_err(|_| DoorError::Protocol(format!("{url} isn't a URL")))?;
    let path = format!("/.well-known/{name}{}", parsed.path().trim_end_matches('/'));
    parsed.set_path(&path);
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed.into())
}

/// https, or plain http to this machine (for tests against a local fake).
fn secure_url(url: &str) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        _ => false,
    }
}

/// A non-2xx answer as [`DoorError::Http`], with the message taken from the
/// body's `error_description` or `error` and nothing else from it.
pub(crate) fn http_error(status: StatusCode, body: &[u8]) -> DoorError {
    let data: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let field = |key: &str| data.get(key).and_then(Value::as_str).map(safe);
    let error = field("error").filter(|error| !error.is_empty());
    let message = field("error_description")
        .filter(|description| !description.is_empty())
        .or_else(|| error.clone())
        .or_else(|| status.canonical_reason().map(str::to_string))
        .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
    DoorError::Http {
        status: status.as_u16(),
        error,
        message,
    }
}

/// A transport failure, named by host and kind only.
pub(crate) fn network_error(error: &reqwest::Error, url: &str) -> DoorError {
    let what = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "couldn't connect"
    } else {
        "the request failed"
    };
    DoorError::Network(format!("{}: {what}", host_of(url)))
}

pub(crate) fn host_of(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "the server".into())
}

/// Printable, one line, short: for text that came from a server.
pub(crate) fn safe(text: &str) -> String {
    const LIMIT: usize = 200;
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= LIMIT {
        return clean;
    }
    let mut cut: String = clean.chars().take(LIMIT - 1).collect();
    cut.push('…');
    cut
}
