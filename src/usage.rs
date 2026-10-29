//! Account and usage information for Claude Code and Codex, read from the
//! files those tools already keep on disk.
//!
//! Everything here is local: nothing is sent anywhere and no login token is
//! ever used. The structs that read credential files declare only the
//! non-secret fields they need, so access and refresh tokens are never even
//! deserialised, let alone shown.
//!
//! What each tool lets us see:
//! * Codex records its plan rate-limit status (percent used of the 5-hour and
//!   weekly windows, and when they reset) in every session log, so that is real
//!   limit status, as of the last session.
//! * Claude Code keeps account details, the plan and rate-limit tier, and the
//!   token usage of every message, but not how much quota is left; that is only
//!   available from Anthropic's servers (`/usage` inside Claude Code).

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::Value;

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

// ---- small helpers ------------------------------------------------------------

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Days since 1970-01-01 for a calendar date (the inverse of the algorithm in `git_graph`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since the epoch for `2026-10-25T14:07:38.380Z` or `...+00:00`.
pub fn parse_iso(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        std::str::from_utf8(b.get(from..from + len)?).ok()?.parse().ok()
    };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') {
        return None;
    }
    let days = days_from_civil(num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let secs = days * DAY + num(11, 2)? * HOUR + num(14, 2)? * 60 + num(17, 2)?;

    // Skip fractional seconds, then read the zone.
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
    }
    let offset = match b.get(i) {
        Some(b'+' | b'-') => {
            let sign = if b[i] == b'-' { -1 } else { 1 };
            sign * (num(i + 1, 2)? * HOUR + num(i + 4, 2).unwrap_or(0) * 60)
        }
        _ => 0,
    };
    Some(secs - offset)
}

/// `1234` -> `1.2K`, `5_600_000` -> `5.6M`.
pub fn fmt_count(n: u64) -> String {
    let f = n as f64;
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", f / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", f / 1e6),
        _ => format!("{:.2}B", f / 1e9),
    }
}

/// `7500` -> `2h 05m`, `200000` -> `2d 7h`.
pub fn fmt_span(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m) = (secs / DAY, secs % DAY / HOUR, secs % HOUR / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m:02}m"),
        _ => format!("{d}d {h}h"),
    }
}

// ---- token counts -------------------------------------------------------------

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
}

/// Longest log line that is read; longer ones are skipped instead of buffered.
const MAX_LINE: u64 = 16 * 1024 * 1024;

/// The lines of a log file. Unlike `BufRead::lines` it survives invalid UTF-8
/// (the bytes are replaced, the file is not abandoned) and never holds an
/// enormous line in memory.
struct Lines<R> {
    reader: BufReader<R>,
    buf: Vec<u8>,
}

impl<R: Read> Lines<R> {
    fn new(source: R) -> Self {
        Self { reader: BufReader::new(source), buf: Vec::new() }
    }
}

impl<R: Read> Iterator for Lines<R> {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        loop {
            self.buf.clear();
            let n = self.reader.by_ref().take(MAX_LINE).read_until(b'\n', &mut self.buf).ok()?;
            if n == 0 {
                return None;
            }
            if self.buf.last() == Some(&b'\n') || (n as u64) < MAX_LINE {
                return Some(String::from_utf8_lossy(&self.buf).into_owned());
            }
            // Overlong: drop the rest of this line, then carry on with the next.
            loop {
                self.buf.clear();
                let n = self.reader.by_ref().take(1 << 20).read_until(b'\n', &mut self.buf).ok()?;
                if n == 0 {
                    return None;
                }
                if self.buf.last() == Some(&b'\n') {
                    break;
                }
            }
        }
    }
}

fn field(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Every `*.jsonl` file under `dir` modified at or after `since` (epoch seconds).
fn jsonl_files(dir: &Path, since: i64, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if depth < 6 {
                jsonl_files(&path, since, out, depth + 1);
            }
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            if modified >= since {
                out.push(path);
            }
        }
    }
}

// ---- Claude Code --------------------------------------------------------------

