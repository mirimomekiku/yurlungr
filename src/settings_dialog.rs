//! The settings dialog, split into tabs: agents (hooks and login), accounts and
//! usage (Claude Code and Codex), providers (API keys) and general.
//!
//! The accounts tab reads a lot of log files, so it is loaded on a background
//! thread the first time it is opened (or on refresh) and polled from the
//! server tick.

use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    app::App,
    integration::{Action as HookAction, Auth},
    overlay::{centered, frame, keep_visible, reversed},
    settings::Snapshot,
    theme::Theme,
    usage::{self, ClaudeInfo, CodexInfo, RateWindow, Report, Tokens, Window},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Agents,
    Accounts,
    Providers,
    General,
}

const TABS: [(Tab, &str); 4] = [
    (Tab::Agents, "Agents"),
    (Tab::Accounts, "Accounts & usage"),
    (Tab::Providers, "Providers"),
    (Tab::General, "General"),
];

enum Usage {
    NotLoaded,
    Loading(Receiver<Report>),
    Ready(Box<Report>),
    Failed,
}

pub struct SettingsDialog {
    snap: Snapshot,
    tab: Tab,
    selected: usize,
    scroll: usize,
    follow: bool,
    message: Option<String>,
    usage: Usage,
}

impl SettingsDialog {
    pub fn new(snap: Snapshot) -> Self {
        Self {
            snap,
            tab: Tab::Agents,
            selected: 0,
            scroll: 0,
            follow: true,
            message: None,
            usage: Usage::NotLoaded,
        }
    }

    fn start_usage(&mut self) {
        self.start_usage_with(usage::collect);
    }

    /// Run `scan` on a background thread; `poll` delivers its result.
    fn start_usage_with(&mut self, scan: impl FnOnce() -> Report + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(scan());
        });
        self.usage = Usage::Loading(rx);
    }

    /// Pick up the result of the background scan. Called every server tick.
    pub fn poll(&mut self) {
        if let Usage::Loading(rx) = &self.usage {
            match rx.try_recv() {
                Ok(report) => self.usage = Usage::Ready(Box::new(report)),
                Err(TryRecvError::Disconnected) => self.usage = Usage::Failed,
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.scroll = 0;
        self.follow = tab == Tab::Agents;
        if tab == Tab::Accounts && matches!(self.usage, Usage::NotLoaded) {
            self.start_usage();
        }
    }

    fn step_tab(&mut self, delta: isize) {
        let i = TABS.iter().position(|(t, _)| *t == self.tab).unwrap_or(0) as isize;
        let next = (i + delta).rem_euclid(TABS.len() as isize) as usize;
        self.set_tab(TABS[next].0);
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = (self.scroll as isize + delta).max(0) as usize;
        self.follow = false;
    }

    /// Handle a key; returns false when the dialog should close.
    pub fn key(&mut self, app: &mut App, key: KeyEvent) -> bool {
        let last_agent = self.snap.agents.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return false,
            KeyCode::Left | KeyCode::BackTab => self.step_tab(-1),
            KeyCode::Right | KeyCode::Tab => self.step_tab(1),
            KeyCode::Char(c @ '1'..='4') => self.set_tab(TABS[c as usize - '1' as usize].0),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.tab == Tab::Agents {
                    self.selected = self.selected.saturating_sub(1);
                    self.follow = true;
                } else {
                    self.scroll_by(-1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.tab == Tab::Agents {
                    self.selected = (self.selected + 1).min(last_agent);
                    self.follow = true;
                } else {
                    self.scroll_by(1);
                }
            }
            KeyCode::PageUp => self.scroll_by(-8),
            KeyCode::PageDown => self.scroll_by(8),
            KeyCode::Home => {
                self.scroll = 0;
                self.follow = false;
            }
            KeyCode::End => self.scroll_by(isize::MAX / 2),
            KeyCode::Char('r') => {
                self.snap = app.settings_snapshot();
                if self.tab == Tab::Accounts || !matches!(self.usage, Usage::NotLoaded) {
                    self.start_usage();
                }
                self.message = Some("refreshing".into());
            }
            KeyCode::Char(c @ ('i' | 'u')) if self.tab == Tab::Agents => {
                let agent = self.snap.agents[self.selected].agent;
                let action = if c == 'i' { HookAction::Install } else { HookAction::Uninstall };
                self.message = Some(match agent.apply(action) {
                    Ok(msg) => format!("{}: {msg}", agent.label()),
                    Err(e) => format!("{}: {e:#}", agent.label()),
                });
                self.snap = app.settings_snapshot();
            }
            _ => {}
        }
        true
    }
}

// ---- drawing ------------------------------------------------------------------

/// Looks shared by the tabs.
struct Look {
    th: Theme,
}

impl Look {
    fn heading(&self, text: &str) -> Line<'static> {
        Line::from(Span::styled(
            format!(" {text}"),
            Style::new().fg(self.th.accent).add_modifier(Modifier::BOLD),
        ))
    }

    fn sub(&self, text: &str) -> Line<'static> {
        Line::from(Span::styled(
            format!("   {text}"),
            Style::new().fg(self.th.dim).add_modifier(Modifier::BOLD),
        ))
    }

    fn note(&self, text: &str) -> Line<'static> {
        Line::from(Span::styled(format!("   {text}"), Style::new().fg(self.th.dim)))
    }

    fn kv(&self, label: &str, value: impl Into<String>) -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("   {label:<15}"), Style::new().fg(self.th.dim)),
            Span::raw(value.into()),
        ])
    }

    fn kv_spans(&self, label: &str, mut spans: Vec<Span<'static>>) -> Line<'static> {
        let mut line = vec![Span::styled(format!("   {label:<15}"), Style::new().fg(self.th.dim))];
        line.append(&mut spans);
        Line::from(line)
    }

    fn ok(&self, s: impl Into<String>) -> Span<'static> {
        Span::styled(s.into(), Style::new().fg(self.th.working))
    }
    fn warn(&self, s: impl Into<String>) -> Span<'static> {
        Span::styled(s.into(), Style::new().fg(self.th.blocked))
    }
    fn bad(&self, s: impl Into<String>) -> Span<'static> {
        Span::styled(s.into(), Style::new().fg(self.th.danger))
    }
    fn dim(&self, s: impl Into<String>) -> Span<'static> {
        Span::styled(s.into(), Style::new().fg(self.th.dim))
    }
}

