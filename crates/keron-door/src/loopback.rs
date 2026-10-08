//! The one-shot loopback listener that takes the browser's sign-in redirect.

use std::net::Ipv4Addr;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use crate::DoorError;

const CALLBACK_PATH: &str = "/callback";

/// A listener on 127.0.0.1:`port` (0 = any free port). The redirect names
/// 127.0.0.1, so there is no IPv6 fallback.
pub(crate) async fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await
}

pub(crate) fn redirect_uri(listener: &TcpListener) -> Result<String, DoorError> {
    let port = listener
        .local_addr()
        .map_err(|e| DoorError::Network(format!("the sign-in listener has no port ({e})")))?
        .port();
    Ok(format!("http://127.0.0.1:{port}{CALLBACK_PATH}"))
}

/// Serve the listener until the browser comes back to `/callback`: its
/// `code` and the browser's socket, to be answered once the code is
/// redeemed. Other paths (a favicon) get a 404 and are ignored. A wrong
/// `state`, or an `error` from the authorization server, ends the sign-in;
/// the browser is answered here then.
pub(crate) async fn wait_for_code(
    listener: &TcpListener,
    state: &str,
) -> Result<(String, TcpStream), DoorError> {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Some(target) = read_request_target(&mut socket).await else {
            continue;
        };
        let url = url::Url::parse(&format!("http://127.0.0.1{target}")).ok();
        let Some(url) = url.filter(|url| url.path() == CALLBACK_PATH) else {
            let _ = socket
                .write_all(http_response("404 Not Found", "text/plain", "").as_bytes())
                .await;
            continue;
        };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        let failure = if !same(param("state").as_deref().unwrap_or(""), state) {
            DoorError::Refused("the browser came back from a different sign-in".into())
        } else if let Some(error) = param("error") {
            let reason = param("error_description")
                .filter(|description| !description.is_empty())
                .unwrap_or(error);
            DoorError::Refused(crate::oauth::safe(&reason))
        } else if let Some(code) = param("code").filter(|code| !code.is_empty()) {
            return Ok((code, socket));
        } else {
            DoorError::Refused("no authorization code came back".into())
        };
        answer(socket, &Err(failure.clone())).await;
        return Err(failure);
    }
}

/// Tell the browser how the sign-in went, in a small page.
pub(crate) async fn answer(mut socket: TcpStream, result: &Result<(), DoorError>) {
    let (status, message) = match result {
        Ok(()) => (
            "200 OK",
            "You're signed in to Keron's memory. You can close this tab.".to_string(),
        ),
        Err(error) => {
            let text = error.to_string();
            let mut chars = text.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
            let text = format!("{}{}.", first.unwrap_or_default(), chars.as_str());
            ("400 Bad Request", text)
        }
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Keron</title><p>{}</p>\n",
        html_escape(&message)
    );
    let response = http_response(status, "text/html; charset=utf-8", &body);
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// The request target of a browser's `GET` (headers read and discarded).
async fn read_request_target(socket: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    let read = async {
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&chunk[..n]);
        }
        Some(())
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .ok()??;
    let line = String::from_utf8_lossy(&head);
    let mut parts = line.lines().next()?.split_whitespace();
    (parts.next()? == "GET").then_some(())?;
    parts
        .next()
        .filter(|target| target.starts_with('/'))
        .map(str::to_string)
}

fn http_response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    )
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Compares in time that doesn't depend on where the strings differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}