#[derive(Default, Debug)]
pub struct ClaudeAccount {
    pub email: Option<String>,
    pub name: Option<String>,
    pub organization: Option<String>,
    pub org_type: Option<String>,
    pub org_role: Option<String>,
    /// Subscription, e.g. `max` or `pro`.
    pub plan: Option<String>,
    /// Rate-limit tier, e.g. `default_claude_max_5x`.
    pub tier: Option<String>,
    pub billing: Option<String>,
    pub extra_usage: Option<bool>,
    pub trial_ends: Option<String>,
    pub member_since: Option<String>,
    /// When the current login token expires (epoch seconds).
    pub token_expires: Option<i64>,
}

#[derive(Deserialize, Default)]
struct ClaudeJson {
    #[serde(rename = "oauthAccount")]
    account: Option<OauthAccount>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OauthAccount {
    #[serde(rename = "emailAddress")]
    email: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "fullName")]
    full_name: Option<String>,
    #[serde(rename = "organizationName")]
    organization: Option<String>,
    #[serde(rename = "organizationType")]
    org_type: Option<String>,
    #[serde(rename = "organizationRole")]
    org_role: Option<String>,
    #[serde(rename = "organizationRateLimitTier")]
    org_tier: Option<String>,
    #[serde(rename = "userRateLimitTier")]
    user_tier: Option<String>,
    #[serde(rename = "billingType")]
    billing: Option<String>,
    #[serde(rename = "hasExtraUsageEnabled")]
    extra_usage: Option<bool>,
    #[serde(rename = "claudeCodeTrialEndsAt")]
    trial_ends: Option<String>,
    #[serde(rename = "subscriptionCreatedAt")]
    subscription_created: Option<String>,
}

