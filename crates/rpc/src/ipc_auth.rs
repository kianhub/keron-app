//! Local IPC authentication.
//!
//! The engine's loopback RPC (`ws://127.0.0.1:{ipc_port}`) drives agents, so
//! reaching the port is not enough: every client (the UI, the CLI, the
//! injected `zeron mcp` server) presents this install's secret as its first
//! frame, an [`AUTHENTICATE`] call. A connection whose first frame is
//! anything else, or carries the wrong secret, gets an error reply and is
//! closed before the engine sees a single command. The secret never rides
//! the URL, so it can't land in logs.
//!
//! The secret is 32 bytes from the OS CSPRNG, hex-encoded in
//! `{data_dir}/ipc-secret` with mode 0600. The first engine to serve a data
//! dir creates it; clients read it from the same data dir, or from the file
//! named by [`SECRET_FILE_ENV`].
//!
//! It lives here rather than in the engine so clients that don't link the
//! engine (`zeron mcp`, the examples) authenticate with the same code.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use subtle::ConstantTimeEq as _;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::{ClientFrame, RpcClient, RpcError, RpcService, ServerFrame};

/// The secret's file name inside the data dir.
pub const SECRET_FILE: &str = "ipc-secret";

/// The first call on every IPC connection: `{ "secret": "<hex>" }` → `{}`.
pub const AUTHENTICATE: &str = "AuthenticateIpc";

/// Environment variable naming the secret file for a client that doesn't
/// know the data dir (the `zeron mcp` server an agent launches).
pub const SECRET_FILE_ENV: &str = "ZERON_IPC_SECRET_FILE";

/// How long a new connection may take to authenticate.
const AUTH_DEADLINE: Duration = Duration::from_secs(10);

const SECRET_BYTES: usize = 32;

/// This install's IPC secret. `Debug` never prints it.
#[derive(Clone)]
pub struct IpcSecret(Arc<str>);

impl std::fmt::Debug for IpcSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IpcSecret(..)")
    }
}

