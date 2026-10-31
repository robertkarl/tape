//! RENDERER — owned by T5. Port of ~/Code/regina/scripts/render_trace.py.
//! Keep that file's CSS/JS/HTML structure verbatim (density modes, one-turn
//! paging, resizable timeline rail, emoji-per-tool badges, edits as diffs).
//! Only the parse step is new; the template is copied, with attribution.

use crate::parse::Session;

pub fn render_session(session: &Session) -> anyhow::Result<String> {
    let _ = session;
    anyhow::bail!("not implemented (T5)")
}
