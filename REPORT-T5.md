# REPORT — T5: renderer port + `tape serve`

Milestone: PLAN M4. Files touched: `src/render/` (new `badges.rs`,
`highlight.rs`, `template.rs`, rewritten `mod.rs`), `src/web.rs`,
`src/main.rs` (one default value), `tests/web_tests.rs`.
**`Cargo.toml` unchanged** — `axum` and `tokio` were already in the
pre-approved list. 65 tests pass (48 unit, 5 index, 9 store, 8 web).

## M4 acceptance, measured

Run against `TAPE_HOME=/tmp/tape-t3-full` (848 blobs, 743 sessions:
96 claude / 356 codex / 291 pi) and against the archive CT
(`rk@192.168.50.108`, 10 sessions). Pages opened in a real browser
(headless Chromium via lookingglass), console clean on every page.

| session | tool | turns | what the page shows |
|---|---|---|---|
| `7bd359d5-b702…` | claude | 35 | **17 subagent turns** marked (rail + turn pill), 3 edit diffs, 2 collapsed outputs |
| `019d68e5-1902…` | codex | 930 | **2503 coloured patch lines**, 87 file headers, rail badges `rg`/`git`/`sed`/`git log` |
| `019e5ae1-0768…` | pi | 27 | 12 edit diffs, 20 thinking disclosures, tokens 148477 in / 9944 out |
| `01a1899d-a523…` | pi (on the CT) | 90 | edit diff + `write` with highlighted Python, over the LAN from debra |
| `e707f04a-fed3…` | claude | 8630 | 6563 subagent turns, 2062 collapsed outputs, 15 repeats — **1.06 s, 45 MB page** |

List page agrees with the index: `?tool=claude` → 96 rows,
`?cwd=ubdogs` → exactly the 10 D9 sessions, `?since=2030-01-01` → 0,
`?limit=10` → 10 with a pager that keeps the filters.

Interactions verified in the browser, not by reading the JS: rail click
navigates (`9 / 35`), density keys 1/2/3 switch modes (skim reveals all 35
turns as one line each), `full` reveals tool output, next/prev page.

## Ported verbatim (translated, not redesigned)

`template.rs` is render_trace.py's `HTML_TMPL` — the whole CSS and both JS
blocks — with two mechanical changes: Python's doubled braces un-doubled, and
`{slot}` became `@@SLOT@@` filled by a small `fill()`. That second change is
not cosmetic: a CSS block full of `{` must not go through a formatter that
reads braces. Kept exactly: density modes (skim/read/full, which *remove*
things rather than rescale), one-turn-at-a-time paging, the resizable rail with
its localStorage width, the skim line being the same badge as the rail (so skim
is a CSS switch, not a second pass), `min-width:0` on grid children (the comment
in the original explains that it is load-bearing), wrap-not-scroll on code
panes, the repeats table, the 1400-char collapse with line/char counts in the
summary.

`badges.rs`: emoji-per-tool, `bash_verb` (stepping past `cd X &&` and `FOO=bar`
prefixes — including Python's `str.isupper()` subtlety, where `=1` is not an
env prefix because it has no cased character), quote-aware `split_stages`,
heredoc detection that excludes `$((1 << 4))`, the 6-verb cap, basename-only
display for file tools.

`highlight.rs`: the dependency-free highlighter as a left-to-right scanner with
the original's precedence — strings, then comments, then numbers, then
keywords, then flags — over **already-escaped** text, which is why string
delimiters are matched as `&quot;` / `&#x27;`. No regex crate added. Tests pin
the precedence (`"if this then that"` inside quotes stays a string) and the
word boundaries (`diff` is not `if`; digits inside an entity are not numbers).

## Deliberately not ported

`find_context_files`, `context_seen`, `injected_context`, `find_meta`
(lines 84–208) read a live source repo — `repo/AGENTS.md`,
`harness_frame.json`, `result.json`. The archive host has blobs and an index,
not the repo, and reading the repo would break the whole point of the archive.
The header's regina-only slots became archive-honest ones: `task`→`cwd`,
`quant`→`tool`/`host`, KV-cache→tokens/cost/branch, `score`→tool-call count,
`status`→`archived`. The `<details>` wrapper around reference material stayed,
now holding only the repeats table.

## New because the archive has what regina's runs did not

1. **Claude subagent turns.** `.side` on the rail row and the turn, plus a
   `subagent` pill and a `subagent turns yes` header flag. The parsers already
   set `is_sidechain`; the renderer finally shows it.
2. **Claude's `old_string`/`new_string` edits.** The original only knew pi's
   `edits` array, so Claude — the tool with the most edits — would have shown
   raw JSON. Both dialects now render as path + `change i of n` + replace/with.
3. **Codex text patches.** `apply_patch` input is not JSON at all, so it is not
   run through a JSON parser; it is coloured line by line (`pl-add`/`pl-del`/
   `pl-file`/`pl-hunk`). 2503 add-lines coloured in one real session.
4. **Multi-blob merge.** A Claude session and its `subagents/agent-*.jsonl`
   travel as separate blobs under one id, so the session page decrypts them all
   and concatenates turns (metadata blob first, then first-timestamp order).
   The **index's** session id wins over the parser's: a subagent file carries no
   session header, and without that rule the merged session would be named `""`
   — a bug the tests caught.

## Guardrails, and the bugs the tests found

- The index connection is opened `PRAGMA query_only` before any query. Test:
  an `INSERT` through the web state fails and the row count is unchanged.
- Filters are numbered parameters, never SQL text. Test: `cwd="'; DROP TABLE
  sessions; --"` returns 0 rows and `sessions` still has its 3.
- `tape serve` **refuses to bind `0.0.0.0`** (decision D3 means no auth), and
  the CLI default is now `127.0.0.1` so a routable bind is something typed.
- A tampered blob is an error, not a page: flipping one ciphertext byte makes
  the session page fail with `hash mismatch` (the blob's content hash is
  re-checked after decryption on every read).
- Two real bugs the first run caught: `Filters::default()` had `limit: 0`, so
  the list rendered an empty table while reporting the right count; and axum 0.8
  uses `{param}` routes, not `:param` — it panicked at startup, loudly.
- One bug a **screenshot** caught that no test would: a long `cwd` is one
  unbroken token, so it overran the model column. `word-break:break-all` + a
  max-width fixed it.

## Known limits (honest list, not blockers)

- **45 MB for an 8630-turn session.** Correct and fast to generate (1.06 s),
  heavy for a browser to parse. The fix is server-side turn paging
  (`/s/{host}/{id}?from=2000`), which wants a decision about what a "page" of a
  transcript is. Not invented here because it changes the model.
- **No full-text search.** The index has no FTS table, so `q=` matches
  id/model/cwd only. Finding "every session that mentions `memcached`" needs an
  index change (T3's schema), not a UI change.
- **Codex sessions show no tokens/cost** in the header: the parser records
  usage differently and the merge doesn't yet fold `token_usage_record`.
- **`unmapped_lines` still doesn't reach the index** (T3 loose end). The
  renderer sums it across merged blobs but there is no column to show it in.
- **`serve` does not start at boot on the CT.** It runs as `rk` with
  `setsid nohup`; a systemd unit belongs with M5.
- The plaintext cache is a bounded map (64 entries, cleared wholesale), not an
  LRU. Fine at this scale, worth knowing.