fn date_part(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

fn tokens_line(t: &Tokens) -> String {
    format!(
        "in {} · out {} · cache read {} · cache write {}",
        usage::fmt_count(t.input),
        usage::fmt_count(t.output),
        usage::fmt_count(t.cache_read),
        usage::fmt_count(t.cache_write)
    )
}

fn window_line(look: &Look, label: &str, w: &Window) -> Line<'static> {
    look.kv(label, format!("{:>5} msgs   {}", w.messages, tokens_line(&w.tokens)))
}

/// `[████░░░░░░]` and a colour that warms up as the limit nears.
fn bar(percent: f64, th: &Theme) -> (String, Color) {
    let filled = ((percent / 100.0) * 12.0).round().clamp(0.0, 12.0) as usize;
    let color = if percent >= 90.0 {
        th.danger
    } else if percent >= 70.0 {
        th.blocked
    } else {
        th.working
    };
    (format!("[{}{}]", "█".repeat(filled), "░".repeat(12 - filled)), color)
}

fn window_name(minutes: u64) -> String {
    match minutes {
        300 => "5-hour window".to_string(),
        10_080 => "Weekly window".to_string(),
        m => format!("{} window", usage::fmt_span(m as i64 * 60)),
    }
}

fn rate_line(look: &Look, w: &RateWindow, now: i64) -> Line<'static> {
    let (bar, color) = bar(w.used_percent, &look.th);
    let reset = if w.resets_at > now {
        format!("resets in {}", usage::fmt_span(w.resets_at - now))
    } else {
        "has reset since this snapshot, so it is likely lower now".to_string()
    };
    look.kv_spans(
        &window_name(w.window_minutes),
        vec![
            Span::styled(bar, Style::new().fg(color)),
            Span::raw(format!(" {:>3.0}% used   ", w.used_percent)),
            look.dim(reset),
        ],
    )
}

