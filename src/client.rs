//! The thin terminal client: paints what the server sends and forwards input.

use std::{
    io::{self, BufRead, BufReader, Write},
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
    terminal::supports_keyboard_enhancement,
};
use ratatui::{buffer::Buffer, layout::Rect};

use crate::proto::{self, ClientMsg, ServerMsg};

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
    proto::send(&mut stream, &ClientMsg::Hello { token, cols: 80, rows: 24 })?;
    proto::send(&mut stream, &ClientMsg::Kill)?;
    println!("session '{session}' stopped");
    Ok(())
}

pub fn attach(session: &str, root: &Path) -> Result<()> {
    let (mut stream, token) = connect_or_start(session, root)?;

    let mut terminal = ratatui::init();
    let size = terminal.size()?;
    let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    // Ctrl+<digit> (jump to pane) cannot be told apart from the bare digit
    // without the kitty keyboard protocol. Terminals that lack it keep
    // working: the same actions are bound to prefix + digit.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    let result = run(&mut stream, &mut terminal, token, size.width, size.height);
    if enhanced {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
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
) -> Result<bool> {
    proto::send(stream, &ClientMsg::Hello { token, cols, rows })?;

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
                    for (x, y, cell) in cells {
                        if x < width && y < height {
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
