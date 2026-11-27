// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The console. Loopback by default, reached through the operator's own SSH tunnel. Any other
//! address only over HTTPS with a client certificate required of every connection.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bijective_authority::Role;
use bijective_common::configuration::{Invalid, Reason, known, required, within};
use bijective_console::app;
use bijective_console::desk::{Desk, MAX_FORM_BYTES, Refusal};
use bijective_console::events::{Sources, subscribe};
use bijective_console::http::{
    JSON, Request, read_request, respond, respond_as, respond_file, see_other,
};
use bijective_console::tls::{TlsFiles, server_config};
use bijective_console::{API_EVENTS, API_PANELS, Console, Page, Streams};
use bijective_serialization::signal::node::{NodeStreams, of_mode};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

const PROGRAM: &str = "bijective-console";
const DEFAULT_PORT: u16 = 7333;

/// Every accepted argument, so a mistyped one is refused with the alternatives (configuration assurance `Unknown`).
const FLAGS: &[&str] = &[
    "--store",
    "--archive",
    "--port",
    "--listen",
    "--tls-cert",
    "--tls-key",
    "--client-ca",
    "--operator",
    "--role",
    "--key",
    "--mode",
    "--manifest",
    "--run",
    "--app",
    "--serve-one",
];

/// Flags that take no value.
const SWITCHES: &[&str] = &["--serve-one"];

const ARGUMENT: &str = "argument";

/// Usable ports. Zero is excluded: binding it picks a random port and the printed address would be wrong.
const LOWEST_PORT: i128 = 1;
const HIGHEST_PORT: i128 = 65_535;

/// A peer that sends nothing for this long is dropped: the console answers one request at a time.
const PEER_TIMEOUT: Duration = Duration::from_secs(10);

const NO_TABLE: &str = "{\"error\":\"no table is behind this address\"}";

struct Args {
    store_root: PathBuf,
    /// The market archive's root, for the market panels. Without it they show the streams only.
    archive: Option<PathBuf>,
    listen: IpAddr,
    port: u16,
    tls: Option<TlsFiles>,
    /// Operator, role and key file: who the control page signs as.
    desk: Option<(String, Role, PathBuf)>,
    /// The run whose record is read and whose node the control page signs for.
    node: NodeStreams,
    /// Both or neither: the manifest and the run directory answer nothing alone.
    standing: Option<(PathBuf, PathBuf)>,
    /// A static application served under `/app/` (`app`). Without it that address is not found.
    app: Option<PathBuf>,
    serve_one: bool,
}

/// Each flag once, with its value. A switch holds an empty value.
fn flag_values<I: Iterator<Item = String>>(given: I) -> Result<BTreeMap<String, String>, Invalid> {
    let mut values = BTreeMap::new();
    let mut it = given;
    while let Some(flag) = it.next() {
        known(ARGUMENT, &flag, FLAGS)?;
        let value = if SWITCHES.contains(&flag.as_str()) {
            String::new()
        } else {
            it.next().ok_or_else(|| Invalid::new(&flag, Reason::Absent))?
        };
        values.insert(flag, value);
    }
    Ok(values)
}

/// All of `names` or none of them. A part names the first one missing.
fn all_or_none(
    values: &BTreeMap<String, String>,
    names: &[&str],
) -> Result<Option<Vec<String>>, Invalid> {
    let found: Vec<Option<&String>> = names.iter().map(|n| values.get(*n)).collect();
    if found.iter().all(Option::is_none) {
        return Ok(None);
    }
    let missing = names.iter().zip(&found).find(|(_, v)| v.is_none());
    let present = names.iter().zip(&found).find(|(_, v)| v.is_some());
    match (missing, present) {
        (Some((other, _)), Some((field, _))) => {
            Err(Invalid::new(*field, Reason::Requires { other: (*other).to_string() }))
        }
        _ => Ok(Some(found.into_iter().flatten().cloned().collect())),
    }
}

fn malformed(field: &str, detail: String) -> Invalid {
    Invalid::new(field, Reason::Malformed { detail })
}

