use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

#[cfg(unix)]
use crate::agents;
use crate::proto::ReportState;

type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

const SCROLLBACK: usize = 5000;

/// How often a pane's foreground process is inspected.
const PROBE_EVERY: Duration = Duration::from_millis(1500);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What an agent (or any program) running in a pane appears to be doing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Working,
    Blocked,
    Idle,
}

const PROMPTS: &[&str] = &[
    "(y/n)",
    "[y/n]",
    "(yes/no)",
    "[yes/no]",
    "do you want to",
    "allow this",
    "proceed?",
    "press enter to continue",
    "1. yes",
    "approve",
];

/// A hook-reported state older than this, with no output since, is distrusted
/// (the agent may have been killed without saying goodbye).
const REPORT_STALE: Duration = Duration::from_secs(60);

struct AgentReport {
    agent: String,
    state: ReportState,
    at: Instant,
}

/// Heuristic: the tail of the screen looks like a question waiting for the user.
fn looks_blocked(screen: &vt100::Screen) -> bool {
    let (rows, cols) = screen.size();
    let tail: String = screen
        .rows(0, cols)
        .skip(rows.saturating_sub(15) as usize)
        .collect::<Vec<_>>()
        .join(
            "
",
        )
        .to_lowercase();
    PROMPTS.iter().any(|p| tail.contains(p))
}

/// Answers the terminal queries that shells (notably ConPTY) block on.
struct Replies {
    writer: SharedWriter,
}

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied();
        let reply = match (c, first, i1, i2) {
            ('n', Some(6), None, None) => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            ('n', Some(5), None, None) => "\x1b[0n".to_string(),
            ('c', None | Some(0), None, None) => "\x1b[?1;2c".to_string(),
            _ => return,
        };
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(reply.as_bytes());
            let _ = w.flush();
        }
    }
}

pub struct Terminal {
    parser: Arc<Mutex<vt100::Parser<Replies>>>,
    writer: SharedWriter,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    last_output: Arc<AtomicU64>,
    last_input: AtomicU64,
    report: Option<AgentReport>,
    /// The agent found by looking at the foreground process, with no hooks involved.
    detected: Option<&'static str>,
    last_probe: Instant,
    size: (u16, u16),
}

fn shell() -> String {
    if cfg!(windows) {
        "powershell.exe".to_string()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

impl Terminal {
    pub fn spawn(cwd: &Path, rows: u16, cols: u16, env: &[(String, String)]) -> Result<Self> {
        let (rows, cols) = (rows.max(1), cols.max(1));
        let pair =
            native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;

        let mut cmd = CommandBuilder::new(shell());
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (key, value) in env {
            cmd.env(key, value);
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer()?));
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK,
            Replies { writer: writer.clone() },
        )));
        let exited = Arc::new(AtomicBool::new(false));
        let last_output = Arc::new(AtomicU64::new(0));

        {
            let parser = parser.clone();
            let exited = exited.clone();
            let last_output = last_output.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Ok(mut p) = parser.lock() {
                                p.process(&buf[..n]);
                            }
                            last_output.store(now_ms(), Ordering::SeqCst);
                        }
                    }
                }
                exited.store(true, Ordering::SeqCst);
            });
        }

        Ok(Self {
            parser,
            writer,
            master: pair.master,
            child,
            exited,
            last_output,
            last_input: AtomicU64::new(0),
            report: None,
            detected: None,
            last_probe: Instant::now(),
            size: (rows, cols),
        })
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 || (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_size(rows, cols);
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.last_input.store(now_ms(), Ordering::SeqCst);
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }

    /// Record a state reported by an agent hook running in this pane.
    pub fn set_report(&mut self, agent: String, state: ReportState) {
        self.report = match state {
            ReportState::Release => None,
            _ => Some(AgentReport { agent, state, at: Instant::now() }),
        };
    }

    /// The agent in this pane: the one its hooks reported, else the one detected
    /// from the foreground process.
    pub fn agent(&self) -> Option<&str> {
        self.report.as_ref().map(|r| r.agent.as_str()).or(self.detected)
    }

    /// Look at what is running in the foreground of this pane. Cheap enough to
    /// call every tick: it only does work once per `PROBE_EVERY`.
    pub fn probe_agent(&mut self) {
        if self.last_probe.elapsed() < PROBE_EVERY {
            return;
        }
        self.last_probe = Instant::now();
        // Asking a pty for its foreground process is Unix-only. Elsewhere agents
        // are still listed when their hooks report in.
        #[cfg(unix)]
        {
            self.detected = self
                .master
                .process_group_leader()
                .and_then(|pid| agents::command_line(i64::from(pid)))
                .and_then(|args| agents::detect(&args));
        }
    }

    pub fn status(&self) -> Status {
        let out = self.last_output.load(Ordering::SeqCst);
        let since = now_ms().saturating_sub(out);

        // Hooks know better than the screen.
        if let Some(r) = &self.report {
            match r.state {
                ReportState::Blocked => return Status::Blocked,
                ReportState::Idle => return Status::Idle,
                ReportState::Working => {
                    let silent = since > REPORT_STALE.as_millis() as u64;
                    return if silent && r.at.elapsed() > REPORT_STALE {
                        Status::Idle
                    } else {
                        Status::Working
                    };
                }
                ReportState::Release => {}
            }
        }
        // Output right after a key press is just the shell echoing it.
        let echo = out >= self.last_input.load(Ordering::SeqCst)
            && out.saturating_sub(self.last_input.load(Ordering::SeqCst)) < 400;
        if since > 800 && self.with_screen(looks_blocked) {
            Status::Blocked
        } else if since < 1500 && !echo {
            Status::Working
        } else {
            Status::Idle
        }
    }

    pub fn mouse_mode(&self) -> (vt100::MouseProtocolMode, vt100::MouseProtocolEncoding) {
        self.with_screen(|s| (s.mouse_protocol_mode(), s.mouse_protocol_encoding()))
    }

    pub fn with_screen<R>(&self, f: impl FnOnce(&vt100::Screen) -> R) -> R {
        let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        f(p.screen())
    }

    /// Positive `delta` scrolls back into history, negative towards the live screen.
    pub fn scroll(&self, delta: isize) {
        if let Ok(mut p) = self.parser.lock() {
            let screen = p.screen_mut();
            let cur = screen.scrollback() as isize;
            screen.set_scrollback((cur + delta).max(0) as usize);
        }
    }

    pub fn reset_scroll(&self) {
        self.scroll(isize::MIN / 2);
    }

    pub fn is_alive(&mut self) -> bool {
        !self.exited.load(Ordering::SeqCst) && !matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}
