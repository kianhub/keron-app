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

use std::sync::Arc;

use serde_json::Value;

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
    // The implementation keeps its loopback listener, PKCE verifier, state
    // and registration here.
    _private: (),
}

/// A cheap, cloneable handle; clones share one token cache and one refresh.
#[derive(Clone)]
pub struct DoorClient {
    inner: Arc<Inner>,
}

struct Inner {
    config: DoorConfig,
    store: Arc<dyn SecretStore>,
}

impl DoorClient {
    pub fn new(config: DoorConfig, store: Arc<dyn SecretStore>) -> Self {
        Self {
            inner: Arc::new(Inner { config, store }),
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
        let _ = &self.inner.store;
        todo!("keron-door: has_session")
    }

    /// Discovery, registration if needed, and the authorize URL with a
    /// loopback listener already bound.
    pub async fn begin_sign_in(&self) -> Result<PendingSignIn, DoorError> {
        todo!("keron-door: begin_sign_in")
    }

    /// Wait (up to 10 minutes) for the browser to come back to the loopback
    /// listener, exchange the code, and store the refresh token.
    pub async fn finish_sign_in(&self, pending: PendingSignIn) -> Result<(), DoorError> {
        let _ = pending;
        todo!("keron-door: finish_sign_in")
    }

    /// Forget the refresh token and any cached access token. The
    /// registration is kept, so the next sign-in doesn't register again.
    pub fn sign_out(&self) -> Result<(), DoorError> {
        todo!("keron-door: sign_out")
    }

    /// `GET {base}{path}` with the bearer token; the JSON body on 2xx.
    pub async fn get_json(&self, path: &str) -> Result<Value, DoorError> {
        let _ = path;
        todo!("keron-door: get_json")
    }

    /// `POST {base}{path}` with a JSON body and the bearer token; the JSON
    /// body on 2xx.
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value, DoorError> {
        let _ = (path, body);
        todo!("keron-door: post_json")
    }
}