fn claude_lines(look: &Look, c: &ClaudeInfo, now: i64) -> Vec<Line<'static>> {
    let mut l = vec![look.heading("Claude Code")];
    match &c.account {
        None => l.push(look.note("not logged in: no account found in Claude's config")),
        Some(a) => {
            let who = match (&a.email, &a.name) {
                (Some(e), Some(n)) => format!("{e}  ({n})"),
                (Some(e), None) => e.clone(),
                (None, Some(n)) => n.clone(),
                (None, None) => "unknown".to_string(),
            };
            l.push(look.kv("Account", who));
            let plan = [a.plan.as_deref(), a.tier.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            if !plan.is_empty() {
                l.push(look.kv("Plan / tier", plan));
            }
            if let Some(org) = &a.organization {
                let detail = [a.org_type.as_deref(), a.org_role.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");
                l.push(look.kv(
                    "Organization",
                    if detail.is_empty() { org.clone() } else { format!("{org}  ({detail})") },
                ));
            }
            let mut billing = a.billing.clone().unwrap_or_default();
            if let Some(extra) = a.extra_usage {
                if !billing.is_empty() {
                    billing.push_str(" · ");
                }
                billing.push_str(if extra { "extra usage on" } else { "extra usage off" });
            }
            if !billing.is_empty() {
                l.push(look.kv("Billing", billing));
            }
            if let Some(since) = &a.member_since {
                l.push(look.kv("Subscribed", date_part(since)));
            }
            if let Some(ends) = &a.trial_ends {
                l.push(look.kv("Trial ends", date_part(ends)));
            }
            if let Some(exp) = a.token_expires {
                let text = if exp > now {
                    format!("valid for another {}", usage::fmt_span(exp - now))
                } else {
                    format!(
                        "expired {} ago (refreshed the next time Claude Code runs)",
                        usage::fmt_span(now - exp)
                    )
                };
                l.push(look.kv("Login token", text));
            }
        }
    }

    l.push(Line::raw(""));
    l.push(look.sub(&format!(
        "Usage from local transcripts ({} files touched in the last 7 days, {} sessions)",
        c.files_scanned, c.sessions_7d
    )));
    l.push(window_line(look, "Last 5 hours", &c.last_5h));
    l.push(window_line(look, "Last 24 hours", &c.last_24h));
    l.push(window_line(look, "Last 7 days", &c.last_7d));
    for (i, (model, t)) in c.models_7d.iter().take(4).enumerate() {
        l.push(look.kv(
            if i == 0 { "By model" } else { "" },
            format!(
                "{:<24} {} total · out {}",
                clip(model, 24),
                usage::fmt_count(t.total()),
                usage::fmt_count(t.output)
            ),
        ));
    }
    if let Some(all) = &c.all_time {
        let as_of = all.as_of.as_deref().map_or(String::new(), |d| format!(" as of {d}"));
        l.push(look.kv(
            "All time",
            format!(
                "{} msgs · {} sessions · {} tokens{as_of}",
                all.messages,
                all.sessions,
                usage::fmt_count(all.tokens.total())
            ),
        ));
        if let Some(first) = &all.first_session {
            l.push(look.kv("First session", date_part(first)));
        }
    }
    l.push(Line::raw(""));
    l.push(look.sub("Limits"));
    l.push(look.note(
        "Claude Code does not keep your remaining 5-hour or weekly quota on this machine; only",
    ));
    l.push(look.note(
        "Anthropic's servers know it. The tier above sets its size. Run /usage inside Claude Code",
    ));
    l.push(look.note(
        "to see what is left. (yurlungr makes no network calls and never uses your login token.)",
    ));
    l
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}

fn codex_lines(look: &Look, c: &CodexInfo, now: i64) -> Vec<Line<'static>> {
    let mut l = vec![look.heading("Codex CLI")];
    match &c.account {
        None => l.push(look.note("not logged in: no auth file found")),
        Some(a) => {
            let mode = match a.auth_mode.as_deref() {
                Some("chatgpt") => "ChatGPT login",
                Some("apikey") => "API key",
                Some(other) => other,
                None => "unknown login type",
            };
            match (&a.email, &a.name) {
                (Some(e), Some(n)) => l.push(look.kv("Account", format!("{e}  ({n})"))),
                (Some(e), None) => l.push(look.kv("Account", e.clone())),
                _ => l.push(look.kv("Account", "identity not stored (API key login)")),
            }
            l.push(look.kv("Sign-in", mode));
            if let Some(plan) = &a.plan {
                l.push(look.kv("Plan", plan.clone()));
            }
            if let Some(until) = &a.subscription_until {
                l.push(look.kv("Active until", date_part(until)));
            }
        }
    }

    l.push(Line::raw(""));
    match &c.limits {
        None => {
            l.push(look.sub("Rate limits"));
            l.push(
                look.note(
                    "none recorded yet: Codex writes them into its session logs while it runs.",
                ),
            );
        }
        Some(lim) => {
            l.push(look.sub(&format!(
                "Rate limits, as of {} ago (from your most recent Codex session)",
                usage::fmt_span(now - lim.seen_at)
            )));
            for w in [&lim.primary, &lim.secondary].into_iter().flatten() {
                l.push(rate_line(look, w, now));
            }
            if let Some(plan) = &lim.plan {
                l.push(look.kv("Plan", plan.clone()));
            }
            if let Some(credits) = &lim.credits {
                l.push(look.kv("Credits", credits.clone()));
            }
            l.push(look.kv_spans(
                "Limit reached",
                vec![match &lim.reached {
                    Some(which) => look.bad(which.clone()),
                    None => look.ok("no"),
                }],
            ));
        }
    }

    l.push(Line::raw(""));
    l.push(look.sub(&format!(
        "Usage by last activity ({} sessions, {} in the last 7 days)",
        c.sessions, c.sessions_7d
    )));
    l.push(look.kv("Last 24 hours", tokens_line(&c.last_24h)));
    l.push(look.kv("Last 7 days", tokens_line(&c.last_7d)));
    l.push(look.kv(
        "All sessions",
        format!("{} total · {}", usage::fmt_count(c.all.total()), tokens_line(&c.all)),
    ));
    l
}

