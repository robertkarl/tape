//! WEB UI — axum, bound to a LAN address only (decision D3): no nginx, no TLS,
//! no auth, no DNS. **Read-only**: the connection is put in `query_only` mode
//! before a single query runs, so a bug in this file cannot touch the archive,
//! and every page is built from blobs + index — never from a source tree.
//!
//! Two pages: the session list (filter by tool / host / date / cwd) and one
//! session page, which decrypts that session's blobs, parses them, and renders.
//! The index answers "which sessions exist, and what is in them at a glance";
//! only the blobs can say what was actually said.

use crate::db;
use crate::parse::{self, Session};
use crate::paths;
use crate::render::{self, esc, template};
use age::x25519::Identity;
use anyhow::{Context, Result};
use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct AppState {
    /// Public so a test can prove the read-only guard holds.
    pub conn: Mutex<Connection>,
    identity: Identity,
    blobs_dir: PathBuf,
    /// hash -> decrypted plaintext. Decryption (age + zstd) is the expensive
    /// part of a page load, and a session page gets opened repeatedly while
    /// someone reads it. Bounded, because this box has 1 GB.
    plain: Mutex<BTreeMap<String, String>>,
}

const LIST: &str = r##"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>tape · @@COUNT@@ sessions</title>
<style>
:root{--bg:#0d1117;--panel:#161b22;--panel2:#1c2330;--edge:#2a3240;--fg:#e6edf3;
--dim:#8b949e;--acc:#58a6ff;--ok:#3fb950;--to:#d29922;
--mono:'SF Mono',ui-monospace,'JetBrains Mono',Menlo,monospace;}
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,Segoe UI,sans-serif;}
main{max-width:1400px;margin:0 auto;padding:30px 40px 120px;}
h1{font:600 20px var(--mono);margin:0 0 4px;}
.sub{color:var(--dim);font:12px var(--mono);margin-bottom:22px;}
form{display:flex;flex-wrap:wrap;gap:8px;margin:0 0 18px;font:13px var(--mono);}
input,select,button{background:var(--panel2);border:1px solid var(--edge);color:var(--fg);
border-radius:6px;padding:6px 9px;font:13px var(--mono);}
button{cursor:pointer}button:hover{border-color:var(--acc);color:var(--acc);}
table{border-collapse:collapse;width:100%;font:13px var(--mono);}
th{text-align:left;color:var(--dim);text-transform:uppercase;letter-spacing:.06em;
font-size:11px;padding:8px;border-bottom:1px solid var(--edge);}
td{border-bottom:1px solid var(--edge);padding:8px;vertical-align:top;}
tr:hover td{background:var(--panel);}
a{color:var(--acc);text-decoration:none;}a:hover{text-decoration:underline;}
.tag{padding:1px 8px;border-radius:20px;font-size:11px;font-weight:600;}
.pi{background:rgba(88,166,255,.14);color:var(--acc);}
.claude{background:rgba(210,153,34,.16);color:var(--to);}
.codex{background:rgba(63,185,80,.16);color:var(--ok);}
.dim{color:var(--dim);} .num{text-align:right;color:var(--dim);}
.pager{margin-top:16px;font:13px var(--mono);color:var(--dim);}
</style></head><body><main>
<h1>tape</h1>
<div class="sub">@@COUNT@@ sessions · @@HOSTS@@ · append-only archive</div>
<form method="get" action="/">
<select name="tool"><option value="">all tools</option>@@TOOLS@@</select>
<select name="host"><option value="">all hosts</option>@@HOSTSSEL@@</select>
<input name="since" placeholder="since YYYY-MM-DD" value="@@SINCE@@">
<input name="until" placeholder="until YYYY-MM-DD" value="@@UNTIL@@">
<input name="cwd" placeholder="cwd contains" value="@@CWD@@">
<input name="q" placeholder="id / model / cwd contains" value="@@Q@@" size="28">
<button>filter</button>
</form>
<table><thead><tr><th>tool</th><th>host</th><th>session</th><th>cwd</th><th>model</th>
<th class="num">turns</th><th class="num">calls</th><th class="num">blobs</th><th>ended</th></tr></thead>
<tbody>@@ROWS@@</tbody></table>
<div class="pager">@@PAGER@@</div>
</main></body></html>"##;

