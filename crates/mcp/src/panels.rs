// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! A console table, read the way any program reads one: the console serves a single request on
//! loopback (`--serve-one`) and this asks it for the route. The tables keep the console's open
//! format (`docs/concepts/observability.md`), and no console code is linked here.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CONNECT_WITHIN: Duration = Duration::from_secs(10);
const ANSWER_WITHIN: Duration = Duration::from_secs(30);

/// The body the console answers at `route` for the record at `store`, in run `mode`.
///
/// # Errors
/// The console could not be started, did not listen in time, or did not answer 200.
pub(crate) fn read(
    console: &Path,
    store: &Path,
    mode: &str,
    route: &str,
) -> Result<String, String> {
    let port = free_port()?;
    let args: Vec<OsString> = vec![
        "--store".into(),
        store.into(),
        "--mode".into(),
        mode.into(),
        "--port".into(),
        port.to_string().into(),
        "--serve-one".into(),
    ];
    let mut child = Command::new(console)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", console.display()))?;

    let answered = connect(port).and_then(|stream| ask(stream, route));
    let _ = child.kill();
    let _ = child.wait();
    let mut refusal = String::new();
    if answered.is_err()
        && let Some(mut err) = child.stderr.take()
    {
        let _ = err.read_to_string(&mut refusal);
    }
    answered.map_err(|e| format!("{e} {}", refusal.trim()).trim().to_string())
}

/// A port the system just handed out. Another program could take it before the console binds,
/// and the console then refuses with the reason, which reaches the agent.
fn free_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("no free port: {e}"))?;
    listener.local_addr().map(|a| a.port()).map_err(|e| format!("no free port: {e}"))
}

fn connect(port: u16) -> Result<TcpStream, String> {
    let started = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return Ok(stream),
            Err(_) if started.elapsed() < CONNECT_WITHIN => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("the console did not listen on {port}: {e}")),
        }
    }
}

fn ask(mut stream: TcpStream, route: &str) -> Result<String, String> {
    stream.set_read_timeout(Some(ANSWER_WITHIN)).map_err(|e| e.to_string())?;
    let request = format!("GET {route} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|e| format!("the request was not sent: {e}"))?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|e| format!("the console did not answer: {e}"))?;
    body_of(&String::from_utf8_lossy(&response))
}

/// The body of a 200 answer. Any other status is refused with the console's own words.
pub(crate) fn body_of(response: &str) -> Result<String, String> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| "the console's answer has no body".to_string())?;
    let status = head.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) == Some("200") {
        Ok(body.to_string())
    } else {
        Err(format!("the console answered {status}: {}", body.trim()))
    }
}

#[cfg(test)]
#[path = "../tests/unit/panels_tests.rs"]
mod tests;