fn accounts_lines(d: &SettingsDialog, look: &Look, now: i64) -> Vec<Line<'static>> {
    let report = match &d.usage {
        Usage::Ready(r) => r,
        Usage::Failed => {
            return vec![look.note("could not read the local logs; press r to try again")]
        }
        Usage::NotLoaded | Usage::Loading(_) => {
            return vec![look.note("reading local logs…")];
        }
    };
    let mut l = Vec::new();
    match &report.claude {
        Some(c) => l.extend(claude_lines(look, c, now)),
        None => {
            l.push(look.heading("Claude Code"));
            l.push(look.note("not installed here: no Claude data folder found"));
        }
    }
    l.push(Line::raw(""));
    match &report.codex {
        Some(c) => l.extend(codex_lines(look, c, now)),
        None => {
            l.push(look.heading("Codex CLI"));
            l.push(look.note("not installed here: no Codex data folder found"));
        }
    }

    l.push(Line::raw(""));
    l.push(look.heading("Other agents"));
    for row in d.snap.agents.iter().filter(|r| !matches!(r.agent.name(), "claude" | "codex")) {
        let login = match &row.auth {
            Auth::LoggedIn(_) => look.ok("login file found"),
            Auth::NotFound => look.dim("no login found"),
            Auth::Unknown => look.dim("login location unknown"),
        };
        l.push(look.kv_spans(
            row.agent.label(),
            vec![login, look.dim("   · no usage data is kept locally")],
        ));
    }
    l.push(Line::raw(""));
    l.push(look.note(&format!(
        "Scanned {} ago. Everything shown is read from files already on this machine.",
        usage::fmt_span(now - report.at)
    )));
    l
}

