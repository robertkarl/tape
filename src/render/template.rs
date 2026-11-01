//! The page template, ported VERBATIM from render_trace.py's `HTML_TMPL`
//! (regina, 2026). Two mechanical changes, no redesign:
//!
//!   * Python's `str.format` doubled every brace (`{{`); they are un-doubled.
//!   * `{title}`-style slots became `@@TITLE@@`, filled by `fill()`. A CSS block
//!     full of braces must never go through a format engine that reads `{`.
//!
//! Everything below the "tape additions" marker is new, because the archive has
//! things regina's traces never did: Claude subagent turns, Codex patches, and
//! a list page to arrive from.

/// Fill `@@NAME@@` slots. Values are already escaped by the caller.
pub fn fill(tmpl: &str, slots: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(tmpl.len() + 1024);
    let mut rest = tmpl;
    while let Some(i) = rest.find("@@") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("@@") {
            Some(j) => {
                let name = &after[..j];
                match slots.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(v),
                    // An unfilled slot is a bug worth seeing, not a hole.
                    None => out.push_str(&format!("<!-- unfilled {name} -->")),
                }
                rest = &after[j + 2..];
            }
            None => {
                out.push_str("@@");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

pub const PAGE: &str = r##"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>@@TITLE@@</title>
<style>
:root{--bg:#0d1117;--panel:#161b22;--panel2:#1c2330;--edge:#2a3240;
--fg:#e6edf3;--dim:#8b949e;--acc:#58a6ff;--ok:#3fb950;--to:#d29922;--err:#f85149;
--in:#132a1c;--out:#0f1620;--think:#171226;--mono:'SF Mono',ui-monospace,'JetBrains Mono',Menlo,monospace;}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,Segoe UI,sans-serif;}
.wrap{display:grid;grid-template-columns:var(--rail,340px) 6px minmax(0,1fr);max-width:1600px;margin:0 auto;}
.wrap>*{min-width:0;}
/* drag handle to resize the timeline tray */
.grip{position:sticky;top:0;height:100vh;cursor:col-resize;background:var(--edge);
opacity:.5;transition:opacity .15s;}
.grip:hover,.grip.drag{opacity:1;background:var(--acc);}
body.resizing{cursor:col-resize;user-select:none;}
/* ---- sticky timeline rail ---- */
.rail{position:sticky;top:0;align-self:start;height:100vh;overflow-y:auto;
padding:14px 8px;border-right:1px solid var(--edge);background:var(--panel);}
.rail h2{font-size:12px;text-transform:uppercase;letter-spacing:.08em;color:var(--dim);margin:4px 6px 10px;}
.tl-row{display:flex;align-items:center;gap:9px;padding:10px 8px;border-radius:6px;
text-decoration:none;color:var(--fg);font:12px/1.5 var(--mono);}
.tl-row:hover{background:var(--panel2);}
.tl-row.on{background:rgba(88,166,255,.13);box-shadow:inset 2px 0 0 var(--acc);}
.tl-row.user{opacity:.75}
.tl-n{width:24px;color:var(--dim);text-align:right;font-size:11px;}
.tl-off{width:46px;color:var(--dim);text-align:right;font-size:11px;}
.tl-badge{flex:1;min-width:0;display:flex;align-items:center;gap:9px;padding:0;
overflow:hidden;white-space:nowrap;}
.tl-em{font-size:20px;line-height:1;}
.tl-cmd{color:#c9d1d9;overflow:hidden;text-overflow:ellipsis;font-size:12.5px;}
.tl-more{color:var(--dim);font-size:11px;flex:none;}
.tl-dur{width:46px;color:var(--dim);text-align:right;font-size:11px;}
/* ---- main ---- */
main{padding:30px 48px 140px;min-width:0;}
.bar{display:flex;align-items:center;gap:10px;margin:0 0 26px;font:12px var(--mono);}
.bar .sp{margin-left:auto;color:var(--dim);}
.dbtn{background:var(--panel2);border:1px solid var(--edge);color:var(--dim);
padding:5px 13px;border-radius:20px;cursor:pointer;font:12px var(--mono);}
.dbtn[aria-pressed="true"]{background:rgba(88,166,255,.14);color:var(--acc);border-color:var(--acc);}
.nav{background:none;border:1px solid var(--edge);color:var(--dim);border-radius:6px;
cursor:pointer;padding:4px 11px;font:13px var(--mono);}
.nav:hover{color:var(--fg);border-color:var(--acc);}
header.run{background:none;border:0;border-bottom:1px solid var(--edge);
border-radius:0;padding:0 0 16px;margin-bottom:20px;}
header.run h1{margin:0 0 4px;font:600 20px var(--mono);}
/* The repeats table and context dump are reference material, not the read.
   Behind one disclosure they stop competing with the transcript. */
.ctxwrap{margin:0 0 30px;}
.ctxwrap>summary{cursor:pointer;color:var(--dim);font:12px var(--mono);
text-transform:uppercase;letter-spacing:.08em;padding:4px 0;}
.meta{display:flex;flex-wrap:wrap;gap:8px 20px;color:var(--dim);font:13px var(--mono);margin-top:6px;}
.meta b{color:var(--fg);font-weight:600;}
.pill{padding:2px 10px;border-radius:20px;font:600 12px var(--mono);}
.st-to{background:rgba(210,153,34,.16);color:var(--to);}
.st-ok{background:rgba(63,185,80,.16);color:var(--ok);}
.score{background:rgba(88,166,255,.14);color:var(--acc);}
/* repeats */
.reps{border-collapse:collapse;width:100%;font:13px var(--mono);margin:4px 0 24px;}
.reps td{border-bottom:1px solid var(--edge);padding:5px 8px;vertical-align:top;}
.reps .rn{color:var(--to);white-space:nowrap;font-weight:700;}
.reps .rt{color:var(--acc);} .reps .rc code{color:var(--fg);}
h2.sec{font-size:13px;text-transform:uppercase;letter-spacing:.08em;color:var(--dim);
border-bottom:1px solid var(--edge);padding-bottom:6px;margin:26px 0 12px;}
/* ---- turns ------------------------------------------------------------
   One frame at a time. The stack of bordered cards is gone: a turn sits on
   the page background with air around it, and only ONE is visible at once
   (.turn[hidden] via JS), so a 70-turn timeout is as calm as a 6-turn pass.
   Nested boxes are gone too -- tool input/output are separated by a single
   hairline and whitespace instead of three concentric borders. */
.turn{margin:0;min-width:0;max-width:var(--measure,88ch);}
.turn-hd{display:flex;gap:14px;align-items:center;padding:0 0 14px;
font:12px var(--mono);color:var(--dim);}
.turn-hd .tn{color:var(--acc);font-weight:700;}
.turn-hd .troll{text-transform:uppercase;letter-spacing:.06em;}
.turn-hd .tdur{margin-left:auto;color:var(--to);}
.say{padding:0 0 18px;white-space:pre-wrap;}
.think{margin:0 0 18px;border-left:2px solid #6e40c9;background:transparent;}
.think summary{cursor:pointer;padding:2px 12px;color:#b596ff;font:12px var(--mono);}
.think-body{padding:6px 14px 2px;white-space:pre-wrap;color:#c9b8f0;font:13px/1.6 var(--mono);}
/* tool i/o -- no card, no nesting; the header is a quiet label, not a bar */
.tool{margin:0 0 22px;min-width:0;}
.tool-hd{display:flex;align-items:center;gap:8px;padding:0 0 6px;background:none;
color:#e3b341;font:600 12px var(--mono);}
.temoji{font-size:15px;line-height:1;}
.tverb{color:var(--dim);font-weight:400;}
/* ---- density ----------------------------------------------------------
   Browser zoom rescales; these modes actually REMOVE things. Skim shows one
   line per turn and the whole trace at once; read/full show a single turn. */
.skim{display:none;align-items:center;gap:10px;padding:7px 0;
font:12.5px var(--mono);color:#c9d1d9;border-bottom:1px solid var(--edge);}
body[data-density="skim"] .turn{display:block!important;max-width:none;}
body[data-density="skim"] .turn-hd,
body[data-density="skim"] .turn-body{display:none;}
body[data-density="skim"] .skim{display:flex;}
body[data-density="read"] .think,
body[data-density="read"] .io-out{display:none;}
/* min-width:0 is load-bearing: a grid child defaults to min-width:auto, which
   lets <pre> grow to its content width, so overflow-x never fires and long
   single-line commands are silently CLIPPED at the panel edge. */
.io{display:grid;grid-template-columns:1fr;min-width:0;}
.io>*{min-width:0;}
.in{background:var(--in);min-width:0;border-radius:6px;}
.io-out{min-width:0;}
.outwrap{background:transparent;border-top:1px solid var(--edge);min-width:0;margin-top:8px;}
.outwrap summary{cursor:pointer;padding:6px 0;color:var(--dim);font:12px var(--mono);}
/* Wrap rather than scroll: these panes are read in screenshots, where an
   offscreen horizontal scrollbar is invisible. overflow-wrap:anywhere breaks
   unbroken runs (long paths, base64, --flag=value) that break-word won't. */
pre.code{margin:0;padding:12px 16px;overflow-x:auto;white-space:pre-wrap;
overflow-wrap:anywhere;word-break:break-word;min-width:0;max-width:100%;
font:13px/1.6 var(--mono);color:var(--fg);tab-size:4;}
pre.out{color:#c9d1d9;} pre.out.err{color:#ffb4ab;background:rgba(248,81,73,.06);}
pre.out.empty{color:var(--dim);font-style:italic;}
/* Edit calls are visual diffs, not raw JSON transport envelopes. */
.edit-input{padding:0;}
.edit-path{padding:9px 16px;border-bottom:1px solid var(--edge);color:var(--fg);font:12px var(--mono);}
.edit-path span{color:var(--dim);text-transform:uppercase;letter-spacing:.08em;margin-right:8px;}
.edit-path code{color:#a5d6ff;}
.change{margin:0;border-bottom:1px solid var(--edge);}
.change:last-child{border-bottom:0;}
.change-hd{padding:7px 16px;background:#132332;color:var(--dim);font:600 11px var(--mono);text-transform:uppercase;letter-spacing:.08em;}
.change-label{padding:5px 16px;font:600 11px var(--mono);text-transform:uppercase;letter-spacing:.08em;}
.change-label.old{color:#ff9b94;background:rgba(248,81,73,.09);}
.change-label.new{color:#8fd07a;background:rgba(63,185,80,.09);border-top:1px solid var(--edge);}
pre.edit-old{background:rgba(248,81,73,.055);color:#ffd0cc;}
pre.edit-new{background:rgba(63,185,80,.055);color:#d1f5c7;}
pre.edit-raw{color:var(--dim);}
/* tokens */
.tok-str{color:#a5d6ff;} .tok-com{color:#8b949e;font-style:italic;}
.tok-num{color:#79c0ff;} .tok-kw{color:#ff7b72;} .tok-flag{color:#e3b341;}
.none{color:var(--dim);}
.ctxf{border:1px solid var(--bd);border-radius:6px;margin:6px 0;background:var(--panel2);}
.ctxf summary{cursor:pointer;padding:8px 10px;font:13px var(--mono);}
.ctxpre{margin:0;padding:10px;border-top:1px solid var(--bd);white-space:pre-wrap;font:12px var(--mono);color:var(--fg);overflow-x:auto;}
.ctxnote{font:400 12px var(--mono);color:var(--dim);}
/* ---- tape additions (the archive has what regina's runs did not) -------- */
/* Claude subagent turns: same transcript, visibly a different speaker. */
.tl-row.side .tl-em{opacity:.6;}
.tl-row.side{opacity:.8;}
.turn.side{border-left:2px solid var(--edge);padding-left:18px;}
.turn-hd .tside{color:#b596ff;border:1px solid #6e40c9;border-radius:20px;
padding:1px 9px;font:600 11px var(--mono);text-transform:uppercase;letter-spacing:.06em;}
/* Codex applies patches as text; colour the +/- so it reads as a diff. */
.pl-add{color:#8fd07a;background:rgba(63,185,80,.07);display:inline-block;
width:100%;box-sizing:border-box;}
.pl-del{color:#ff9b94;background:rgba(248,81,73,.07);display:inline-block;
width:100%;box-sizing:border-box;}
.pl-hunk{color:var(--acc);}
.pl-file{color:#a5d6ff;font-weight:600;}
a.nav{text-decoration:none;display:inline-block;}
</style></head><body><div class="wrap">
<nav class="rail"><h2>timeline · @@TURNS@@ turns</h2>@@TIMELINE@@</nav>
<div class="grip" id="grip" title="drag to resize · double-click to reset"></div>
<main>
<header class="run"><h1>@@MODEL@@</h1>
<div class="meta">
<span>cwd <b>@@CWD@@</b></span><span>tool <b>@@TOOL@@</b></span><span>host <b>@@HOST@@</b></span>@@EXTRA@@
<span class="pill score">@@SCORE@@</span>
<span class="pill @@STCLS@@">@@STATUS@@</span>
<span>wall <b>@@WALL@@</b></span><span>turns <b>@@TURNS@@</b></span>
</div></header>
<div class="bar">
<a class="nav" href="/" title="all sessions">&larr; sessions</a>
<button class="dbtn" data-d="skim">skim</button>
<button class="dbtn" data-d="read">read</button>
<button class="dbtn" data-d="full">full</button>
<span class="sp"><button class="nav" id="prev">&larr;</button>
<span id="pos"></span>
<button class="nav" id="next">&rarr;</button></span>
</div>
<details class="ctxwrap"><summary>repeated tool inputs</summary>
<h2 class="sec">🔁 most-repeated tool inputs</h2>@@REPS@@</details>
@@BODY@@
</main></div>
<script>
(function(){
  var root=document.documentElement, grip=document.getElementById('grip');
  var KEY='tape-trace-rail', MIN=160, MAX=620, DEF=340;
  var saved=parseInt(localStorage.getItem(KEY)||'',10);
  if(saved>=MIN&&saved<=MAX) root.style.setProperty('--rail',saved+'px');
  var dragging=false;
  grip.addEventListener('mousedown',function(e){
    dragging=true; grip.classList.add('drag'); document.body.classList.add('resizing');
    e.preventDefault();
  });
  window.addEventListener('mousemove',function(e){
    if(!dragging) return;
    var w=Math.max(MIN,Math.min(MAX,e.clientX-root.getBoundingClientRect().left));
    root.style.setProperty('--rail',w+'px');
  });
  window.addEventListener('mouseup',function(){
    if(!dragging) return;
    dragging=false; grip.classList.remove('drag'); document.body.classList.remove('resizing');
    var cur=getComputedStyle(root).getPropertyValue('--rail').trim();
    localStorage.setItem(KEY,parseInt(cur,10)||DEF);
  });
  grip.addEventListener('dblclick',function(){
    root.style.setProperty('--rail',DEF+"px"); localStorage.setItem(KEY,DEF);
  });
})();
/* ---- density + one-turn-at-a-time paging ---- */
(function(){
  var DKEY='tape-trace-density';
  var turns=[].slice.call(document.querySelectorAll('.turn'));
  var rows=[].slice.call(document.querySelectorAll('.tl-row'));
  var pos=document.getElementById('pos'), cur=0, density='read';

  function show(i){
    if(!turns.length) return;
    cur=Math.max(0,Math.min(turns.length-1,i));
    var solo=(density!=='skim');
    turns.forEach(function(t,n){ t.hidden = solo && n!==cur; });
    rows.forEach(function(r,n){ r.classList.toggle('on', solo && n===cur); });
    pos.textContent = solo ? (cur+1)+' / '+turns.length : turns.length+' turns';
    if(solo) window.scrollTo(0,0);
  }
  function setD(d){
    density=d; document.body.dataset.density=d;
    localStorage.setItem(DKEY,d);
    [].forEach.call(document.querySelectorAll('.dbtn'),function(b){
      b.setAttribute('aria-pressed', b.dataset.d===d ? 'true':'false');
    });
    show(cur);
  }
  [].forEach.call(document.querySelectorAll('.dbtn'),function(b){
    b.addEventListener('click',function(){ setD(b.dataset.d); });
  });
  document.getElementById('prev').addEventListener('click',function(){ show(cur-1); });
  document.getElementById('next').addEventListener('click',function(){ show(cur+1); });
  /* The rail is now navigation: clicking a row swaps the frame rather than
     scrolling to a card that may be hidden. */
  rows.forEach(function(r,n){
    r.addEventListener('click',function(e){
      if(density==='skim') return;
      e.preventDefault(); show(n);
    });
  });
  window.addEventListener('keydown',function(e){
    if(/^(INPUT|TEXTAREA)$/.test((e.target.tagName||''))) return;
    if(e.key==='ArrowLeft'||e.key==='k'){ show(cur-1); e.preventDefault(); }
    else if(e.key==='ArrowRight'||e.key==='j'){ show(cur+1); e.preventDefault(); }
    else if(e.key==='1'){ setD('skim'); }
    else if(e.key==='2'){ setD('read'); }
    else if(e.key==='3'){ setD('full'); }
  });
  setD(localStorage.getItem(DKEY)||'read');
})();
</script>
</body></html>"##;