/// Only the non-secret fields; `accessToken` and `refreshToken` are not declared.
#[derive(Deserialize, Default)]
struct ClaudeCreds {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<ClaudeOauth>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaudeOauth {
    #[serde(rename = "subscriptionType")]
    subscription: Option<String>,
    #[serde(rename = "rateLimitTier")]
    tier: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StatsCache {
    #[serde(rename = "lastComputedDate")]
    last_computed: Option<String>,
    #[serde(rename = "totalSessions")]
    sessions: u64,
    #[serde(rename = "totalMessages")]
    messages: u64,
    #[serde(rename = "firstSessionDate")]
    first_session: Option<String>,
    #[serde(rename = "modelUsage")]
    models: HashMap<String, StatsModel>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StatsModel {
    #[serde(rename = "inputTokens")]
    input: u64,
    #[serde(rename = "outputTokens")]
    output: u64,
    #[serde(rename = "cacheReadInputTokens")]
    cache_read: u64,
    #[serde(rename = "cacheCreationInputTokens")]
    cache_write: u64,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub tokens: Tokens,
    pub messages: u32,
}

/// Usage totalled over Claude Code's own cache, which lags behind (see `as_of`).
#[derive(Debug)]
pub struct AllTime {
    pub sessions: u64,
    pub messages: u64,
    pub tokens: Tokens,
    pub first_session: Option<String>,
    pub as_of: Option<String>,
}

#[derive(Default, Debug)]
pub struct ClaudeInfo {
    pub account: Option<ClaudeAccount>,
    pub last_5h: Window,
    pub last_24h: Window,
    pub last_7d: Window,
    /// Token totals per model over the last 7 days, biggest first.
    pub models_7d: Vec<(String, Tokens)>,
    pub sessions_7d: usize,
    pub files_scanned: usize,
    pub all_time: Option<AllTime>,
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

fn claude_account(dir: &Path, json: &Path) -> Option<ClaudeAccount> {
    let parsed: ClaudeJson = fs::read_to_string(json)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let creds: ClaudeCreds = fs::read_to_string(dir.join(".credentials.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if parsed.account.is_none() && creds.oauth.is_none() {
        return None;
    }
    let a = parsed.account.unwrap_or_default();
    let o = creds.oauth.unwrap_or_default();
    Some(ClaudeAccount {
        email: nonempty(a.email),
        name: nonempty(a.full_name).or(nonempty(a.display_name)),
        organization: nonempty(a.organization),
        org_type: nonempty(a.org_type),
        org_role: nonempty(a.org_role),
        plan: nonempty(o.subscription),
        tier: nonempty(o.tier).or(nonempty(a.user_tier)).or(nonempty(a.org_tier)),
        billing: nonempty(a.billing),
        extra_usage: a.extra_usage,
        trial_ends: nonempty(a.trial_ends),
        member_since: nonempty(a.subscription_created),
        token_expires: o.expires_at.map(|ms| ms / 1000),
    })
}

/// One assistant message's usage, kept once per message id (Claude Code writes
/// the same message several times as it streams).
struct Seen {
    at: i64,
    model: String,
    tokens: Tokens,
    session: String,
}

fn scan_claude(projects: &Path, now: i64) -> ClaudeInfo {
    let since = now - 7 * DAY;
    let mut files = Vec::new();
    jsonl_files(projects, since, &mut files, 0);

    let mut seen: HashMap<String, Seen> = HashMap::new();
    for file in &files {
        let Ok(handle) = fs::File::open(file) else { continue };
        let session =
            file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        for line in Lines::new(handle) {
            // Cheap filter: most lines are not assistant messages and some are huge.
            if !line.contains("\"assistant\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("type").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(at) = v.get("timestamp").and_then(Value::as_str).and_then(parse_iso) else {
                continue;
            };
            if at < since {
                continue;
            }
            let message = &v["message"];
            let usage = &message["usage"];
            let (Some(id), true) = (message.get("id").and_then(Value::as_str), usage.is_object())
            else {
                continue;
            };
            let model = message.get("model").and_then(Value::as_str).unwrap_or("unknown");
            if model.starts_with('<') {
                continue; // "<synthetic>" placeholders carry no usage
            }
            let tokens = Tokens {
                input: field(usage, "input_tokens"),
                output: field(usage, "output_tokens"),
                cache_read: field(usage, "cache_read_input_tokens"),
                cache_write: field(usage, "cache_creation_input_tokens"),
            };
            let candidate = Seen { at, model: model.to_string(), tokens, session: session.clone() };
            match seen.get_mut(id) {
                Some(existing) if existing.tokens.total() >= candidate.tokens.total() => {}
                Some(existing) => *existing = candidate,
                None => {
                    seen.insert(id.to_string(), candidate);
                }
            }
        }
    }

    let mut info = ClaudeInfo { files_scanned: files.len(), ..Default::default() };
    let mut per_model: HashMap<&str, Tokens> = HashMap::new();
    let mut sessions: HashSet<&str> = HashSet::new();
    for m in seen.values() {
        for (limit, window) in [
            (now - 5 * HOUR, &mut info.last_5h),
            (now - DAY, &mut info.last_24h),
            (now - 7 * DAY, &mut info.last_7d),
        ] {
            if m.at >= limit {
                window.tokens.add(&m.tokens);
                window.messages += 1;
            }
        }
        per_model.entry(m.model.as_str()).or_default().add(&m.tokens);
        sessions.insert(m.session.as_str());
    }
    info.sessions_7d = sessions.len();
    let mut models: Vec<(String, Tokens)> =
        per_model.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    models.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then_with(|| a.0.cmp(&b.0)));
    info.models_7d = models;
    info
}

fn claude_all_time(dir: &Path) -> Option<AllTime> {
    let cache: StatsCache =
        serde_json::from_str(&fs::read_to_string(dir.join("stats-cache.json")).ok()?).ok()?;
    let mut tokens = Tokens::default();
    for m in cache.models.values() {
        tokens.add(&Tokens {
            input: m.input,
            output: m.output,
            cache_read: m.cache_read,
            cache_write: m.cache_write,
        });
    }
    Some(AllTime {
        sessions: cache.sessions,
        messages: cache.messages,
        tokens,
        first_session: cache.first_session,
        as_of: cache.last_computed,
    })
}

pub fn claude(dir: &Path, json: &Path, now: i64) -> ClaudeInfo {
    let mut info = scan_claude(&dir.join("projects"), now);
    info.account = claude_account(dir, json);
    info.all_time = claude_all_time(dir);
    info
}

// ---- Codex --------------------------------------------------------------------

#[derive(Default, Debug)]
pub struct CodexAccount {
    pub email: Option<String>,
    pub name: Option<String>,
    pub plan: Option<String>,
    /// `chatgpt` (a ChatGPT login) or `apikey`.
    pub auth_mode: Option<String>,
    pub subscription_until: Option<String>,
}

/// Only what is needed to read the account out of the login token's claims.
#[derive(Deserialize, Default)]
#[serde(default)]
struct CodexAuth {
    auth_mode: Option<String>,
    tokens: Option<CodexTokens>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CodexTokens {
    id_token: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct IdClaims {
    email: Option<String>,
    name: Option<String>,
    #[serde(rename = "https://api.openai.com/auth")]
    auth: Option<AuthClaims>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthClaims {
    chatgpt_plan_type: Option<String>,
    chatgpt_subscription_active_until: Option<String>,
}

fn codex_account(dir: &Path) -> Option<CodexAccount> {
    let auth: CodexAuth =
        serde_json::from_str(&fs::read_to_string(dir.join("auth.json")).ok()?).ok()?;
    let mut account = CodexAccount { auth_mode: nonempty(auth.auth_mode), ..Default::default() };
    // The id token is a JWT; its middle part is a plain base64 JSON object of claims.
    let claims: IdClaims = auth
        .tokens
        .and_then(|t| t.id_token)
        .and_then(|jwt| {
            let payload = jwt.split('.').nth(1)?.to_string();
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?)
                .ok()
        })
        .unwrap_or_default();
    account.email = nonempty(claims.email);
    account.name = nonempty(claims.name);
    if let Some(a) = claims.auth {
        account.plan = nonempty(a.chatgpt_plan_type);
        account.subscription_until = nonempty(a.chatgpt_subscription_active_until);
    }
    Some(account)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateWindow {
    pub used_percent: f64,
    pub window_minutes: u64,
    /// When the window resets (epoch seconds).
    pub resets_at: i64,
}

#[derive(Debug)]
pub struct CodexLimits {
    pub plan: Option<String>,
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
    pub credits: Option<String>,
    /// Which limit was hit, if one was.
    pub reached: Option<String>,
    /// When Codex last reported this (epoch seconds).
    pub seen_at: i64,
}

#[derive(Default, Debug)]
pub struct CodexInfo {
    pub account: Option<CodexAccount>,
    pub limits: Option<CodexLimits>,
    pub last_24h: Tokens,
    pub last_7d: Tokens,
    pub all: Tokens,
    pub sessions_7d: usize,
    pub sessions: usize,
}

fn rate_window(v: &Value) -> Option<RateWindow> {
    Some(RateWindow {
        used_percent: v.get("used_percent")?.as_f64()?,
        window_minutes: v.get("window_minutes").and_then(Value::as_u64).unwrap_or(0),
        resets_at: v.get("resets_at")?.as_i64()?,
    })
}

fn codex_tokens(usage: &Value) -> Tokens {
    let cached = field(usage, "cached_input_tokens");
    Tokens {
        // OpenAI counts cached tokens inside `input_tokens`; keep them apart.
        input: field(usage, "input_tokens").saturating_sub(cached),
        output: field(usage, "output_tokens"),
        cache_read: cached,
        cache_write: field(usage, "cache_write_input_tokens"),
    }
}

fn scan_codex(sessions_dir: &Path, now: i64) -> CodexInfo {
    let mut files = Vec::new();
    jsonl_files(sessions_dir, 0, &mut files, 0);
    let mut info = CodexInfo { sessions: files.len(), ..Default::default() };
    let mut newest: Option<(i64, Value)> = None;

    for file in &files {
        let modified = fs::metadata(file)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        let Ok(handle) = fs::File::open(file) else { continue };
        // Each token_count event carries the session's running total, so the
        // last one is the session's total.
        let mut last_total = Tokens::default();
        for line in Lines::new(handle) {
            if !line.contains("\"token_count\"") {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let payload = &v["payload"];
            if payload.get("type").and_then(Value::as_str) != Some("token_count") {
                continue;
            }
            if let Some(total) = payload.get("info").and_then(|i| i.get("total_token_usage")) {
                last_total = codex_tokens(total);
            }
            if let Some(limits) = payload.get("rate_limits").filter(|l| l.is_object()) {
                let at = v
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_iso)
                    .unwrap_or(modified);
                if newest.as_ref().is_none_or(|(t, _)| at >= *t) {
                    newest = Some((at, limits.clone()));
                }
            }
        }
        info.all.add(&last_total);
        if modified >= now - DAY {
            info.last_24h.add(&last_total);
        }
        if modified >= now - 7 * DAY {
            info.last_7d.add(&last_total);
            info.sessions_7d += 1;
        }
    }

    if let Some((seen_at, l)) = newest {
        let credits = l.get("credits").map(|c| {
            if c.get("unlimited").and_then(Value::as_bool) == Some(true) {
                "unlimited".to_string()
            } else if c.get("has_credits").and_then(Value::as_bool) == Some(true) {
                format!("balance {}", c.get("balance").and_then(Value::as_str).unwrap_or("?"))
            } else {
                "none".to_string()
            }
        });
        info.limits = Some(CodexLimits {
            plan: l.get("plan_type").and_then(Value::as_str).map(String::from),
            primary: l.get("primary").and_then(rate_window),
            secondary: l.get("secondary").and_then(rate_window),
            credits,
            reached: l.get("rate_limit_reached_type").and_then(Value::as_str).map(String::from),
            seen_at,
        });
    }
    info
}

pub fn codex(dir: &Path, now: i64) -> CodexInfo {
    let mut info = scan_codex(&dir.join("sessions"), now);
    info.account = codex_account(dir);
    info
}

// ---- both ---------------------------------------------------------------------

pub struct Report {
    /// `None` when Claude Code has no data folder here.
    pub claude: Option<ClaudeInfo>,
    pub codex: Option<CodexInfo>,
    pub at: i64,
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Gather everything for the usual locations (honouring `CLAUDE_CONFIG_DIR` and
/// `CODEX_HOME`). Reads a lot of files; call it off the UI thread.
pub fn collect() -> Report {
    let now = now_secs();
    let env_dir = |var: &str| std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from);
    let home = home();

    let claude_dir =
        env_dir("CLAUDE_CONFIG_DIR").or_else(|| home.as_ref().map(|h| h.join(".claude")));
    let claude_json = match env_dir("CLAUDE_CONFIG_DIR") {
        Some(d) => d.join(".claude.json"),
        None => home.clone().unwrap_or_default().join(".claude.json"),
    };
    let codex_dir = env_dir("CODEX_HOME").or_else(|| home.as_ref().map(|h| h.join(".codex")));

    Report {
        claude: claude_dir.filter(|d| d.is_dir()).map(|d| claude(&d, &claude_json, now)),
        codex: codex_dir.filter(|d| d.is_dir()).map(|d| codex(&d, now)),
        at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_792_000_000; // 2026-10-17-ish

    #[test]
    fn lines_survive_invalid_utf8_and_missing_final_newline() {
        let data: &[u8] = b"one\n\xff\xfe bad\ntwo\r\nthree";
        let lines: Vec<String> = Lines::new(data).collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "one\n");
        assert!(lines[1].contains("bad"));
        assert_eq!(lines[3], "three");
    }

    fn iso(secs_ago: i64) -> String {
        let t = NOW - secs_ago;
        let (days, rem) = (t.div_euclid(DAY), t.rem_euclid(DAY));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.123Z",
            rem / HOUR,
            rem % HOUR / 60,
            rem % 60
        )
    }

    #[test]
    fn parses_timestamps_in_the_formats_the_tools_write() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("2023-11-14T22:13:20.380Z"), Some(1_700_000_000));
        assert_eq!(parse_iso("2023-11-14T22:13:20.380123456Z"), Some(1_700_000_000));
        assert_eq!(parse_iso("2023-11-15T06:13:20+08:00"), Some(1_700_000_000));
        assert_eq!(parse_iso("2023-11-14T16:43:20-05:30"), Some(1_700_000_000));
        assert_eq!(parse_iso("2023-11-14T22:13:20"), Some(1_700_000_000));
        assert_eq!(parse_iso("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(parse_iso("garbage"), None);
        assert_eq!(parse_iso("2023-11-14"), None);
        assert_eq!(parse_iso(&iso(0)), Some(NOW));
    }

    #[test]
    fn formats_counts_and_spans() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1_500), "1.5K");
        assert_eq!(fmt_count(2_300_000), "2.3M");
        assert_eq!(fmt_count(4_560_000_000), "4.56B");
        assert_eq!(fmt_span(-5), "0m");
        assert_eq!(fmt_span(59 * 60), "59m");
        assert_eq!(fmt_span(2 * HOUR + 5 * 60), "2h 05m");
        assert_eq!(fmt_span(2 * DAY + 7 * HOUR), "2d 7h");
    }

    fn assistant(
        id: &str,
        model: &str,
        ago: i64,
        input: u64,
        output: u64,
        read: u64,
        write: u64,
    ) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{}","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":{read},"cache_creation_input_tokens":{write}}}}}}}"#,
            iso(ago)
        )
    }

    #[test]
    fn claude_usage_is_windowed_and_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("projects/-home-me-app");
        fs::create_dir_all(&proj).unwrap();
        let lines = [
            r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z"}"#.to_string(),
            // The same message streamed three times: counted once, at its largest.
            assistant("m1", "claude-opus", HOUR, 10, 5, 100, 20),
            assistant("m1", "claude-opus", HOUR, 10, 50, 100, 20),
            assistant("m1", "claude-opus", HOUR, 10, 50, 100, 20),
            assistant("m2", "claude-sonnet", 10 * HOUR, 1, 2, 3, 4),
            assistant("m3", "claude-opus", 3 * DAY, 100, 0, 0, 0),
            assistant("old", "claude-opus", 30 * DAY, 999, 999, 999, 999),
            assistant("syn", "<synthetic>", HOUR, 5, 5, 5, 5),
            "not json at all {{".to_string(),
        ];
        fs::write(proj.join("sess-1.jsonl"), lines.join("\n")).unwrap();

        let info = scan_claude(&dir.path().join("projects"), NOW);
        assert_eq!(info.files_scanned, 1);
        assert_eq!(info.sessions_7d, 1);
        assert_eq!(info.last_5h.messages, 1);
        assert_eq!(
            info.last_5h.tokens,
            Tokens { input: 10, output: 50, cache_read: 100, cache_write: 20 }
        );
        assert_eq!(info.last_24h.messages, 2, "m1 and m2");
        assert_eq!(
            info.last_7d.messages, 3,
            "m1, m2, m3; not the 30-day-old one or the synthetic one"
        );
        assert_eq!(info.last_7d.tokens.input, 10 + 1 + 100);
        assert_eq!(info.models_7d[0].0, "claude-opus", "biggest model first");
        assert_eq!(info.models_7d.len(), 2);
    }