fn agents_lines(d: &SettingsDialog, look: &Look) -> (Vec<Line<'static>>, usize) {
    let mut lines = vec![look.heading("Coding agents: status hooks and login")];
    let mut selected_line = 0;
    for (i, row) in d.snap.agents.iter().enumerate() {
        let on = i == d.selected;
        if on {
            selected_line = lines.len();
        }
        let binary =
            if row.binary.is_some() { look.ok("installed    ") } else { look.dim("not on PATH  ") };
        let hooks = match &row.hooks {
            Ok(true) => look.ok("hooks installed    "),
            Ok(false) => look.warn("hooks not installed"),
            Err(_) => look.bad("hooks: error       "),
        };
        let login = match &row.auth {
            Auth::LoggedIn(_) => look.ok("  login found"),
            Auth::NotFound => look.dim("  no login found"),
            Auth::Unknown => look.dim("  login: n/a"),
        };
        let mut name =
            Span::raw(format!(" {} {:<15}", if on { ">" } else { " " }, row.agent.label()));
        if on {
            name = name.style(reversed());
        }
        lines.push(Line::from(vec![name, Span::raw(" "), binary, Span::raw(" "), hooks, login]));
        if on {
            if let Some(p) = &row.binary {
                lines.push(look.note(&format!("   binary: {}", p.display())));
            }
            if let Some(p) = &row.config {
                lines.push(look.note(&format!("   config: {}", p.display())));
            }
            if let Err(e) = &row.hooks {
                lines.push(Line::from(look.bad(format!("       {e}"))));
            }
            if let Auth::LoggedIn(p) = &row.auth {
                lines.push(look.note(&format!("   login file: {}", p.display())));
            }
        }
    }
    (lines, selected_line)
}

fn providers_lines(d: &SettingsDialog, look: &Look) -> Vec<Line<'static>> {
    let mut l = vec![look.heading("LLM providers: API keys in the session's environment")];
    l.push(Line::raw(""));
    for p in &d.snap.providers {
        let status = match p.set_var {
            Some(var) => look.ok(format!("{var} is set")),
            None => look.dim(format!("not set ({})", p.vars.join(" / "))),
        };
        l.push(Line::from(vec![Span::raw(format!("   {:<18}", p.name)), status]));
        l.push(look.note(&format!("   usage, limits & billing: {}", p.dashboard)));
    }
    l.push(Line::raw(""));
    l.push(look.note(
        "API-key spend and rate limits live on each provider's dashboard above; yurlungr does not",
    ));
    l.push(look.note(
        "call any provider and never shows or sends a key. Variables are read from the environment",
    ));
    l.push(look.note("the session server started with."));
    l
}

fn general_lines(d: &SettingsDialog, look: &Look) -> Vec<Line<'static>> {
    let mut l = vec![look.heading("yurlungr")];
    for (label, value) in &d.snap.info {
        if label == "config warning" {
            l.push(look.kv_spans("config warning", vec![look.warn(value.clone())]));
        } else {
            l.push(look.kv(label, value.clone()));
        }
    }
    l
}