#[derive(Clone, Debug)]
pub struct Filters {
    pub tool: String,
    pub host: String,
    pub since: String,
    pub until: String,
    pub cwd: String,
    pub q: String,
    pub offset: i64,
    pub limit: i64,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            tool: String::new(),
            host: String::new(),
            since: String::new(),
            until: String::new(),
            cwd: String::new(),
            q: String::new(),
            offset: 0,
            limit: 200,
        }
    }
}

/// Query strings arrive as text; parse defensively. Nothing here ever becomes
/// SQL text — filters become numbered parameters.
fn filters_from(params: &BTreeMap<String, String>) -> Filters {
    let get = |k: &str| params.get(k).map(|v| v.trim().to_string()).unwrap_or_default();
    let num = |k: &str, dflt: i64| get(k).parse::<i64>().unwrap_or(dflt);
    Filters {
        tool: get("tool"),
        host: get("host"),
        since: get("since"),
        until: get("until"),
        cwd: get("cwd"),
        q: get("q"),
        offset: num("offset", 0).max(0),
        limit: num("limit", 200).clamp(1, 1000),
    }
}

fn date_ms(s: &str, end_of_day: bool) -> Option<i64> {
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    let t = if end_of_day {
        d.and_hms_opt(23, 59, 59)?.and_utc()
    } else {
        d.and_hms_opt(0, 0, 0)?.and_utc()
    };
    Some(t.timestamp_millis())
}

