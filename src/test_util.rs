// SPDX-License-Identifier: MIT

//! Helpers shared by tests (compiled only for `cargo test`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// A local HTTP server that answers one request with `status` and a JSON
/// `body`. Returns its port and the request body it received.
pub fn serve_once(status: &str, body: &str) -> (u16, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(String::new()));
    let seen2 = seen.clone();
    let (status, body) = (status.to_owned(), body.to_owned());
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut len = 0;
        loop {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut b = vec![0; len];
        r.read_exact(&mut b).unwrap();
        *seen2.lock().unwrap() = String::from_utf8(b).unwrap();
        write!(
            s,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    (port, seen)
}