pub fn draw(f: &mut Frame, d: &mut SettingsDialog, th: &Theme) {
    let area = centered(f.area(), 112, 44);
    let inner = frame(f, area, "Settings", th);
    let [tabs, body, footer] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1), Constraint::Length(2)])
            .areas(inner);
    let look = Look { th: *th };
    let now = usage::now_secs();

    // Tab bar.
    let mut spans = vec![Span::raw(" ")];
    for (i, (tab, name)) in TABS.iter().enumerate() {
        let style = if *tab == d.tab {
            Style::new().fg(th.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::new().fg(th.dim)
        };
        spans.push(Span::styled(format!(" {} {name} ", i + 1), style));
        spans.push(Span::raw("  "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs);

    let (lines, selected_line) = match d.tab {
        Tab::Agents => agents_lines(d, &look),
        Tab::Accounts => (accounts_lines(d, &look, now), 0),
        Tab::Providers => (providers_lines(d, &look), 0),
        Tab::General => (general_lines(d, &look), 0),
    };
    let height = body.height as usize;
    if d.follow {
        keep_visible(&mut d.scroll, selected_line, height);
        keep_visible(&mut d.scroll, (selected_line + 4).min(lines.len().saturating_sub(1)), height);
    }
    d.scroll = d.scroll.min(lines.len().saturating_sub(height));
    f.render_widget(Paragraph::new(lines).scroll((d.scroll as u16, 0)), body);

    let status = match &d.message {
        Some(m) => Span::styled(format!(" {m}"), Style::new().fg(th.marker)),
        None => look.dim(
            " Read from files already on this machine. Login tokens and API keys are never read or shown.",
        ),
    };
    let keys = match d.tab {
        Tab::Agents => {
            " ←/→ tab · j/k select · i install hooks · u uninstall · r refresh · Esc close"
        }
        Tab::Accounts => " ←/→ tab · j/k or PgUp/PgDn scroll · r rescan · Esc close",
        _ => " ←/→ tab · j/k or PgUp/PgDn scroll · r refresh · Esc close",
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(status),
            Line::from(Span::styled(keys, Style::new().fg(th.dim))),
        ]),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_and_warm_up() {
        let th = crate::theme::builtin("default").unwrap();
        assert_eq!(bar(0.0, &th).0, "[░░░░░░░░░░░░]");
        assert_eq!(bar(100.0, &th).0, "[████████████]");
        assert_eq!(bar(50.0, &th).0.matches('█').count(), 6);
        assert_eq!(bar(15.0, &th).1, th.working);
        assert_eq!(bar(75.0, &th).1, th.blocked);
        assert_eq!(bar(95.0, &th).1, th.danger);
        assert_eq!(bar(500.0, &th).0.matches('█').count(), 12, "never overflows");
    }

    #[test]
    fn rate_windows_get_readable_names() {
        assert_eq!(window_name(300), "5-hour window");
        assert_eq!(window_name(10_080), "Weekly window");
        assert_eq!(window_name(60), "1h 00m window");
    }

    fn dialog() -> SettingsDialog {
        SettingsDialog::new(crate::settings::gather("t", "default", 0, &[]))
    }

    #[test]
    fn tabs_cycle_in_both_directions() {
        // Starting from tabs other than Accounts, so no background scan starts.
        let mut d = dialog();
        assert_eq!(d.tab, Tab::Agents);
        d.step_tab(-1);
        assert_eq!(d.tab, Tab::General, "wraps backwards");
        d.step_tab(1);
        assert_eq!(d.tab, Tab::Agents, "wraps forwards");
        d.set_tab(Tab::Providers);
        d.scroll = 7;
        d.set_tab(Tab::General);
        assert_eq!(d.scroll, 0, "each tab starts at the top");
        assert!(
            matches!(d.usage, Usage::NotLoaded),
            "the log scan waits until the Accounts tab opens"
        );
    }

    fn fake_report() -> Report {
        Report { claude: None, codex: None, at: 0 }
    }

    #[test]
    fn the_scan_runs_in_the_background_and_is_picked_up_by_poll() {
        let mut d = dialog();
        d.start_usage_with(fake_report);
        assert!(matches!(d.usage, Usage::Loading(_)), "starts loading without blocking");
        for _ in 0..200 {
            d.poll();
            if matches!(d.usage, Usage::Ready(_)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(matches!(d.usage, Usage::Ready(_)), "the result arrives");
    }

    #[test]
    fn a_scan_that_dies_is_reported_not_hung() {
        let mut d = dialog();
        d.start_usage_with(|| panic!("simulated failure"));
        for _ in 0..200 {
            d.poll();
            if matches!(d.usage, Usage::Failed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(matches!(d.usage, Usage::Failed));
    }

    #[test]
    fn accounts_tab_shows_a_placeholder_until_the_scan_finishes() {
        let mut d = dialog();
        let look = Look { th: crate::theme::builtin("default").unwrap() };
        assert_eq!(accounts_lines(&d, &look, 0).len(), 1, "one 'reading…' line");
        d.usage = Usage::Ready(Box::new(fake_report()));
        let text: String = accounts_lines(&d, &look, 0)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("Claude Code")
                && text.contains("Codex CLI")
                && text.contains("Other agents"),
            "{text}"
        );
        assert!(text.contains("not installed here"), "missing tools are explained");
    }

    #[test]
    fn dates_and_clipping() {
        assert_eq!(date_part("2026-11-02T07:07:23+00:00"), "2026-11-02");
        assert_eq!(date_part("short"), "short");
        assert_eq!(clip("claude-opus-5-5", 13), "claude-opus-…");
        assert_eq!(clip("abc", 13), "abc");
    }
}