/// Takes the arguments rather than reading them, so every refusal below has a test.
fn parse_args<I: Iterator<Item = String>>(given: I) -> Result<Args, Invalid> {
    let values = flag_values(given)?;
    let text = |name: &str, default: &str| values.get(name).cloned().unwrap_or(default.into());

    let raw_port = text("--port", &DEFAULT_PORT.to_string());
    let port: i128 =
        raw_port.parse().map_err(|_| malformed("--port", format!("{raw_port} is not a number")))?;
    // The range is what makes the conversion below total.
    let port = within("--port", port, LOWEST_PORT, HIGHEST_PORT)? as u16;

    let tls =
        all_or_none(&values, &["--tls-cert", "--tls-key", "--client-ca"])?.map(|v| TlsFiles {
            certificate: PathBuf::from(&v[0]),
            key: PathBuf::from(&v[1]),
            client_authority: PathBuf::from(&v[2]),
        });
    let listen: IpAddr = match values.get("--listen") {
        Some(raw) => {
            raw.parse().map_err(|_| malformed("--listen", format!("{raw} is not an address")))?
        }
        None => IpAddr::V4(Ipv4Addr::LOCALHOST),
    };
    if !listen.is_loopback() && tls.is_none() {
        return Err(Invalid::new("--listen", Reason::Requires { other: "--tls-cert".into() }));
    }

    let desk = match all_or_none(&values, &["--operator", "--role", "--key"])? {
        Some(v) => {
            let role = Role::named(&v[1]).ok_or_else(|| {
                malformed("--role", format!("{} is not a role this build knows", v[1]))
            })?;
            Some((v[0].clone(), role, PathBuf::from(&v[2])))
        }
        None => None,
    };
    let mode = text("--mode", "live");
    let node = of_mode(&mode).ok_or_else(|| {
        malformed("--mode", format!("{mode} is not a run mode: live, paper or backtest"))
    })?;
    let standing = all_or_none(&values, &["--manifest", "--run"])?
        .map(|v| (PathBuf::from(&v[0]), PathBuf::from(&v[1])));

    Ok(Args {
        store_root: required("--store", values.get("--store").map(PathBuf::from))?,
        archive: values.get("--archive").map(PathBuf::from),
        listen,
        port,
        tls,
        desk,
        node,
        standing,
        app: values.get("--app").map(PathBuf::from),
        serve_one: values.contains_key("--serve-one"),
    })
}

/// The console the arguments describe. A desk whose key cannot be read stops the start.
fn console_of(args: &Args) -> Result<Console, String> {
    let Some((operator, role, key_file)) = &args.desk else { return Ok(Console::default()) };
    let mut token = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut token)
        .map_err(|_| "the system could not produce random bytes".to_string())?;
    let desk = Desk::new(operator.clone(), *role, key_file.clone(), &token)?;
    Ok(Console::with_desk(desk))
}

fn main() -> std::process::ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{PROGRAM}: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    let started = args.tls.as_ref().map(server_config).transpose().and_then(|tls| {
        let console = console_of(&args)?;
        let listener = TcpListener::bind((args.listen, args.port))
            .map_err(|e| format!("{}:{} could not be taken: {e}", args.listen, args.port))?;
        Ok((tls, console, listener))
    });
    let (tls, mut console, listener) = match started {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{PROGRAM}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let scheme = if tls.is_some() { "https" } else { "http" };
    eprintln!(
        "{PROGRAM}: reading {} on {scheme}://{}:{}",
        args.store_root.display(),
        args.listen,
        args.port
    );
    let sources = Sources {
        store_root: args.store_root.clone(),
        archive: args.archive.clone(),
        standing: args.standing.clone(),
        streams: Streams::of(args.node),
    };
    // One request at a time, so the fold kept between them needs no lock. An event stream is
    // the exception: it is handed to its own thread with its own fold (`events`).
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let _ = stream.set_read_timeout(Some(PEER_TIMEOUT));
        let _ = stream.set_write_timeout(Some(PEER_TIMEOUT));
        let answered = match &tls {
            Some(config) => over_tls(config, stream, &args, &sources, &mut console),
            None => answer(stream, &args, &sources, &mut console),
        };
        if let Err(e) = answered {
            eprintln!("{PROGRAM}: a request could not be answered: {e}");
        }
        if args.serve_one {
            break;
        }
    }
    std::process::ExitCode::SUCCESS
}

/// A connection without a certificate the client authority issued fails in the handshake, before
/// any byte of a request is read.
fn over_tls(
    config: &Arc<ServerConfig>,
    stream: std::net::TcpStream,
    args: &Args,
    sources: &Sources,
    console: &mut Console,
) -> std::io::Result<()> {
    let connection = ServerConnection::new(config.clone()).map_err(std::io::Error::other)?;
    answer(StreamOwned::new(connection, stream), args, sources, console)
}