impl IpcSecret {
    /// Where the secret lives for `data_dir`.
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(SECRET_FILE)
    }

    /// Server side: the data dir's secret, created on first run. A file that
    /// is unreadable as a secret, isn't a regular file, or is readable by
    /// anyone but its owner is replaced with a fresh one.
    pub fn load_or_create(data_dir: &Path) -> std::io::Result<Self> {
        let path = Self::path(data_dir);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && private(&meta) => match Self::read(&path) {
                Ok(secret) => return Ok(secret),
                Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {
                    tracing::warn!("ipc secret unreadable; replacing it");
                }
                Err(err) => return Err(err),
            },
            Ok(_) => tracing::warn!("ipc secret file not private; replacing it"),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        std::fs::create_dir_all(data_dir)?;
        Self::create(data_dir, &path)
    }

    /// Client side: the secret of the engine serving `data_dir`.
    pub fn load(data_dir: &Path) -> std::io::Result<Self> {
        Self::read(&Self::path(data_dir))
    }

    /// Client side: the secret in the file [`SECRET_FILE_ENV`] names.
    /// `None` when the variable is unset; an error when the file is bad.
    pub fn from_env() -> Option<std::io::Result<Self>> {
        let path = std::env::var_os(SECRET_FILE_ENV).filter(|p| !p.is_empty())?;
        Some(Self::read(Path::new(&path)))
    }

    /// Read a secret file (clients handed a path, see [`SECRET_FILE_ENV`]).
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let secret = text.trim();
        if secret.len() != SECRET_BYTES * 2 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ipc secret file is malformed",
            ));
        }
        Ok(Self(secret.to_ascii_lowercase().into()))
    }

    /// A fresh secret from the OS CSPRNG.
    fn generate() -> std::io::Result<Self> {
        let mut bytes = [0u8; SECRET_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        Ok(Self(hex.into()))
    }

    /// Write a fresh secret: a private temp file beside the target, renamed
    /// over it, so a reader sees the old secret or the new one, never half.
    fn create(data_dir: &Path, path: &Path) -> std::io::Result<Self> {
        let secret = Self::generate()?;
        let mut file = tempfile::Builder::new()
            .prefix(".ipc-secret-")
            .tempfile_in(data_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(secret.0.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|err| err.error)?;
        Ok(secret)
    }

    /// Whether `presented` is this secret, compared in constant time.
    pub fn matches(&self, presented: &str) -> bool {
        presented.as_bytes().ct_eq(self.0.as_bytes()).into()
    }

    /// The value a client sends. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(unix)]
fn private(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    meta.mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn private(_: &std::fs::Metadata) -> bool {
    // The data dir lives in the user's own profile folder.
    true
}

/// Authenticate a freshly dialed connection. Must be the first call on it.
///
/// An engine that doesn't know [`AUTHENTICATE`] serves anyone who dials it,
/// so it is refused like a wrong secret: no Keron engine ever ran without
/// authentication, so it is someone else's (a stock Zeron, say).
pub async fn authenticate(client: &RpcClient, secret: &IpcSecret) -> Result<(), RpcError> {
    let call = client.call(
        AUTHENTICATE,
        serde_json::json!({ "secret": secret.expose() }),
    );
    match tokio::time::timeout(AUTH_DEADLINE, call)
        .await
        .map_err(|_| RpcError::Transport("timed out authenticating to the engine".into()))?
    {
        Ok(_) => Ok(()),
        Err(RpcError::UnknownMethod(method)) if method == AUTHENTICATE => {
            Err(RpcError::Failed(UNAUTHENTICATED_ENGINE.into()))
        }
        Err(err) => Err(err),
    }
}

/// Why a client refuses an engine that doesn't authenticate its clients.
pub const UNAUTHENTICATED_ENGINE: &str = "the engine on this port doesn't authenticate its \
     clients, so it isn't a Keron engine (a stock Zeron uses the same protocol); refusing it";

/// Dial the engine on `port` and authenticate with `secret`.
pub async fn connect(port: u16, secret: &IpcSecret) -> Result<RpcClient, RpcError> {
    connect_url(&format!("ws://127.0.0.1:{port}"), secret).await
}

/// Dial the engine at `url` (`ws://127.0.0.1:{port}`) and authenticate.
pub async fn connect_url(url: &str, secret: &IpcSecret) -> Result<RpcClient, RpcError> {
    let client = crate::connect_ws(url).await?;
    authenticate(&client, secret).await?;
    Ok(client)
}

/// Accept connections forever, serving each with `service` once it has
/// authenticated with `secret`.
pub async fn serve(listener: TcpListener, service: Arc<dyn RpcService>, secret: IpcSecret) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                tracing::debug!(%peer, "rpc: connection accepted");
                tokio::spawn(serve_socket(stream, service.clone(), secret.clone()));
            }
            Err(err) => {
                tracing::warn!(error = %err, "rpc: accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn serve_socket(stream: TcpStream, service: Arc<dyn RpcService>, secret: IpcSecret) {
    let Some(ws) = crate::server::accept_local_ws(stream).await else {
        return;
    };
    let (mut sink, mut ws_stream) = ws.split();

    let first = tokio::time::timeout(AUTH_DEADLINE, async {
        loop {
            match ws_stream.next().await {
                Some(Ok(WsMessage::Text(text))) => return Some(text),
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => continue,
                _ => return None,
            }
        }
    })
    .await
    .ok()
    .flatten();
    let (id, verdict) = match first.as_deref() {
        Some(text) => check(text, &secret),
        None => (None, Err("authentication required")),
    };
    let reply = |frame: ServerFrame| serde_json::to_string(&frame).ok();
    match verdict {
        Ok(()) => {
            let ok = reply(ServerFrame {
                id: id.unwrap_or_default(),
                ok: Some(serde_json::json!({})),
                ..Default::default()
            });
            if let Some(ok) = ok
                && sink.send(WsMessage::Text(ok)).await.is_err()
            {
                return;
            }
        }
        Err(reason) => {
            tracing::warn!(reason, "rpc: rejected an unauthenticated IPC connection");
            if let Some(id) = id
                && let Some(err) = reply(ServerFrame {
                    id,
                    err: Some(format!("unauthorized: {reason}")),
                    ..Default::default()
                })
            {
                let _ = sink.send(WsMessage::Text(err)).await;
            }
            let _ = sink
                .send(WsMessage::Close(Some(CloseFrame {
                    code: CloseCode::Policy,
                    reason: "unauthorized".into(),
                })))
                .await;
            return;
        }
    }

    crate::server::serve_ws_halves(sink, ws_stream, service).await;
}

/// Judge a connection's first message: exactly one [`AUTHENTICATE`] frame
/// carrying this install's secret. Returns the frame id when there is one,
/// so a refusal can still be answered.
fn check(text: &str, secret: &IpcSecret) -> (Option<u64>, Result<(), &'static str>) {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(frame) = lines
        .next()
        .and_then(|line| serde_json::from_str::<ClientFrame>(line).ok())
    else {
        return (None, Err("authentication required"));
    };
    let id = Some(frame.id);
    if lines.next().is_some() {
        return (id, Err("authenticate before sending anything else"));
    }
    if frame.method.as_deref() != Some(AUTHENTICATE) {
        return (id, Err("authentication required"));
    }
    match frame
        .params
        .get("secret")
        .and_then(serde_json::Value::as_str)
    {
        Some(presented) if secret.matches(presented) => (id, Ok(())),
        _ => (id, Err("wrong secret")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_creates_a_private_random_secret_and_later_runs_reuse_it() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let secret = IpcSecret::load_or_create(&data).unwrap();
        assert_eq!(secret.expose().len(), 64);
        assert!(secret.expose().bytes().all(|b| b.is_ascii_hexdigit()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(IpcSecret::path(&data))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        }
        // No temp files left behind.
        let names: Vec<_> = std::fs::read_dir(&data)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(SECRET_FILE)]);

        let again = IpcSecret::load_or_create(&data).unwrap();
        assert!(again.matches(secret.expose()));
        let client = IpcSecret::load(&data).unwrap();
        assert!(client.matches(secret.expose()));
        // Two installs never share a secret.
        let other = IpcSecret::load_or_create(&dir.path().join("other")).unwrap();
        assert!(!other.matches(secret.expose()));
        assert!(!format!("{secret:?}").contains(secret.expose()));
    }

    #[test]
    fn a_malformed_or_exposed_secret_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = IpcSecret::path(dir.path());
        std::fs::write(&path, "not a secret").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(IpcSecret::load(dir.path()).is_err());
        let fresh = IpcSecret::load_or_create(dir.path()).unwrap();
        assert_eq!(fresh.expose().len(), 64);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let replaced = IpcSecret::load_or_create(dir.path()).unwrap();
            assert!(!replaced.matches(fresh.expose()));
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn matching_requires_the_exact_secret() {
        let dir = tempfile::tempdir().unwrap();
        let secret = IpcSecret::load_or_create(dir.path()).unwrap();
        let exact = secret.expose().to_owned();
        assert!(secret.matches(&exact));
        assert!(!secret.matches(""));
        assert!(!secret.matches(&exact[..63]));
        assert!(!secret.matches(&format!("{exact}0")));
        let mut flipped = exact.clone().into_bytes();
        flipped[10] = if flipped[10] == b'0' { b'1' } else { b'0' };
        assert!(!secret.matches(std::str::from_utf8(&flipped).unwrap()));
    }

    #[test]
    fn the_first_frame_must_authenticate_alone() {
        let dir = tempfile::tempdir().unwrap();
        let secret = IpcSecret::load_or_create(dir.path()).unwrap();
        let frame = |method: &str, params: serde_json::Value| {
            serde_json::json!({ "id": 7, "method": method, "params": params }).to_string()
        };
        let good = frame(
            AUTHENTICATE,
            serde_json::json!({ "secret": secret.expose() }),
        );
        assert_eq!(check(&good, &secret), (Some(7), Ok(())));
        assert_eq!(
            check(
                &frame(
                    AUTHENTICATE,
                    serde_json::json!({ "secret": "0".repeat(64) })
                ),
                &secret
            ),
            (Some(7), Err("wrong secret"))
        );
        assert_eq!(
            check(&frame(AUTHENTICATE, serde_json::json!({})), &secret),
            (Some(7), Err("wrong secret"))
        );
        assert_eq!(
            check(&frame("ListChats", serde_json::json!({})), &secret).1,
            Err("authentication required")
        );
        let piggyback = format!("{good}\n{}", frame("ListChats", serde_json::json!({})));
        assert!(check(&piggyback, &secret).1.is_err());
        assert_eq!(
            check("garbage", &secret),
            (None, Err("authentication required"))
        );
    }

    /// An engine that serves anyone: no authentication, and an
    /// [`AUTHENTICATE`] call is just an unknown method to it.
    async fn serve_without_authentication(listener: TcpListener) {
        struct Open;
        #[async_trait::async_trait]
        impl RpcService for Open {
            async fn handle(
                &self,
                method: &str,
                _: serde_json::Value,
            ) -> Result<crate::RpcReply, RpcError> {
                Err(RpcError::UnknownMethod(method.into()))
            }
        }
        while let Ok((stream, _)) = listener.accept().await {
            if let Some(ws) = crate::server::accept_local_ws(stream).await {
                let (sink, stream) = ws.split();
                tokio::spawn(crate::server::serve_ws_halves(sink, stream, Arc::new(Open)));
            }
        }
    }

    #[tokio::test]
    async fn an_engine_that_does_not_authenticate_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve_without_authentication(listener));
        let dir = tempfile::tempdir().unwrap();
        let secret = IpcSecret::load_or_create(dir.path()).unwrap();
        let Err(err) = connect(port, &secret).await else {
            panic!("attached to an engine that doesn't authenticate");
        };
        assert_eq!(err.to_string(), UNAUTHENTICATED_ENGINE);
    }
}
