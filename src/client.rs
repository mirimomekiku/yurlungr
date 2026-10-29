//! The thin terminal client: paints what the server sends and forwards input.

use std::{
    io::{self, BufRead, BufReader, IsTerminal, Write},
    net::TcpStream,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::{bail, Result};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
};
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{
    proto::{self, ClientMsg, ServerMsg},
    termcaps::ColorLevel,
};

fn spawn_server(session: &str, root: &Path) -> Result<()> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("server")
        .arg("--root")
        .arg(root)
        .arg("--session")
        .arg(session)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: outlive this console.
        cmd.creation_flags(0x0800_0000 | 0x0000_0200);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()?;
    Ok(())
}

fn connect_or_start(session: &str, root: &Path) -> Result<(TcpStream, String)> {
    if let Some(c) = proto::connect(session) {
        return Ok(c);
    }
    spawn_server(session, root)?;
    for _ in 0..80 {
        thread::sleep(Duration::from_millis(100));
        if let Some(c) = proto::connect(session) {
            return Ok(c);
        }
    }
    bail!("the session server did not start")
}

pub fn kill(session: &str) -> Result<()> {
    let Some((mut stream, token)) = proto::connect(session) else {
        bail!("no running session named '{session}'");
    };
    proto::send(&mut stream, &ClientMsg::Hello { token, cols: 80, rows: 24, force: true })?;
    proto::send(&mut stream, &ClientMsg::Kill)?;
    println!("session '{session}' stopped");
    Ok(())
}

/// Fail early, and clearly, when there is no terminal to draw on.
pub fn ensure_interactive() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("yurlungr needs an interactive terminal (stdin and stdout must be a TTY)");
    }
    if std::env::var("TERM").is_ok_and(|t| t == "dumb") {
        bail!("TERM=dumb cannot show yurlungr; use a real terminal emulator");
    }
    Ok(())
}

pub fn attach(session: &str, root: &Path, force: bool) -> Result<()> {
    let (mut stream, token) = connect_or_start(session, root)?;

    let mut terminal = ratatui::init();
    let size = terminal.size()?;
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    // Ctrl+Tab cannot be told apart from Tab without the kitty keyboard
    // protocol. The request is sent unconditionally rather than gated on a
    // support query: that query can time out or go unanswered (SSH, slow
    // terminals) and would silently disable the keys. Terminals without the
    // protocol ignore it; next pane is also bound to prefix + o / Tab.
    let _ = execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let result = run(&mut stream, &mut terminal, token, size.width, size.height, force);
    let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();

    match result {
        Ok(true) => println!("[session ended]"),
        Ok(false) => println!("[detached from '{session}']"),
        Err(e) => return Err(e),
    }
    Ok(())
}

/// Returns Ok(true) if the session ended, Ok(false) if we were detached.
fn run(
    stream: &mut TcpStream,
    terminal: &mut ratatui::DefaultTerminal,
    token: String,
    cols: u16,
    rows: u16,
    force: bool,
) -> Result<bool> {
    let level = ColorLevel::detect();
    proto::send(stream, &ClientMsg::Hello { token, cols, rows, force })?;

    let (tx, rx) = mpsc::channel();
    let reader = BufReader::new(stream.try_clone()?);
    thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Ok(msg) = serde_json::from_str::<ServerMsg>(&line) {
                if tx.send(msg).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send(ServerMsg::Bye);
    });

    let mut local = Buffer::empty(Rect::default());
    let mut dirty = false;

    loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ServerMsg::Update { width, height, full, cells } => {
                    if full || local.area.width != width || local.area.height != height {
                        local = Buffer::empty(Rect::new(0, 0, width, height));
                    }
                    for (x, y, mut cell) in cells {
                        if x < width && y < height {
                            if level != ColorLevel::TrueColor {
                                cell.fg = level.apply(cell.fg);
                                cell.bg = level.apply(cell.bg);
                            }
                            local[(x, y)] = cell;
                        }
                    }
                    dirty = true;
                }
                ServerMsg::Clipboard(b64) => {
                    let mut out = io::stdout();
                    write!(out, "\x1b]52;c;{b64}\x07")?;
                    out.flush()?;
                }
                ServerMsg::Busy => bail!(
                    "this session is already open in another terminal; \
                     detach it there (Ctrl+B d), or run with --force to take it over"
                ),
                ServerMsg::Detached => return Ok(false),
                ServerMsg::Bye => return Ok(true),
            }
        }

        if dirty {
            terminal.draw(|f| {
                if f.area() == local.area {
                    f.buffer_mut().content.clone_from(&local.content);
                }
            })?;
            dirty = false;
        }

        if event::poll(Duration::from_millis(16))? {
            let ev = event::read()?;
            if matches!(&ev, Event::Key(k) if k.kind == KeyEventKind::Release) {
                continue;
            }
            if proto::send(stream, &ClientMsg::Event(ev)).is_err() {
                return Ok(true);
            }
        }
    }
}
