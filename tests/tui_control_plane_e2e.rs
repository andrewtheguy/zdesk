//! The native control plane as one process graph: two real gateway children,
//! their private endpoints — Unix sockets, or named pipes on Windows — and one
//! browser-facing TCP port routed by subdomain.

mod common;

use std::net::Ipv4Addr;

use zdesk::worker::access::{Access, PinState};
use zdesk::worker::manager::{SharedPort, Supervisor};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[tokio::test]
async fn instances_start_stop_and_share_the_master_port() {
    let root = common::ScratchDir::new("tui-instances");

    for name in ["one", "two"] {
        let dir = root.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join("zdesk.toml"),
            format!("[branding]\ntext = \"{name}\"\n"),
        )
        .unwrap();
    }

    let mut supervisor =
        Supervisor::open(root.path().to_path_buf(), env!("CARGO_BIN_EXE_zdesk").into())
            .await
            .unwrap();
    let port = free_port();
    let access = Access::open(port);
    let router = SharedPort::bind(port, supervisor.routes(), access.clone()).await.unwrap();
    supervisor.start("one", None).await.unwrap();
    supervisor.start("two", None).await.unwrap();

    let refused = request(router.port(), "one", "/api/targets", None).await;
    assert!(refused.starts_with("HTTP/1.1 401 Unauthorized"), "{refused}");
    let PinState::Open { digits, .. } = access.pin() else {
        panic!("a new control plane has an open PIN");
    };
    let login = log_in(router.port(), &digits).await;
    let one = request(router.port(), "one", "/api/config", Some(&login)).await;
    let two = request(router.port(), "two", "/api/config", Some(&login)).await;
    assert!(one.contains("\"branding\":\"one\""), "{one}");
    assert!(two.contains("\"branding\":\"two\""), "{two}");

    let landing = request_master(router.port(), "/api/instances", &login).await;
    assert!(landing.contains("\"open\":\"/__zdesk/open/one\""), "{landing}");
    assert!(landing.contains("\"open\":\"/__zdesk/open/two\""), "{landing}");

    // A restarted worker has a new token, which the master presents for the
    // browser: the login stands, whatever happened to the PIN since.
    access.draw();
    supervisor.restart("two", None).await.unwrap();
    let restarted = request(router.port(), "two", "/api/config", Some(&login)).await;
    assert!(restarted.contains("\"branding\":\"two\""), "{restarted}");

    supervisor.stop("one").await.unwrap();
    let stopped = request(router.port(), "one", "/api/config", Some(&login)).await;
    assert!(stopped.starts_with("HTTP/1.1 503 Service Unavailable"), "{stopped}");
    let still_running = request(router.port(), "two", "/api/config", Some(&login)).await;
    assert!(still_running.contains("\"branding\":\"two\""), "{still_running}");
    #[cfg(unix)]
    assert!(!root.path().join("one/gateway.sock").exists());

    supervisor.shutdown().await;
    #[cfg(unix)]
    assert!(!root.path().join("two/gateway.sock").exists());
    let gone = request(router.port(), "two", "/api/config", Some(&login)).await;
    assert!(gone.starts_with("HTTP/1.1 503 Service Unavailable"), "{gone}");
}

/// A port nothing is listening on, found by taking one and letting it go. The
/// control plane never asks the kernel for its port — it is typed into a browser —
/// so a test that wants one out of the way picks it here instead.
fn free_port() -> u16 {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Enter `pin` on the landing page, as its page posts it, and return the login
/// cookie it sets.
async fn log_in(port: u16, pin: &str) -> String {
    let body = format!("{{\"pin\":\"{pin}\"}}");
    let response = raw_request(
        port,
        &format!(
            "POST /api/auth/pin HTTP/1.1\r\nHost: zdesk.localhost:{port}\r\nOrigin: http://zdesk.localhost:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    response
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("set-cookie")
                .then(|| value.trim().split(';').next().unwrap().to_owned())
        })
        .expect("the PIN logs the browser in")
}

async fn request(port: u16, instance: &str, path: &str, cookie: Option<&str>) -> String {
    let cookie = cookie.map_or(String::new(), |value| format!("Cookie: {value}\r\n"));
    raw_request(
        port,
        &format!(
            "GET {path} HTTP/1.1\r\nHost: {instance}.zdesk.localhost:{port}\r\n{cookie}Connection: close\r\n\r\n"
        ),
    )
    .await
}

async fn request_master(port: u16, path: &str, cookie: &str) -> String {
    raw_request(
        port,
        &format!(
            "GET {path} HTTP/1.1\r\nHost: zdesk.localhost:{port}\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await
}

async fn raw_request(port: u16, request: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}
