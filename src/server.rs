//! The headless session server. It owns every PTY and the layout, so closing
//! the client (or losing an SSH connection) leaves the shells running.

use std::{
    fs,
    io::{self, BufRead, BufReader, Read},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::Duration,
};

use anyhow::{bail, Result};
use crossterm::event::Event;
use ratatui::{
    backend::TestBackend,
    buffer::{Buffer, Cell},
    layout::Rect,
    Terminal as Tui,
};

use crate::{
    app::App,
    proto::{self, ClientMsg, ConnInfo, ReportState, ServerMsg},
    ui,
};

enum Incoming {
    Connected { id: u64, stream: TcpStream, cols: u16, rows: u16 },
    Msg(u64, ClientMsg),
    Gone(u64),
    Report { pane: usize, agent: String, state: ReportState },
}

struct Client {
    id: u64,
    stream: TcpStream,
    /// What the client currently shows; frames are sent as a diff against it.
    prev: Buffer,
}

pub fn run(session: &str, root: PathBuf) -> Result<()> {
    if proto::connect(session).is_some() {
        bail!("session '{session}' is already running");
    }

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let token = proto::new_token();
    let conn_file = proto::conn_path(session)?;
    let info = ConnInfo {
        port: listener.local_addr()?.port(),
        token: token.clone(),
        pid: std::process::id(),
    };
    fs::write(&conn_file, serde_json::to_string(&info)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&conn_file, fs::Permissions::from_mode(0o600))?;
    }

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut next = 0u64;
        for stream in listener.incoming().flatten() {
            next += 1;
            let (id, tx, token) = (next, tx.clone(), token.clone());
            thread::spawn(move || serve_conn(id, stream, token, tx));
        }
    });

    let result = main_loop(session, root, &rx);
    let _ = fs::remove_file(&conn_file);
    result
}

fn serve_conn(id: u64, stream: TcpStream, token: String, tx: Sender<Incoming>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(read_half) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_half);

    // The first line must be a small, valid Hello carrying the session token.
    let mut line = String::new();
    if (&mut reader).take(4096).read_line(&mut line).is_err() {
        return;
    }
    let (cols, rows) = match serde_json::from_str::<ClientMsg>(&line) {
        Ok(ClientMsg::Hello { token: given, cols, rows }) if given == token => (cols, rows),
        Ok(ClientMsg::Report { token: given, pane, agent, state }) => {
            if given == token {
                let _ = tx.send(Incoming::Report { pane, agent, state });
            }
            return;
        }
        _ => return,
    };
    let _ = stream.set_read_timeout(None);
    if tx.send(Incoming::Connected { id, stream, cols, rows }).is_err() {
        return;
    }

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if let Ok(msg) = serde_json::from_str(&line) {
                    if tx.send(Incoming::Msg(id, msg)).is_err() {
                        break;
                    }
                }
            }
        }
    }
    let _ = tx.send(Incoming::Gone(id));
}

fn main_loop(session: &str, root: PathBuf, rx: &mpsc::Receiver<Incoming>) -> Result<()> {
    let mut app = App::new_or_restore(root, proto::layout_path(session)?, session)?;
    let mut tui = Tui::new(TestBackend::new(80, 24))?;
    let mut client: Option<Client> = None;
    let mut killed = false;

    loop {
        let mut msgs = Vec::new();
        match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(m) => msgs.push(m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }

        for msg in msgs {
            match msg {
                Incoming::Connected { id, stream, cols, rows } => {
                    if let Some(mut old) = client.take() {
                        let _ = proto::send(&mut old.stream, &ServerMsg::Detached);
                    }
                    tui.backend_mut().resize(cols.max(1), rows.max(1));
                    client = Some(Client { id, stream, prev: Buffer::empty(Rect::default()) });
                }
                Incoming::Msg(_, ClientMsg::Kill) => killed = true,
                Incoming::Msg(id, ClientMsg::Event(ev))
                    if client.as_ref().is_some_and(|c| c.id == id) =>
                {
                    if let Event::Resize(w, h) = ev {
                        tui.backend_mut().resize(w.max(1), h.max(1));
                        if let Some(c) = &mut client {
                            c.prev = Buffer::empty(Rect::default());
                        }
                    } else if let Err(e) = app.on_event(ev) {
                        app.notice = Some(e.to_string());
                    }
                }
                Incoming::Report { pane, agent, state } => app.report(pane, &agent, state),
                Incoming::Msg(..) => {}
                Incoming::Gone(id) => {
                    if client.as_ref().is_some_and(|c| c.id == id) {
                        client = None;
                    }
                }
            }
        }

        app.reap();
        if killed || app.quit || app.panes.is_empty() {
            if let Some(mut c) = client.take() {
                let _ = proto::send(&mut c.stream, &ServerMsg::Bye);
            }
            app.forget_layout();
            return Ok(());
        }
        app.persist();
        app.poll_config();
        app.track_agents();
        app.poll_overlay();

        if app.detach {
            app.detach = false;
            if let Some(mut c) = client.take() {
                let _ = proto::send(&mut c.stream, &ServerMsg::Detached);
            }
        }
        if let Some(c) = &mut client {
            if push_frame(&mut tui, &mut app, c).is_err() {
                client = None;
            }
        } else {
            app.outbox.clear();
        }
    }
    Ok(())
}

fn push_frame(tui: &mut Tui<TestBackend>, app: &mut App, c: &mut Client) -> io::Result<()> {
    if tui.draw(|f| ui::draw(f, app)).is_err() {
        return Err(io::Error::other("draw failed"));
    }
    let cur = tui.backend().buffer();
    let area = cur.area;
    let full = c.prev.area != area;

    let cells: Vec<(u16, u16, Cell)> = if full {
        let w = area.width.max(1) as usize;
        cur.content
            .iter()
            .enumerate()
            .map(|(i, cell)| ((i % w) as u16, (i / w) as u16, cell.clone()))
            .collect()
    } else {
        c.prev.diff(cur).into_iter().map(|(x, y, cell)| (x, y, cell.clone())).collect()
    };
    if full || !cells.is_empty() {
        proto::send(
            &mut c.stream,
            &ServerMsg::Update { width: area.width, height: area.height, full, cells },
        )?;
    }
    c.prev = cur.clone();

    for payload in app.outbox.drain(..) {
        proto::send(&mut c.stream, &ServerMsg::Clipboard(payload))?;
    }
    Ok(())
}