/// Refreshed per request, so the page cannot be older than the ask. The only request that is not
/// a read is a control form, and it is answered by sending the browser to read what came of it.
fn answer<S: Read + Write + Send + 'static>(
    mut stream: S,
    args: &Args,
    sources: &Sources,
    console: &mut Console,
) -> std::io::Result<()> {
    let request = match read_request(&mut stream, MAX_FORM_BYTES) {
        Ok(r) => r,
        Err(_) => return respond(&mut stream, "400 Bad Request", "<p>Not a request.</p>"),
    };
    let path = request.target.split('?').next().unwrap_or("");
    match request.method.as_str() {
        "GET" if path == API_EVENTS => subscribe(stream, sources),
        "GET" | "HEAD" if path == "/app" => see_other(stream, app::PREFIX),
        "GET" | "HEAD" if path.starts_with(app::PREFIX) => application(stream, args, path),
        "GET" | "HEAD" => {
            let archive = sources.archive.as_deref();
            console.refresh(&sources.store_root, &sources.streams, archive, sources.standing());
            if path == API_PANELS || path.starts_with("/api/") {
                return match console.api(path) {
                    Some(body) => respond_as(&mut stream, "200 OK", JSON, &body),
                    None => respond_as(&mut stream, "404 Not Found", JSON, NO_TABLE),
                };
            }
            if path == "/metrics" {
                let body = console.metrics();
                return respond_as(
                    &mut stream,
                    "200 OK",
                    bijective_console::metrics::CONTENT_TYPE,
                    &body,
                );
            }
            match console.page_noting(&request.target, notice(&request).as_deref()) {
                Page::Found(body) => respond(&mut stream, "200 OK", &body),
                Page::NotFound(body) => respond(&mut stream, "404 Not Found", &body),
            }
        }
        "POST" if request.target == "/control" => submit(stream, &request, args, console),
        _ => respond(stream, "405 Method Not Allowed", "<p>This console only reads.</p>"),
    }
}

/// A file of the application, read from disk on each request. Nothing of the console's fold is
/// touched.
fn application(stream: impl Write, args: &Args, path: &str) -> std::io::Result<()> {
    let served = args.app.as_deref().map_or(app::Served::NotFound, |root| app::serve(root, path));
    match served {
        app::Served::File { content_type, body } => {
            respond_file(stream, content_type, app::POLICY, &body)
        }
        app::Served::NotFound => {
            respond(stream, "404 Not Found", "<p>No file of the application is here.</p>")
        }
    }
}

fn submit(
    stream: impl Write,
    request: &Request,
    args: &Args,
    console: &Console,
) -> std::io::Result<()> {
    let Some(desk) = console.desk().filter(|_| request.is_own_form()) else {
        return respond(stream, "403 Forbidden", "<p>This console issues nothing from here.</p>");
    };
    match desk.submit(&args.store_root, args.node, &request.body) {
        Ok(position) => {
            eprintln!("{PROGRAM}: command at position {position} signed as {}", desk.operator);
            see_other(stream, &format!("/control?issued={position}"))
        }
        Err(refusal) => {
            eprintln!("{PROGRAM}: a control form was refused: {refusal:?}");
            see_other(stream, &format!("/control?refused={}", refusal_code(&refusal)))
        }
    }
}

fn refusal_code(refusal: &Refusal) -> &'static str {
    match refusal {
        Refusal::NotThisConsole => "token",
        Refusal::NotOffered { .. } => "not-offered",
        Refusal::Malformed { .. } => "malformed",
        Refusal::NotIssued { .. } => "not-issued",
    }
}

/// What came of the last form, from the address it sent the browser to. Only codes this program
/// writes are read back, so the address cannot put words on the page.
fn notice(request: &Request) -> Option<String> {
    if let Some(position) = request.query("issued").and_then(|p| p.parse::<u64>().ok()) {
        return Some(format!(
            "Command signed and appended at position {position} of the command stream. What \
             came of it is under Audit."
        ));
    }
    let why = match request.query("refused")? {
        "token" => "the form was not one this console served",
        "not-offered" => "that action is not offered to this operator's role",
        "malformed" => "the form could not be read",
        "not-issued" => "it could not be signed or appended; the console's log names why",
        _ => return None,
    };
    Some(format!("Nothing was issued: {why}."))
}

#[cfg(test)]
#[path = "../tests/unit/arguments_tests.rs"]
mod arguments_tests;
