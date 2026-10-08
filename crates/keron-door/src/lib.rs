//! keron-door: the Keron app's own sign-in to the Mac mini's public door,
//! and authenticated JSON requests through it.
//!
//! The door (keron-memory behind Cloudflare Tunnel, `keron_config::DOOR_URL`)
//! admits only the owner's WorkOS account. It follows MCP's authorization
//! spec, so this client does what an MCP client does
//! (docs/research/mcp-security.md in the Keron repo):
//!
//! 1. Discovery: `GET {base}/.well-known/oauth-protected-resource/mcp` names
//!    the authorization server and the resource (`keron_config::DOOR_RESOURCE`);
//!    the server's `/.well-known/oauth-authorization-server` names the
//!    authorization, token and registration endpoints.
//! 2. Registration, once: RFC 7591 dynamic client registration as a public
//!    client ("Keron app", no secret, `token_endpoint_auth_method = none`)
//!    with the loopback redirect `http://127.0.0.1:<port>/callback`
//!    (`keron_config::DOOR_SIGN_IN_CALLBACK_PORT`, or a free port when that
//!    one is taken, which registers again). Stored in the [`SecretStore`].
//! 3. Sign-in: authorization code with PKCE (S256) in the owner's browser,
//!    `scope = "openid offline_access"`, and exactly one `resource` parameter
//!    (the door's resource) on the authorization, code and refresh requests:
//!    WorkOS refuses a repeated one with `invalid_request`. A one-shot
//!    loopback listener takes the redirect and checks `state`.
//! 4. Storage: the registration (JSON) and the refresh token go in the login
//!    Keychain (service `keron`, accounts [`REGISTRATION_ACCOUNT`] and
//!    [`REFRESH_ACCOUNT`]); access tokens stay in memory. WorkOS rotates
//!    refresh tokens, so a rotated one is stored before it is used.
//! 5. Use: [`DoorClient::get_json`] / [`DoorClient::post_json`] send the
//!    access token as a bearer token, refresh it when it is about to expire
//!    or the door answers 401, and retry once. Refreshes are single-flight.
//!
//! Tokens, codes and verifiers never go into logs, errors or argv. Every
//! error message here is safe to show.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use reqwest::Method;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use serde_json::Value;

mod loopback;
mod oauth;
pub mod store;

pub use store::{KeychainStore, MemoryStore, SecretStore};

/// Keychain account of the dynamic client registration (JSON).
pub const REGISTRATION_ACCOUNT: &str = "app-door-client";
/// Keychain account of the refresh token.
pub const REFRESH_ACCOUNT: &str = "app-door-refresh";
/// Keychain service shared with Keron's other secrets.
pub const KEYCHAIN_SERVICE: &str = "keron";

/// Where the door is and how this app registers with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoorConfig {
    /// Base URL, like `https://memory.example.com` (`keron_config::DOOR_URL`).
    /// Request paths (`/sources/<name>`, `/memory/<name>`) are relative to it.
    pub base_url: String,
    /// The OAuth resource and token audience, like
    /// `https://memory.example.com/mcp` (`keron_config::DOOR_RESOURCE`).
    pub resource: String,
    /// The client name registered with WorkOS ("Keron app").
    pub client_name: String,
    /// Preferred loopback callback port.
    pub callback_port: u16,
}

impl DoorConfig {
    /// The owner's door, from keron.toml.
    pub fn keron() -> Self {
        Self {
            base_url: keron_config::DOOR_URL.to_string(),
            resource: keron_config::DOOR_RESOURCE.to_string(),
            client_name: "Keron app".to_string(),
            callback_port: keron_config::DOOR_SIGN_IN_CALLBACK_PORT,
        }
    }
}

/// Everything that can go wrong, with messages safe to show the owner.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DoorError {
    /// No refresh token is stored: sign in first.
    #[error("not signed in to your memory")]
    SignedOut,
    /// The stored sign-in was refused (revoked, expired, or for another
    /// account); it has been forgotten, so the next call is `SignedOut`.
    #[error("the sign-in to your memory expired or was revoked; sign in again")]
    Expired,
    /// The door answered with an error status. `error` and `message` come
    /// from its JSON body (`error`, `error_description`) when it has one.
    #[error("the door answered {status}: {message}")]
    Http {
        status: u16,
        error: Option<String>,
        message: String,
    },
    /// The door or WorkOS couldn't be reached.
    #[error("can't reach the door: {0}")]
    Network(String),
    /// An answer that doesn't follow the protocol (bad metadata, no token...).
    #[error("unexpected answer: {0}")]
    Protocol(String),
    /// Reading or writing the Keychain failed.
    #[error("Keychain: {0}")]
    Store(String),
    /// The browser sign-in didn't finish in time.
    #[error("the sign-in timed out")]
    Timeout,
    /// The authorization server sent the browser back with an error, or the
    /// owner cancelled.
    #[error("the sign-in didn't finish: {0}")]
    Refused(String),
}

