//! Wire protocol between the headless session server and the thin terminal client.
//!
//! Messages are newline-delimited JSON over a loopback TCP socket. The server
//! renders the whole UI into a buffer and ships cell updates; the client only
//! paints them and forwards input events.

use std::{
    collections::hash_map::RandomState,
    fs,
    hash::{BuildHasher, Hasher},
    io::{self, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use crossterm::event::Event;
use ratatui::buffer::Cell;
use serde::{Deserialize, Serialize};

/// What an agent's hook reports about itself.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum ReportState {
    Working,
    Blocked,
    Idle,
    /// The agent exited; fall back to screen heuristics.
    Release,
}

#[derive(Serialize, Deserialize)]
pub enum ClientMsg {
    Hello { token: String, cols: u16, rows: u16 },
    /// One-shot message from an agent hook; never replaces the attached client.
    Report { token: String, pane: usize, agent: String, state: ReportState },
    Event(Event),
    /// Stop the whole session (all shells).
    Kill,
}

#[derive(Serialize, Deserialize)]
pub enum ServerMsg {
    Update { width: u16, height: u16, full: bool, cells: Vec<(u16, u16, Cell)> },
    /// Base64 payload for an OSC 52 clipboard write.
    Clipboard(String),
    /// The client was detached (prefix+d) or replaced by another client.
    Detached,
    /// The session ended.
    Bye,
}

pub fn send<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(msg).map_err(io::Error::other)?;
    line.push('\n');
    w.write_all(line.as_bytes())?;
    w.flush()
}

/// Where the running server for a session advertises itself.
#[derive(Serialize, Deserialize)]
pub struct ConnInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

pub fn data_dir() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
    };
    let dir = base.context("cannot locate a per-user data directory")?.join("yurlungr");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn conn_path(session: &str) -> Result<PathBuf> {
    Ok(data_dir()?.join(format!("{session}.conn.json")))
}

pub fn layout_path(session: &str) -> Result<PathBuf> {
    Ok(data_dir()?.join(format!("{session}.layout.json")))
}

pub fn read_conn(session: &str) -> Option<ConnInfo> {
    let text = fs::read_to_string(conn_path(session).ok()?).ok()?;
    serde_json::from_str(&text).ok()
}

/// A 128-bit token from randomly keyed hashers; it only guards a loopback socket.
pub fn new_token() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut out = String::new();
    for salt in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u64(salt);
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

pub fn valid_session_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Connect to the running server of a session; returns the socket and its token.
pub fn connect(session: &str) -> Option<(TcpStream, String)> {
    let info = read_conn(session)?;
    let addr = SocketAddr::from(([127, 0, 0, 1], info.port));
    let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    let _ = stream.set_nodelay(true);
    Some((stream, info.token))
}