    #[test]
    fn claude_scan_skips_files_not_touched_in_a_week() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("projects")).unwrap();
        let f = dir.path().join("projects/a.jsonl");
        fs::write(&f, assistant("m", "x", HOUR, 1, 1, 1, 1)).unwrap();
        // Pretend "now" is far in the future so the file looks stale.
        let info = scan_claude(&dir.path().join("projects"), now_secs() + 30 * DAY);
        assert_eq!(info.files_scanned, 0);
    }

    #[test]
    fn claude_account_reads_only_non_secret_fields() {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join(".claude.json");
        fs::write(
            &json,
            r#"{"unrelated":{"big":[1,2,3]},"oauthAccount":{"emailAddress":"me@x.io","fullName":"Me Myself","displayName":"Me","organizationName":"Acme","organizationType":"claude_max","organizationRole":"admin","userRateLimitTier":"default_claude_max_5x","billingType":"stripe_subscription","hasExtraUsageEnabled":false,"claudeCodeTrialEndsAt":null,"subscriptionCreatedAt":"2026-01-02T03:04:05Z"}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"SECRET-ACCESS","refreshToken":"SECRET-REFRESH","expiresAt":1792000000000,"subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}"#,
        )
        .unwrap();

        let a = claude_account(dir.path(), &json).unwrap();
        assert_eq!(a.email.as_deref(), Some("me@x.io"));
        assert_eq!(a.name.as_deref(), Some("Me Myself"));
        assert_eq!(a.organization.as_deref(), Some("Acme"));
        assert_eq!(a.plan.as_deref(), Some("max"));
        assert_eq!(a.tier.as_deref(), Some("default_claude_max_20x"), "the login's tier wins");
        assert_eq!(a.extra_usage, Some(false));
        assert_eq!(a.token_expires, Some(1_792_000_000));
        let shown = format!("{a:?}");
        assert!(!shown.contains("SECRET"), "tokens must never be read: {shown}");
    }

    #[test]
    fn claude_account_is_none_without_a_login() {
        let dir = tempfile::tempdir().unwrap();
        assert!(claude_account(dir.path(), &dir.path().join("missing.json")).is_none());
        fs::write(dir.path().join(".claude.json"), "{}").unwrap();
        assert!(claude_account(dir.path(), &dir.path().join(".claude.json")).is_none());
    }

    #[test]
    fn claude_all_time_comes_from_the_stats_cache() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("stats-cache.json"),
            r#"{"lastComputedDate":"2026-10-17","totalSessions":3,"totalMessages":4267,"firstSessionDate":"2026-10-15T13:23:15.187Z","modelUsage":{"a":{"inputTokens":1,"outputTokens":2,"cacheReadInputTokens":3,"cacheCreationInputTokens":4},"b":{"inputTokens":10,"outputTokens":20,"cacheReadInputTokens":30,"cacheCreationInputTokens":40}}}"#,
        )
        .unwrap();
        let all = claude_all_time(dir.path()).unwrap();
        assert_eq!((all.sessions, all.messages), (3, 4267));
        assert_eq!(all.tokens, Tokens { input: 11, output: 22, cache_read: 33, cache_write: 44 });
        assert_eq!(all.as_of.as_deref(), Some("2026-10-17"));
        assert!(claude_all_time(&dir.path().join("nope")).is_none());
    }

    fn jwt(claims: &str) -> String {
        format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#),
            URL_SAFE_NO_PAD.encode(claims)
        )
    }

    #[test]
    fn codex_account_comes_from_the_login_claims_only() {
        let dir = tempfile::tempdir().unwrap();
        let claims = r#"{"email":"me@x.io","name":"Me","sub":"zzz","https://api.openai.com/auth":{"chatgpt_plan_type":"plus","chatgpt_subscription_active_until":"2026-11-02T07:07:23+00:00","chatgpt_user_id":"u-1"}}"#;
        fs::write(
            dir.path().join("auth.json"),
            format!(
                r#"{{"OPENAI_API_KEY":"sk-SECRET","auth_mode":"chatgpt","tokens":{{"id_token":"{}","access_token":"SECRET-AT","refresh_token":"SECRET-RT","account_id":"acc"}}}}"#,
                jwt(claims)
            ),
        )
        .unwrap();
        let a = codex_account(dir.path()).unwrap();
        assert_eq!(a.email.as_deref(), Some("me@x.io"));
        assert_eq!(a.name.as_deref(), Some("Me"));
        assert_eq!(a.plan.as_deref(), Some("plus"));
        assert_eq!(a.auth_mode.as_deref(), Some("chatgpt"));
        assert_eq!(a.subscription_until.as_deref(), Some("2026-11-02T07:07:23+00:00"));
        assert!(!format!("{a:?}").contains("SECRET"));
    }

    #[test]
    fn codex_api_key_mode_has_no_identity() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-SECRET","auth_mode":"apikey"}"#,
        )
        .unwrap();
        let a = codex_account(dir.path()).unwrap();
        assert_eq!(a.auth_mode.as_deref(), Some("apikey"));
        assert!(a.email.is_none() && a.plan.is_none());
        assert!(codex_account(&dir.path().join("none")).is_none());
        fs::write(dir.path().join("auth.json"), "not json").unwrap();
        assert!(codex_account(dir.path()).is_none());
    }

    fn token_count(
        ago: i64,
        total_input: u64,
        cached: u64,
        output: u64,
        primary_pct: f64,
        resets_in: i64,
    ) -> String {
        format!(
            r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{total_input},"cached_input_tokens":{cached},"output_tokens":{output}}}}},"rate_limits":{{"plan_type":"plus","primary":{{"used_percent":{primary_pct},"window_minutes":300,"resets_at":{}}},"secondary":{{"used_percent":13.0,"window_minutes":10080,"resets_at":{}}},"credits":{{"has_credits":false,"unlimited":false,"balance":"0"}},"rate_limit_reached_type":null}}}}}}"#,
            iso(ago),
            NOW + resets_in,
            NOW + 5 * DAY
        )
    }

    #[test]
    fn codex_limits_and_usage_come_from_session_logs() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions/2026/10/16");
        fs::create_dir_all(&day).unwrap();
        let session = |lines: &[String]| lines.join("\n");
        // An older session, then the newest one whose last event holds the totals.
        fs::write(
            day.join("rollout-a.jsonl"),
            session(&[token_count(3 * DAY, 1000, 400, 50, 80.0, -DAY)]),
        )
        .unwrap();
        fs::write(
            day.join("rollout-b.jsonl"),
            session(&[
                r#"{"timestamp":"2026-10-16T00:00:00Z","type":"session_meta","payload":{}}"#
                    .to_string(),
                token_count(2 * HOUR, 500, 100, 10, 5.0, 2 * HOUR),
                token_count(HOUR, 2000, 1500, 100, 15.0, HOUR),
            ]),
        )
        .unwrap();

        let info = scan_codex(&dir.path().join("sessions"), now_secs());
        assert_eq!(info.sessions, 2);
        // Totals use each session's last event; cached input is split out of input.
        assert_eq!(
            info.all,
            Tokens { input: 600 + 500, output: 150, cache_read: 400 + 1500, cache_write: 0 }
        );
        assert_eq!(info.all.total(), 1000 + 50 + 2000 + 100, "matches OpenAI's own total");
    }

    #[test]
    fn codex_picks_the_newest_rate_limit_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("sessions/2026/10/16");
        fs::create_dir_all(&day).unwrap();
        // Written out of order on purpose: the snapshot to keep is the most recent by timestamp.
        fs::write(
            day.join("a.jsonl"),
            [
                token_count(HOUR, 10, 0, 1, 15.0, HOUR),
                token_count(5 * HOUR, 10, 0, 1, 99.0, 5 * HOUR),
            ]
            .join("\n"),
        )
        .unwrap();
        let info = scan_codex(&dir.path().join("sessions"), NOW);
        let limits = info.limits.expect("limits found");
        assert_eq!(limits.plan.as_deref(), Some("plus"));
        let p = limits.primary.unwrap();
        assert_eq!((p.used_percent, p.window_minutes), (15.0, 300));
        assert_eq!(limits.secondary.unwrap().window_minutes, 10_080);
        assert_eq!(limits.credits.as_deref(), Some("none"));
        assert_eq!(limits.reached, None);
        assert_eq!(limits.seen_at, NOW - HOUR);
    }

    #[test]
    fn empty_or_missing_folders_give_empty_info() {
        let dir = tempfile::tempdir().unwrap();
        let c = claude(dir.path(), &dir.path().join(".claude.json"), NOW);
        assert!(c.account.is_none() && c.all_time.is_none());
        assert_eq!(c.last_7d, Window::default());
        let x = codex(dir.path(), NOW);
        assert!(x.account.is_none() && x.limits.is_none());
        assert_eq!(x.sessions, 0);
    }
}