/// WHERE clause from bound parameters only, numbered as they are pushed.
fn where_clause(f: &Filters) -> (String, Vec<SqlValue>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut vals: Vec<SqlValue> = Vec::new();
    let push = |sql: &str, v: SqlValue, clauses: &mut Vec<String>, vals: &mut Vec<SqlValue>| {
        clauses.push(sql.replace('?', &format!("?{}", vals.len() + 1)));
        vals.push(v);
    };
    if !f.tool.is_empty() {
        push("s.tool = ?", SqlValue::Text(f.tool.clone()), &mut clauses, &mut vals);
    }
    if !f.host.is_empty() {
        push("s.host = ?", SqlValue::Text(f.host.clone()), &mut clauses, &mut vals);
    }
    if !f.cwd.is_empty() {
        push(
            "s.cwd LIKE ?",
            SqlValue::Text(format!("%{}%", f.cwd)),
            &mut clauses,
            &mut vals,
        );
    }
    if !f.q.is_empty() {
        // One value, three columns: repeat it per placeholder.
        let like = SqlValue::Text(format!("%{}%", f.q));
        for _ in 0..3 {
            vals.push(like.clone());
        }
        clauses.push(format!(
            "(s.id LIKE ?{} OR s.model LIKE ?{} OR s.cwd LIKE ?{})",
            vals.len() - 2,
            vals.len() - 1,
            vals.len()
        ));
    }
    if let Some(ms) = date_ms(&f.since, false) {
        push("s.ended_ms >= ?", SqlValue::Integer(ms), &mut clauses, &mut vals);
    }
    if let Some(ms) = date_ms(&f.until, true) {
        push("s.ended_ms <= ?", SqlValue::Integer(ms), &mut clauses, &mut vals);
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (where_sql, vals)
}

pub fn list_html(state: &AppState, f: &Filters) -> Result<String> {
    let conn = state.conn.lock().expect("index lock poisoned");
    let (where_sql, mut vals) = where_clause(f);

    let total: i64 = conn.query_row(
        &format!("SELECT count(*) FROM sessions s{where_sql}"),
        rusqlite::params_from_iter(vals.iter()),
        |r| r.get(0),
    )?;

    let tools: Vec<String> = conn
        .prepare("SELECT DISTINCT tool FROM sessions ORDER BY tool")?
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    let hosts: Vec<String> = conn
        .prepare("SELECT DISTINCT host FROM sessions ORDER BY host")?
        .query_map([], |r| r.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    // Clamp here too: `Filters::default()` must never mean "show nothing".
    let limit = f.limit.clamp(1, 1000);
    vals.push(SqlValue::Integer(limit));
    vals.push(SqlValue::Integer(f.offset));
    let sql = format!(
        "SELECT s.id, s.host, s.tool, s.cwd, s.model, s.turn_count, s.ended_ms, s.blob, \
         (SELECT count(DISTINCT m.blob) FROM messages m WHERE m.session_id = s.id AND m.host = s.host), \
         (SELECT count(*) FROM tool_calls c WHERE c.session_id = s.id AND c.host = s.host) \
         FROM sessions s{where_sql} ORDER BY s.ended_ms DESC LIMIT ?{} OFFSET ?{}",
        vals.len() - 1,
        vals.len()
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = String::new();
    let mut got = 0i64;
    let mut mapped = stmt.query_map(rusqlite::params_from_iter(vals.iter()), |r| {
        let id: String = r.get(0)?;
        let host: String = r.get(1)?;
        let tool: String = r.get(2)?;
        let cwd: Option<String> = r.get(3)?;
        let model: Option<String> = r.get(4)?;
        let turns: i64 = r.get(5)?;
        let ended: Option<i64> = r.get(6)?;
        let blob: Option<String> = r.get(7)?;
        let blob_count: i64 = r.get(8)?;
        let calls: i64 = r.get(9)?;
        let when = ended
            .and_then(|ms| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms))
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "—".into());
        Ok(format!(
            "<tr><td><span class=\"tag {}\">{}</span></td><td>{}</td>\
             <td><a href=\"/s/{}/{}\">{}</a>{}<div class=\"dim\">{}</div></td>\
             <td>{}</td><td class=\"dim\">{}</td><td class=\"num\">{}</td>\
             <td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"dim\">{}</td></tr>",
            tool.to_lowercase(),
            esc(&tool),
            esc(&host),
            esc(&host),
            esc(&id),
            esc(&id),
            if blob_count > 1 {
                format!(" <span class=\"tag pi\">{blob_count} blobs</span>")
            } else {
                String::new()
            },
            blob.map(|b| esc(&b[..12.min(b.len())])).unwrap_or_default(),
            cwd.as_deref().map(esc).unwrap_or_default(),
            model.as_deref().map(esc).unwrap_or_default(),
            turns,
            calls,
            blob_count,
            when,
        ))
    })?;
    while let Some(row) = mapped.next() {
        rows.push_str(&row?);
        got += 1;
    }

    let opts = |vals_: &[String], chosen: &str| -> String {
        vals_
            .iter()
            .map(|v| format!(
                "<option value=\"{}\"{}>{}</option>",
                esc(v),
                if v == chosen { " selected" } else { "" },
                esc(v)
            ))
            .collect()
    };

    let pager = format!(
        "{}–{} of {} · {}",
        if total == 0 { 0 } else { f.offset + 1 },
        f.offset + got,
        total,
        if f.offset + limit < total {
            format!("<a href=\"{}\">older →</a>", esc(&page_link(f, f.offset + limit)))
        } else {
            "end".into()
        }
    );

    Ok(template::fill(
        LIST,
        &[
            ("COUNT", &total.to_string()),
            ("HOSTS", &hosts.join(", ")),
            ("TOOLS", &opts(&tools, &f.tool)),
            ("HOSTSSEL", &opts(&hosts, &f.host)),
            ("SINCE", &esc(&f.since)),
            ("UNTIL", &esc(&f.until)),
            ("CWD", &esc(&f.cwd)),
            ("Q", &esc(&f.q)),
            ("ROWS", &rows),
            ("PAGER", &pager),
        ],
    ))
}

/// Rebuild a query string with a new offset, so paging keeps the filters.
pub fn page_link(f: &Filters, offset: i64) -> String {
    let mut parts = vec![format!("offset={offset}"), format!("limit={}", f.limit)];
    for (k, v) in [
        ("tool", &f.tool),
        ("host", &f.host),
        ("since", &f.since),
        ("until", &f.until),
        ("cwd", &f.cwd),
        ("q", &f.q),
    ] {
        if !v.is_empty() {
            parts.push(format!("{k}={}", urlencode(v)));
        }
    }
    format!("/?{}", parts.join("&"))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Decrypt + decompress one blob. `open_and_check` re-hashes the plaintext, so a
/// bit flip is an error, not a page of mojibake.
fn plaintext(state: &AppState, hash: &str) -> Result<String> {
    if let Some(hit) = state.plain.lock().expect("cache lock").get(hash) {
        return Ok(hit.clone());
    }
    let path = crate::store::blob_path(&state.blobs_dir, hash);
    let ciphertext =
        std::fs::read(&path).with_context(|| format!("reading blob {} ({})", hash, path.display()))?;
    let bytes = crate::store::open_and_check(&ciphertext, &state.identity, hash)
        .with_context(|| format!("decrypting blob {hash}"))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mut cache = state.plain.lock().expect("cache lock");
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(hash.to_string(), text.clone());
    Ok(text)
}

/// One session, possibly several blobs: a Claude session and its subagent files
/// arrive as separate blobs under one id, so the page merges them — the blob the
/// `sessions` row points at first (it carries the metadata), then the rest in
/// first-timestamp order.
pub fn load_session(state: &AppState, host: &str, id: &str) -> Result<Session> {
    let (_primary, ordered) = {
        let conn = state.conn.lock().expect("index lock poisoned");
        let primary: Option<String> = conn
            .query_row(
                "SELECT blob FROM sessions WHERE id = ?1 AND host = ?2",
                rusqlite::params![id, host],
                |r| r.get(0),
            )
            .optional()?;
        let Some(primary) = primary else {
            anyhow::bail!("no session {id} from host {host} in the index");
        };
        let mut stmt = conn.prepare(
            "SELECT m.blob, min(m.ts_ms) FROM messages m \
             WHERE m.session_id = ?1 AND m.host = ?2 AND m.blob IS NOT NULL \
             GROUP BY m.blob ORDER BY 2",
        )?;
        let blobs: Vec<String> = stmt
            .query_map(rusqlite::params![id, host], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();

        // The blobs table carries the tool, which is what tells the parser how
        // to read the plaintext.
        let tool_of = |h: &str| -> Option<String> {
            conn.query_row(
                "SELECT tool FROM blobs WHERE hash = ?1",
                rusqlite::params![h],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
        };
        let mut ordered: Vec<(String, Option<String>)> = vec![(primary.clone(), tool_of(&primary))];
        for b in blobs {
            if b != primary {
                let tool = tool_of(&b);
                ordered.push((b, tool));
            }
        }
        (primary, ordered)
    };

    let mut merged: Option<Session> = None;
    for (hash, tool) in &ordered {
        let tool = match tool.as_deref().and_then(tool_from_label) {
            Some(t) => t,
            None => {
                tracing::warn!("blob {hash} has no tool in the blobs table; skipping");
                continue;
            }
        };
        let text = plaintext(state, hash)?;
        let parsed = parse::parse_for(tool, &text)
            .with_context(|| format!("parsing blob {hash} for session {id}"))?;
        match merged.as_mut() {
            None => merged = Some(parsed),
            Some(base) => {
                if base.cwd.is_none() {
                    base.cwd = parsed.cwd.clone();
                }
                if base.model.is_none() {
                    base.model = parsed.model.clone();
                }
                if base.git_branch.is_none() {
                    base.git_branch = parsed.git_branch.clone();
                }
                base.turns.extend(parsed.turns);
                base.in_tokens += parsed.in_tokens;
                base.out_tokens += parsed.out_tokens;
                base.cost += parsed.cost;
                base.unmapped_lines += parsed.unmapped_lines;
                base.started_ms = match (base.started_ms, parsed.started_ms) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                base.ended_ms = match (base.ended_ms, parsed.ended_ms) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
            }
        }
    }
    // The index's id wins: a subagent file carries no session header, so the
    // parser alone would name the merged session "".
    let mut sess = merged.with_context(|| format!("no blob could be parsed for session {id}"))?;
    sess.id = id.to_string();
    Ok(sess)
}

trait OptionalExt {
    type Out;
    fn optional(self) -> Result<Option<Self::Out>>;
}
impl<T> OptionalExt for rusqlite::Result<T> {
    type Out = T;
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// The index stores the tool as text. Kept local rather than added to
/// `types::Tool` so the cross-work-unit contract stays untouched.
fn tool_from_label(s: &str) -> Option<crate::types::Tool> {
    match s {
        "pi" => Some(crate::types::Tool::Pi),
        "claude" => Some(crate::types::Tool::Claude),
        "codex" => Some(crate::types::Tool::Codex),
        _ => None,
    }
}

fn error_page(status: StatusCode, msg: &str) -> axum::response::Response {
    (
        status,
        Html(format!(
            "<!DOCTYPE html><meta charset=utf-8><title>tape</title>\
             <body style=\"font:15px/1.6 ui-monospace,monospace;background:#0d1117;color:#e6edf3;padding:40px\">\
             <p><a href=\"/\" style=\"color:#58a6ff\">← sessions</a></p><pre>{}</pre>",
            esc(msg)
        )),
    )
        .into_response()
}

async fn index(
    State(state): State<Arc<AppState>>,
    Query(q): Query<BTreeMap<String, String>>,
) -> axum::response::Response {
    let f = filters_from(&q);
    match list_html(&state, &f) {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(?e, "list page");
            error_page(StatusCode::INTERNAL_SERVER_ERROR, &format!("index unreadable: {e}"))
        }
    }
}

async fn session_page(
    State(state): State<Arc<AppState>>,
    AxPath((host, id)): AxPath<(String, String)>,
) -> axum::response::Response {
    match load_session(&state, &host, &id) {
        Ok(sess) => match render::render_session(&sess, &host) {
            Ok(html) => Html(html).into_response(),
            Err(e) => {
                tracing::error!(?e, "render");
                error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("rendering failed: {e}"),
                )
            }
        },
        Err(e) => {
            tracing::warn!(?e, "session page");
            error_page(
                StatusCode::NOT_FOUND,
                &format!(
                    "{e}\n\nA session page decrypts its blobs, so it needs the archive's \
                     private identity. The index alone cannot show a transcript."
                ),
            )
        }
    }
}

async fn health(State(state): State<Arc<AppState>>) -> axum::response::Response {
    let n: i64 = state
        .conn
        .lock()
        .map(|c| {
            c.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
                .unwrap_or(-1)
        })
        .unwrap_or(-1);
    Html(format!(
        "<!DOCTYPE html><meta charset=utf-8>tape: {n} sessions indexed · identity {}",
        esc(&paths::identity_path().display().to_string())
    ))
        .into_response()
}

pub fn app_state() -> Result<Arc<AppState>> {
    let identity = store_identity()?;
    let conn = db::open_default()?;
    // Belt and braces: nothing in this process may write the archive.
    conn.pragma_update(None, "query_only", true)
        .context("setting query_only on the index")?;
    Ok(Arc::new(AppState {
        conn: Mutex::new(conn),
        identity,
        blobs_dir: paths::blobs_dir(),
        plain: Mutex::new(BTreeMap::new()),
    }))
}

fn store_identity() -> Result<Identity> {
    let p = paths::identity_path();
    crate::store::load_identity(&p).with_context(|| {
        format!(
            "the web UI decrypts blobs, so it only runs where the identity is: {}",
            p.display()
        )
    })
}

pub fn serve(addr: &str, port: u16) -> Result<()> {
    if addr == "0.0.0.0" || addr == "::" || addr.is_empty() {
        anyhow::bail!(
            "refusing to bind {addr}: this UI has no authentication (decision D3). \
             Pass this machine's LAN address."
        );
    }
    let state = app_state()?;
    let app = Router::new()
        .route("/", get(index))
        .route("/s/{host}/{id}", get(session_page))
        .route("/health", get(health))
        .with_state(state);

    let rt = tokio::runtime::Runtime::new().context("starting the tokio runtime")?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind((addr, port))
            .await
            .with_context(|| format!("binding {addr}:{port}"))?;
        eprintln!(
            "tape serve: http://{addr}:{port} — LAN only, no auth. Do not expose this port."
        );
        axum::serve(listener, app).await.context("serving")
    })
}