/// A browser sign-in in progress: open [`PendingSignIn::authorize_url`] in
/// the browser, then pass this to [`DoorClient::finish_sign_in`], which
/// waits for the loopback redirect.
pub struct PendingSignIn {
    /// The URL to open in the owner's browser.
    pub authorize_url: String,
    listener: tokio::net::TcpListener,
    state: String,
    verifier: String,
    registration: oauth::Registration,
}

/// A cheap, cloneable handle; clones share one token cache and one refresh.
#[derive(Clone)]
pub struct DoorClient {
    inner: Arc<Inner>,
}

struct Inner {
    config: DoorConfig,
    store: Arc<dyn SecretStore>,
    http: reqwest::Client,
    session: Mutex<Session>,
    /// Held across a refresh, so only one runs at a time: WorkOS rotates
    /// refresh tokens, and a second refresh with the old one would fail.
    refresh_gate: tokio::sync::Mutex<()>,
}

/// The in-memory side of the sign-in.
#[derive(Default)]
struct Session {
    /// Bumped by every sign-in and sign-out, so a refresh that was in flight
    /// across one doesn't store or forget a token that's no longer its own.
    generation: u64,
    access: Option<Access>,
}

struct Access {
    token: String,
    refresh_at: Instant,
}

/// How long the browser has to come back to the loopback listener.
const SIGN_IN_WAIT: Duration = Duration::from_secs(10 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl DoorClient {
    pub fn new(config: DoorConfig, store: Arc<dyn SecretStore>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .default_headers(headers)
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                config,
                store,
                http,
                session: Mutex::default(),
                refresh_gate: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// The owner's door with the login Keychain.
    pub fn keron() -> Self {
        Self::new(
            DoorConfig::keron(),
            Arc::new(KeychainStore::new(KEYCHAIN_SERVICE)),
        )
    }

    pub fn config(&self) -> &DoorConfig {
        &self.inner.config
    }

    /// Whether a refresh token is stored. Doesn't contact the door; a stored
    /// token can still turn out to be revoked ([`DoorError::Expired`]).
    pub fn has_session(&self) -> bool {
        matches!(self.inner.store.get(REFRESH_ACCOUNT), Ok(Some(token)) if !token.is_empty())
    }

    /// Discovery, registration if needed, and the authorize URL with a
    /// loopback listener already bound.
    pub async fn begin_sign_in(&self) -> Result<PendingSignIn, DoorError> {
        let inner = &*self.inner;
        let config = &inner.config;
        let server = oauth::discover(&inner.http, config).await?;
        let stored = oauth::Registration::load(&*inner.store)?.filter(|registration| {
            registration.resource.trim_end_matches('/') == config.resource.trim_end_matches('/')
                && registration.issuer == server.issuer
        });
        // The stored registration's port if it has one; when that port is
        // taken, any free one, which needs a registration of its own.
        let port = stored
            .as_ref()
            .and_then(oauth::Registration::redirect_port)
            .unwrap_or(config.callback_port);
        let listener = match loopback::bind(port).await {
            Ok(listener) => listener,
            Err(_) => loopback::bind(0).await.map_err(|e| {
                DoorError::Network(format!("can't listen on 127.0.0.1 for the sign-in ({e})"))
            })?,
        };
        let redirect_uri = loopback::redirect_uri(&listener)?;
        let registration = match stored.filter(|r| r.redirect_uri == redirect_uri) {
            Some(registration) => registration,
            None => {
                let registration =
                    oauth::register(&inner.http, &server, config, &redirect_uri).await?;
                registration.save(&*inner.store)?;
                tracing::info!(
                    host = oauth::host_of(&server.issuer),
                    "registered the app with the door's sign-in server"
                );
                registration
            }
        };
        let (verifier, challenge) = oauth::pkce_pair();
        let state = oauth::random_url_token();
        let authorize_url =
            oauth::authorize_url(&registration, &challenge, &state, &config.resource)?;
        Ok(PendingSignIn {
            authorize_url,
            listener,
            state,
            verifier,
            registration,
        })
    }

    /// Wait (up to 10 minutes) for the browser to come back to the loopback
    /// listener, exchange the code, and store the refresh token.
    pub async fn finish_sign_in(&self, pending: PendingSignIn) -> Result<(), DoorError> {
        let PendingSignIn {
            listener,
            state,
            verifier,
            registration,
            ..
        } = pending;
        let (code, browser) =
            tokio::time::timeout(SIGN_IN_WAIT, loopback::wait_for_code(&listener, &state))
                .await
                .map_err(|_| DoorError::Timeout)??;
        drop(listener);
        let result = self.redeem(&registration, &code, &verifier).await;
        loopback::answer(browser, &result).await;
        result
    }

    /// Exchange the code, then keep the refresh token and the access token.
    async fn redeem(
        &self,
        registration: &oauth::Registration,
        code: &str,
        verifier: &str,
    ) -> Result<(), DoorError> {
        let inner = &*self.inner;
        let grant = oauth::exchange_code(
            &inner.http,
            registration,
            code,
            verifier,
            &inner.config.resource,
        )
        .await?;
        let Some(refresh_token) = grant.refresh_token else {
            return Err(DoorError::Protocol(
                "no refresh token came back; offline_access wasn't granted".into(),
            ));
        };
        let mut session = lock(&inner.session);
        session.generation += 1;
        session.access = None;
        inner.store.set(REFRESH_ACCOUNT, &refresh_token)?;
        session.access = Some(Access {
            token: grant.access_token,
            refresh_at: grant.refresh_at,
        });
        tracing::info!("signed in to the door");
        Ok(())
    }

    /// Forget the refresh token and any cached access token. The
    /// registration is kept, so the next sign-in doesn't register again.
    pub fn sign_out(&self) -> Result<(), DoorError> {
        let mut session = lock(&self.inner.session);
        session.generation += 1;
        session.access = None;
        self.inner.store.delete(REFRESH_ACCOUNT)
    }

    /// `GET {base}{path}` with the bearer token; the JSON body on 2xx.
    pub async fn get_json(&self, path: &str) -> Result<Value, DoorError> {
        self.request(Method::GET, path, None).await
    }

    /// `POST {base}{path}` with a JSON body and the bearer token; the JSON
    /// body on 2xx.
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value, DoorError> {
        self.request(Method::POST, path, Some(body)).await
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, DoorError> {
        let base = self.inner.config.base_url.trim_end_matches('/');
        let slash = if path.starts_with('/') { "" } else { "/" };
        let url = format!("{base}{slash}{path}");
        let token = self.access_token().await?;
        let mut response = self.send(&method, &url, body, &token).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let fresh = self.refresh(Some(&token)).await?;
            response = self.send(&method, &url, body, &fresh).await?;
        }
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| oauth::network_error(&e, &url))?;
        tracing::debug!(status = status.as_u16(), %method, path, "door answered");
        if !status.is_success() {
            return Err(oauth::http_error(status, &bytes));
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| DoorError::Protocol(format!("the door's answer to {path} isn't JSON")))
    }

    async fn send(
        &self,
        method: &Method,
        url: &str,
        body: Option<&Value>,
        token: &str,
    ) -> Result<reqwest::Response, DoorError> {
        let mut request = self
            .inner
            .http
            .request(method.clone(), url)
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        request
            .send()
            .await
            .map_err(|e| oauth::network_error(&e, url))
    }

    /// A cached access token that isn't about to expire and isn't `stale`.
    fn cached_token(&self, stale: Option<&str>) -> Option<String> {
        let session = lock(&self.inner.session);
        let access = session.access.as_ref()?;
        (Instant::now() < access.refresh_at && stale != Some(access.token.as_str()))
            .then(|| access.token.clone())
    }

    async fn access_token(&self) -> Result<String, DoorError> {
        match self.cached_token(None) {
            Some(token) => Ok(token),
            None => self.refresh(None).await,
        }
    }

    /// A new access token from the stored refresh token, unless another
    /// caller got one (newer than `stale`) while this one waited its turn.
    async fn refresh(&self, stale: Option<&str>) -> Result<String, DoorError> {
        let inner = &*self.inner;
        let _turn = inner.refresh_gate.lock().await;
        if let Some(token) = self.cached_token(stale) {
            return Ok(token);
        }
        let generation = lock(&inner.session).generation;
        let refresh_token = inner
            .store
            .get(REFRESH_ACCOUNT)?
            .filter(|token| !token.is_empty())
            .ok_or(DoorError::SignedOut)?;
        let registration = oauth::Registration::load(&*inner.store)?.ok_or(DoorError::SignedOut)?;
        let result = oauth::refresh_grant(
            &inner.http,
            &registration,
            &refresh_token,
            &inner.config.resource,
        )
        .await;

        let mut session = lock(&inner.session);
        if session.generation != generation {
            // Signed out or in again meanwhile: that session wins.
            return session
                .access
                .as_ref()
                .map(|access| access.token.clone())
                .ok_or(DoorError::SignedOut);
        }
        match result {
            Ok(grant) => {
                // Store a rotated refresh token before using what came with
                // it: the old one no longer works.
                if let Some(rotated) = grant.refresh_token.filter(|t| *t != refresh_token) {
                    inner.store.set(REFRESH_ACCOUNT, &rotated)?;
                }
                session.access = Some(Access {
                    token: grant.access_token.clone(),
                    refresh_at: grant.refresh_at,
                });
                Ok(grant.access_token)
            }
            Err(DoorError::Http { status, error, .. })
                if status == 400
                    || (status == 401 && error.as_deref() == Some("invalid_grant")) =>
            {
                tracing::info!(status, "the door's sign-in was refused; forgetting it");
                session.generation += 1;
                session.access = None;
                inner.store.delete(REFRESH_ACCOUNT)?;
                Err(DoorError::Expired)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests;
