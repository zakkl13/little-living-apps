//! The Inspector's single-page UI: a self-contained HTML+JS shell with no build step and no
//! dependencies (matches the repo's minimalism). It fetches the read-only JSON API and renders each
//! panel client-side. URLs are relative so it works both at `localhost:PORT/` (dev) and behind Caddy
//! at `/_inspect/` (prod). The access token rides along from the page's own `?t=` query string and
//! the `x-inspector-token` header. Field names are snake_case, matching the server's JSON.

pub const INSPECTOR_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>lila Inspector</title>
<style>
  :root { color-scheme: dark; --bg:#0d1117; --panel:#161b22; --line:#30363d; --fg:#e6edf3; --dim:#8b949e; --acc:#58a6ff; }
  * { box-sizing: border-box; }
  body { margin:0; font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace; background:var(--bg); color:var(--fg); }
  header { padding:10px 16px; border-bottom:1px solid var(--line); display:flex; gap:16px; align-items:baseline; flex-wrap:wrap; }
  header b { color:var(--acc); }
  header .stat { color:var(--dim); }
  header .stat span { color:var(--fg); }
  nav { display:flex; gap:4px; padding:8px 12px; border-bottom:1px solid var(--line); flex-wrap:wrap; }
  nav button { background:var(--panel); color:var(--fg); border:1px solid var(--line); padding:5px 12px; border-radius:6px; cursor:pointer; font:inherit; }
  nav button.active { border-color:var(--acc); color:var(--acc); }
  main { padding:16px; max-width:1100px; }
  .card { background:var(--panel); border:1px solid var(--line); border-radius:8px; padding:12px 14px; margin-bottom:12px; }
  .card h3 { margin:0 0 8px; font-size:13px; }
  .role { font-size:11px; text-transform:uppercase; letter-spacing:.06em; color:var(--dim); }
  .blk { border-left:2px solid var(--line); padding:2px 0 2px 10px; margin:6px 0; }
  .blk.text { border-color:var(--acc); }
  .blk.thinking { border-color:#a371f7; color:var(--dim); }
  .blk.tool_use { border-color:#3fb950; }
  .blk.tool_result { border-color:#d29922; }
  .tag { font-size:10px; text-transform:uppercase; letter-spacing:.05em; color:var(--dim); }
  pre { white-space:pre-wrap; word-break:break-word; margin:4px 0 0; }
  a { color:var(--acc); cursor:pointer; text-decoration:none; }
  .dim { color:var(--dim); }
  .row { display:flex; justify-content:space-between; gap:12px; }
  table { border-collapse:collapse; width:100%; }
  td,th { text-align:left; padding:4px 8px; border-bottom:1px solid var(--line); vertical-align:top; }
</style>
</head>
<body>
<header>
  <b>lila Inspector</b>
  <span class="stat">backend <span id="h-backend">—</span></span>
  <span class="stat">model <span id="h-model">—</span></span>
  <span class="stat">context <span id="h-ctx">—</span> tok</span>
  <span class="stat">total tokens <span id="h-tokens">—</span></span>
  <span class="stat">turns <span id="h-turns">—</span></span>
  <span class="stat">workers <span id="h-workers">—</span></span>
</header>
<nav id="nav"></nav>
<main id="main">loading…</main>
<script>
const T = new URLSearchParams(location.search).get('t');
const api = (p) => fetch(p + (p.includes('?') ? '&' : '?') + 't=' + encodeURIComponent(T || ''), { headers: { 'x-inspector-token': T || '' } }).then(r => r.json());
const esc = (s) => String(s ?? '').replace(/[&<>]/g, c => ({ '&':'&amp;','<':'&lt;','>':'&gt;' }[c]));
const main = document.getElementById('main');
const fmt = (n) => (n || 0).toLocaleString();
const sumTokens = (u) => (u.input_tokens||0) + (u.output_tokens||0) + (u.reasoning_tokens||0) + (u.worker_input_tokens||0) + (u.worker_output_tokens||0) + (u.worker_reasoning_tokens||0);

const TABS = {
  Overview: renderOverview,
  Conversation: renderConversation,
  Trace: renderTrace,
  Workers: renderWorkers,
  Memories: renderMemories,
  Usage: renderUsage,
};

const nav = document.getElementById('nav');
let active = 'Overview';
Object.keys(TABS).forEach(name => {
  const b = document.createElement('button');
  b.textContent = name;
  b.onclick = () => { active = name; draw(); };
  b.dataset.tab = name;
  nav.appendChild(b);
});

async function refreshHeader() {
  const o = await api('api/overview');
  document.getElementById('h-backend').textContent = o.backend;
  document.getElementById('h-model').textContent = o.manager_model || '(default)';
  document.getElementById('h-ctx').textContent = fmt(o.context_tokens);
  document.getElementById('h-tokens').textContent = fmt(sumTokens(o.usage));
  document.getElementById('h-turns').textContent = o.counts.turns;
  document.getElementById('h-workers').textContent = o.counts.workers;
  return o;
}

async function draw() {
  [...nav.children].forEach(b => b.classList.toggle('active', b.dataset.tab === active));
  main.textContent = 'loading…';
  try { await TABS[active](); } catch (e) { main.innerHTML = '<div class="card">error: ' + esc(e.message) + '</div>'; }
}

function card(html) { return '<div class="card">' + html + '</div>'; }
function row(k, v) { return '<div class="row"><span class="dim">' + esc(k) + '</span><span>' + esc(v) + '</span></div>'; }

async function renderOverview() {
  const o = await refreshHeader();
  const u = o.usage;
  let h = card('<h3>Runtime</h3>'
    + row('Backend', o.backend)
    + row('Manager model', o.manager_model || '(default)')
    + row('Workspace', o.workspace_dir)
    + row('App public URL', o.app_public_url || '(not published)')
    + row('Context tokens (last call)', fmt(o.context_tokens)));
  h += card('<h3>Manager tokens</h3>'
    + row('Input', fmt(u.input_tokens) + ' (' + fmt(u.cached_input_tokens) + ' cached)')
    + row('Output', fmt(u.output_tokens) + ' (' + fmt(u.reasoning_tokens) + ' reasoning)')
    + row('Manager turns', u.manager_turns));
  h += card('<h3>Worker tokens</h3>'
    + row('Input', fmt(u.worker_input_tokens) + ' (' + fmt(u.worker_cached_input_tokens) + ' cached)')
    + row('Output', fmt(u.worker_output_tokens) + ' (' + fmt(u.worker_reasoning_tokens) + ' reasoning)')
    + row('Worker runs', u.worker_turns)
    + '<div class="dim" style="margin-top:8px">Everything rides the subscription — no metered $.</div>');
  if (o.last_turn) h += card('<h3>Last turn #' + o.last_turn.turn_id + ' · ' + esc(o.last_turn.kind) + '</h3>'
    + '<pre>' + esc(o.last_turn.request) + '</pre>'
    + '<div class="dim">' + o.last_turn.iterations + ' iteration(s) · ' + fmt(o.last_turn.input_tokens) + ' in / ' + fmt(o.last_turn.output_tokens) + ' out</div>');
  main.innerHTML = h;
}

async function renderConversation() {
  const c = await api('api/conversation');
  let h = card('<div class="row"><span class="dim">' + c.message_count + ' messages</span><span class="dim">~' + fmt(c.context_tokens) + ' tokens in context</span></div>');
  for (const m of c.messages) {
    let inner = '<div class="role">' + esc(m.role) + '</div>';
    for (const b of (m.blocks || [])) inner += renderBlock(b);
    h += card(inner);
  }
  main.innerHTML = h;
}
function renderBlock(b) {
  const t = b.block;
  let body = '';
  if (t === 'text') body = esc(b.text || '');
  else if (t === 'thinking') body = '(private reasoning)';
  else if (t === 'tool_use') body = esc(b.name || '');
  else if (t === 'tool_result') body = esc(b.content || '');
  else body = esc(JSON.stringify(b).slice(0, 400));
  return '<div class="blk ' + esc(t) + '"><span class="tag">' + esc(t) + '</span><pre>' + body + '</pre></div>';
}

async function renderTrace() {
  const d = await api('api/trace');
  if (!d.turns.length) { main.innerHTML = card('<span class="dim">no turns recorded yet</span>'); return; }
  let h = '';
  for (const t of d.turns) {
    let inner = '<h3>#' + t.turn_id + ' · ' + esc(t.kind) + '</h3>';
    inner += '<pre>' + esc(t.request) + '</pre>';
    inner += '<div class="dim">' + t.iterations + ' iteration(s) · ' + fmt(t.input_tokens) + ' in / ' + fmt(t.output_tokens) + ' out</div>';
    for (const p of t.prompts) {
      inner += '<div class="blk tool_use"><span class="tag">' + esc(p.kind) + ' → ' + esc(p.worker_id) + '</span><pre>' + esc(p.prompt || '(no prompt)') + '</pre></div>';
    }
    h += card(inner);
  }
  main.innerHTML = h;
}

async function renderWorkers() {
  // Workers are single-shot; this is the dispatch history (newest first), not a live roster.
  const d = await api('api/workers');
  if (!d.workers.length) { main.innerHTML = card('<span class="dim">no workers dispatched yet</span>'); return; }
  let h = '';
  for (const w of d.workers) {
    let inner = '<div class="row"><h3>' + esc(w.id) + '</h3><span class="dim">single-shot</span></div>';
    for (const p of w.prompts) inner += '<div class="blk tool_use"><span class="tag">' + esc(p.kind) + ' · turn #' + p.turn_id + '</span><pre>' + esc(p.prompt || '(none)') + '</pre></div>';
    h += card(inner);
  }
  main.innerHTML = h;
}

async function renderMemories() {
  const d = await api('api/memories');
  let h = card('<span class="dim">' + d.files.length + ' memory files</span>');
  for (const f of d.files) {
    h += card('<h3>' + esc(f.path) + '</h3><pre>' + esc(f.body) + '</pre>');
  }
  main.innerHTML = h;
}

async function renderUsage() {
  const d = await api('api/usage');
  const m = d.meter;
  let h = card('<h3>Token usage meter</h3>'
    + row('Manager input', fmt(m.input_tokens) + ' (' + fmt(m.cached_input_tokens) + ' cached)')
    + row('Manager output', fmt(m.output_tokens) + ' (' + fmt(m.reasoning_tokens) + ' reasoning)')
    + row('Manager turns', m.manager_turns)
    + row('Worker input', fmt(m.worker_input_tokens) + ' (' + fmt(m.worker_cached_input_tokens) + ' cached)')
    + row('Worker output', fmt(m.worker_output_tokens) + ' (' + fmt(m.worker_reasoning_tokens) + ' reasoning)')
    + row('Worker runs', m.worker_turns)
    + '<div class="dim" style="margin-top:8px">' + esc(d.note) + '</div>');
  let rows = d.turns.slice().reverse().map(t =>
    '<tr><td>#' + t.turn_id + '</td><td>' + esc(t.kind) + '</td><td>' + t.iterations + '</td><td>' + fmt(t.input_tokens) + '</td><td>' + fmt(t.output_tokens) + '</td></tr>').join('');
  h += card('<h3>Per-turn (manager)</h3><table><tr><th>turn</th><th>kind</th><th>iters</th><th>in</th><th>out</th></tr>' + rows + '</table>');
  main.innerHTML = h;
}

draw();
</script>
</body>
</html>"##;
