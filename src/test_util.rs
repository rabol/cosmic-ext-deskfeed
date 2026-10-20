// SPDX-License-Identifier: MIT

//! Helpers shared by tests (compiled only for `cargo test`).

use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};

/// A mock Microsoft token endpoint (mockito): answers every
/// `POST …/oauth2/v2.0/token` with `status` (for example "400 Bad
/// Request") and the JSON `body`, and keeps the request bodies it got.
/// Any other request (the signing keys, for example) gets mockito's 501.
pub struct TokenServer {
    _server: mockito::ServerGuard,
    pub port: u16,
    sent: Arc<Mutex<Vec<String>>>,
}

impl TokenServer {
    /// Every request body received, in order, one per line.
    pub fn sent(&self) -> String {
        self.sent.lock().unwrap().join("\n")
    }
}

/// The port a mockito server listens on (it binds 127.0.0.1).
pub fn port_of(server: &mockito::ServerGuard) -> u16 {
    server
        .host_with_port()
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .expect("mockito listens on host:port")
}

pub fn token_server(status: &str, body: &str) -> TokenServer {
    let mut server = mockito::Server::new();
    let port = port_of(&server);
    let code: usize = status
        .split_whitespace()
        .next()
        .and_then(|c| c.parse().ok())
        .expect("a status like \"200 OK\"");
    let sent = Arc::new(Mutex::new(Vec::new()));
    let (seen, body) = (sent.clone(), body.to_owned());
    server
        .mock(
            "POST",
            mockito::Matcher::Regex(r"/oauth2/v2\.0/token$".into()),
        )
        .with_status(code)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |request| {
            let text = request.utf8_lossy_body().unwrap_or_default().into_owned();
            seen.lock().unwrap().push(text);
            body.clone().into_bytes()
        })
        .create();
    TokenServer {
        _server: server,
        port,
        sent,
    }
}

/// Random-input testing (ADR 0023), shared with the sign-in helper.
pub use common::testkit::random_inputs;

/// A private D-Bus session bus (`dbus-daemon`), ended when dropped,
/// however the test ends, so a failing test can't leave it running (and
/// holding the test's output open). Tests that need a bus service use
/// this, never the desktop's bus.
pub struct PrivateBus {
    daemon: std::process::Child,
    address: String,
}

impl PrivateBus {
    pub fn start() -> Self {
        let mut daemon = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("dbus-daemon (package dbus)");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().expect("piped"))
            .read_line(&mut address)
            .expect("dbus-daemon prints its address");
        Self {
            daemon,
            address: address.trim().to_owned(),
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}
