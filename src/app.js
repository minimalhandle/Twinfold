'use strict';

// ---------- backend bridge (Tauri, or the browser mock for design work) ----------
const T = window.__TAURI__;
const api = T
  ? { invoke: T.core.invoke, listen: T.event.listen, pick: () => T.dialog.open({ directory: true, multiple: false }) }
  : window.TwinfoldMock;

// ---------- state ----------
const S = {
  pairs: [],
  selected: null,       // pair id, or 'runall' for the Run-all results view
  views: {},            // per pair: { plan, excluded:Set, filter, search, limit, report, error, previewing }
  running: null,        // { ids, progress }
  runAllReports: null,
  menuOpen: false,
};
const PAGE = 400;

const $ = (sel, root = document) => root.querySelector(sel);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const other = (side) => (side === 'left' ? 'right' : 'left');
const MODE = {
  synchronize: { label: 'Synchronize', glyph: '⇄', arrow: '⇄', desc: 'New, changed, renamed and deleted files go both ways.' },
  echo: { label: 'Echo', glyph: '→', arrow: '⟶', desc: 'Left to right, including renames and deletes. Files added only on the right are left alone.' },
  contribute: { label: 'Contribute', glyph: '⇢', arrow: '⇢', desc: 'Left to right, including renames. Nothing is ever deleted.' },
};

const DAY_NAMES = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
function describeSchedule(s) {
  if (!s || !s.enabled) return '';
  switch (s.repeat) {
    case 'daily': return `Daily at ${s.time}`;
    case 'weekly': return `${[...s.days].sort((a, b) => ((a + 6) % 7) - ((b + 6) % 7)).map((d) => DAY_NAMES[d]).join(', ')} at ${s.time}`;
    case 'hourly': return `Every ${s.every_hours} h from ${s.time}`;
    case 'logon': return 'When you sign in';
  }
  return '';
}
const CLOCK = '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.2"/><path d="M8 4.6V8l2.3 1.5"/></svg>';

function view(id) {
  if (!S.views[id]) S.views[id] = { plan: null, excluded: new Set(), filter: 'all', search: '', limit: PAGE, report: null, error: null, previewing: false };
  return S.views[id];
}
const current = () => S.pairs.find((p) => p.id === S.selected);

function bytes(n) {
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return i === 0 ? `${n} B` : `${n.toFixed(n < 10 ? 1 : 0)} ${u[i]}`;
}

function ago(iso) {
  if (!iso) return '';
  const s = (Date.now() - new Date(iso).getTime()) / 1000;
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  if (s < 86400 * 7) return `${Math.floor(s / 86400)} d ago`;
  return new Date(iso).toLocaleDateString();
}

let toastTimer;
function toast(msg) {
  const el = $('#toast');
  el.textContent = msg;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.hidden = true), 4200);
}

function confirmBox(title, text, ok = 'OK') {
  const d = $('#confirmDialog');
  $('#confirmTitle').textContent = title;
  $('#confirmText').textContent = text;
  $('#confirmYes').textContent = ok;
  d.returnValue = '';
  d.showModal();
  return new Promise((res) => d.addEventListener('close', () => res(d.returnValue === 'yes'), { once: true }));
}

// ---------- data ----------
async function loadPairs() {
  try {
    S.pairs = await api.invoke('get_pairs');
  } catch (e) {
    S.pairs = [];
    toast(String(e));
  }
  if (S.selected !== 'runall' && !S.pairs.some((p) => p.id === S.selected)) S.selected = S.pairs[0]?.id ?? null;
  render();
}

// ---------- sidebar ----------
function renderSidebar() {
  const list = $('#pairList');
  list.innerHTML = S.pairs.map((p) => {
    const busy = S.running && S.running.progress?.pairId === p.id;
    const lr = p.last_run;
    const dot = busy ? 'busy' : lr ? (lr.ok ? 'ok' : 'bad') : '';
    const meta = busy ? 'Running…' : lr ? `${ago(lr.at)} · ${lr.summary}` : 'Never run';
    return `<button class="pair-item ${p.id === S.selected ? 'selected' : ''} ${p.active ? '' : 'inactive'}" data-id="${esc(p.id)}" title="${esc(p.left)}  ${MODE[p.mode].glyph}  ${esc(p.right)}">
      <span class="glyph ${p.mode}">${MODE[p.mode].glyph}</span>
      <span style="min-width:0"><div class="name">${esc(p.name)}</div><div class="meta">${esc(meta)}</div></span>
      <span class="dot ${dot}"></span>
    </button>`;
  }).join('') || `<p class="pair-item meta" style="cursor:default;color:var(--faint)">No folder pairs yet</p>`;
  $('#runAll').disabled = !!S.running || !S.pairs.some((p) => p.active);
}

// ---------- main ----------
function render() {
  renderSidebar();
  const main = $('#main');
  if (S.selected === 'runall') return (main.innerHTML = renderRunAll());
  const p = current();
  main.innerHTML = p ? renderPair(p) : renderEmpty();
}

function renderEmpty() {
  return `<div class="empty">
    <img src="logo.svg" width="72" height="72" alt="">
    <h1>Keep two folders in step</h1>
    <p>Create a folder pair — say your Pictures folder and a backup drive — then choose how changes should flow. You'll always see a preview before anything is copied or deleted.</p>
    <div class="mode-explainer">
      ${Object.values(MODE).map((m) => `<div><b>${m.glyph}&nbsp; ${m.label}</b><span>${m.desc}</span></div>`).join('')}
    </div>
    <button class="btn primary big" data-act="new">Create your first folder pair</button>
  </div>`;
}

function renderPair(p) {
  const v = view(p.id);
  const m = MODE[p.mode];
  const busyHere = S.running && S.running.ids.includes(p.id);
  const busy = !!S.running;
  const lr = p.last_run;
  const counts = v.plan ? { l: v.plan.left_count, r: v.plan.right_count } : null;
  return `
  <header class="pair-head">
    <div class="titles">
      <h1>${esc(p.name)}</h1>
      <div class="sub">${m.label}${p.schedule?.enabled ? ` · <span class="sched-badge">${CLOCK}${esc(describeSchedule(p.schedule))}</span>` : ''}${p.active ? '' : ' · not in Run all'}${lr ? ` · last run ${esc(ago(lr.at))}: ${esc(lr.summary)}` : ' · never run'}</div>
    </div>
    <div class="head-actions">
      <button class="btn" data-act="edit" ${busy ? 'disabled' : ''}>Edit</button>
      <button class="icon-btn" data-act="menu" aria-label="More actions" aria-expanded="${S.menuOpen}">
        <svg viewBox="0 0 16 16"><circle cx="3.5" cy="8" r=".6"/><circle cx="8" cy="8" r=".6"/><circle cx="12.5" cy="8" r=".6"/></svg>
      </button>
      ${S.menuOpen ? `<div class="menu" role="menu">
        ${p.schedule?.enabled ? `<button data-act="test-schedule" ${busy ? 'disabled' : ''}>Run the scheduled task now</button>` : ''}
        <button data-act="forget" ${busy ? 'disabled' : ''}>Forget sync history…</button>
        <button data-act="delete" class="danger" ${busy ? 'disabled' : ''}>Delete folder pair…</button>
      </div>` : ''}
    </div>
  </header>

  <section class="route">
    <div class="folder-card">
      <div class="label">Left <button class="link-btn" data-act="open" data-path="${esc(p.left)}">Open</button></div>
      <div class="path">${esc(p.left)}</div>
      ${counts ? `<div class="count">${counts.l.toLocaleString()} files</div>` : ''}
    </div>
    <div class="route-arrow ${p.mode}"><span class="arrow">${m.arrow}</span><span class="mode">${m.label}</span></div>
    <div class="folder-card">
      <div class="label">Right <button class="link-btn" data-act="open" data-path="${esc(p.right)}">Open</button></div>
      <div class="path">${esc(p.right)}</div>
      ${counts ? `<div class="count">${counts.r.toLocaleString()} files</div>` : ''}
    </div>
  </section>

  <div class="actions">
    <button class="btn big" data-act="preview" ${busy || v.previewing ? 'disabled' : ''}>
      <svg viewBox="0 0 16 16"><path d="M1.5 8s2.5-4.5 6.5-4.5S14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8z"/><circle cx="8" cy="8" r="2"/></svg>
      ${v.previewing ? 'Scanning…' : 'Preview'}
    </button>
    <button class="btn primary big" data-act="run" ${busy || v.previewing ? 'disabled' : ''}>
      <svg viewBox="0 0 16 16"><path d="M5 3.5v9l7-4.5z"/></svg>
      ${v.plan ? `Run ${selectedCount(v)} step${selectedCount(v) === 1 ? '' : 's'}` : 'Run'}
    </button>
    <span class="spacer"></span>
    <span class="muted">${esc(m.desc)}</span>
  </div>

  ${busyHere ? renderProgress() : v.error ? renderError(v.error) : v.report ? renderReport(v.report, p) : v.plan ? renderPlan(v) : renderHint(p)}`;
}

function renderHint(p) {
  return `<div class="panel hint-panel">
    <b>Preview</b> scans both folders and lists every copy, rename and delete before anything happens. Untick anything you don't want.
    ${p.last_run ? '' : '<br><br>The first run never deletes anything — Twinfold needs to see both folders once before it can tell a deleted file from a new one.'}
  </div>`;
}

function renderError(msg) {
  return `<div class="banner error"><svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.5"/><path d="M8 4.8v3.8M8 11v.2"/></svg><div class="grow">${esc(msg)}</div></div>`;
}

// ---------- plan ----------
const isToRight = (o) => (o.action === 'copy' || o.action === 'mkdir') && o.target === 'right';
const isToLeft = (o) => (o.action === 'copy' || o.action === 'mkdir') && o.target === 'left';
const isDelete = (o) => o.action === 'delete' || o.action === 'rmdir';
const FILTERS = {
  all: { label: 'All', test: () => true },
  right: { label: '→ Right', test: isToRight },
  left: { label: '← Left', test: isToLeft },
  rename: { label: 'Renames', test: (o) => o.action === 'rename' },
  delete: { label: 'Deletes', test: isDelete },
  conflict: { label: 'Conflicts', test: (o) => o.reason === 'conflict' || o.reason === 'restored' },
};

const selectedCount = (v) => (v.plan ? v.plan.ops.length - v.excluded.size : 0);

function visibleOps(v) {
  const f = FILTERS[v.filter] || FILTERS.all;
  const q = v.search.trim().toLowerCase();
  return v.plan.ops.filter((o) => f.test(o) && (!q || o.rel.toLowerCase().includes(q) || (o.from_rel || '').toLowerCase().includes(q)));
}

function actionCell(o) {
  const side = o.target;
  switch (o.action) {
    case 'copy': return `<span class="act copy">Copy <span class="tag">${side === 'right' ? '→ right' : '← left'}</span></span>`;
    case 'mkdir': return `<span class="act mkdir">New folder <span class="tag">${side === 'right' ? '→ right' : '← left'}</span></span>`;
    case 'rename': return `<span class="act rename">Rename <span class="tag">on ${side}</span></span>`;
    case 'delete': return `<span class="act delete">Delete <span class="tag">on ${side}</span></span>`;
    case 'rmdir': return `<span class="act rmdir">Delete folder <span class="tag">on ${side}</span></span>`;
  }
  return esc(o.action);
}

function whyCell(o) {
  const src = other(o.target);
  const base = {
    new: `New on the ${src}`,
    changed: `Changed on the ${src}`,
    conflict: '<span class="conflict">Conflict</span>',
    deleted: `Deleted on the ${src}`,
    renamed: `Renamed on the ${src}`,
    restored: '<span class="conflict">Kept</span>',
  }[o.reason] || esc(o.reason);
  return o.note ? `${base} — ${esc(o.note)}` : base;
}

function renderPlan(v) {
  const plan = v.plan;
  const ops = plan.ops;
  const banners = [];
  if (!plan.has_history) banners.push(`<div class="banner info"><svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.5"/><path d="M8 7.2V11M8 5v.2"/></svg><div class="grow">First run for this pair: nothing will be deleted. From the next run on, Twinfold remembers what both folders looked like and can follow deletes and renames.</div></div>`);
  if (plan.warnings.length) banners.push(`<div class="banner warn"><svg viewBox="0 0 16 16"><path d="M8 1.8 15 14H1z"/><path d="M8 6.3v3.4M8 11.6v.2"/></svg><div class="grow">${plan.warnings.length} item${plan.warnings.length === 1 ? '' : 's'} couldn't be read and will be left alone:<ul>${plan.warnings.slice(0, 8).map((w) => `<li>${esc(w)}</li>`).join('')}${plan.warnings.length > 8 ? `<li>…and ${plan.warnings.length - 8} more</li>` : ''}</ul></div></div>`);

  if (!ops.length) {
    return `${banners.join('')}<div class="banner ok"><svg viewBox="0 0 16 16"><path d="m3.5 8.5 3 3 6-7"/></svg><div class="grow"><b>Already in sync.</b> ${plan.left_count.toLocaleString()} files checked on each side${plan.skipped.length ? `, ${plan.skipped.length} left alone (below)` : ''}.</div></div>
      ${plan.skipped.length ? `<div class="panel">${renderSkips(plan.skipped)}</div>` : ''}`;
  }

  const shown = visibleOps(v);
  const page = shown.slice(0, v.limit);
  const allOn = shown.length > 0 && shown.every((o) => !v.excluded.has(o.id));
  const chip = (key) => {
    const n = key === 'all' ? ops.length : ops.filter(FILTERS[key].test).length;
    if (!n && key !== 'all') return '';
    return `<button class="chip ${v.filter === key ? 'on' : ''}" data-filter="${key}">${FILTERS[key].label} <b>${n.toLocaleString()}</b></button>`;
  };
  const copyBytes = ops.filter((o) => o.action === 'copy' && !v.excluded.has(o.id)).reduce((a, o) => a + o.size, 0);

  return `${banners.join('')}
  <div class="panel">
    <div class="chips">${Object.keys(FILTERS).map(chip).join('')}
      ${plan.skipped.length ? `<button class="chip ${v.filter === 'skipped' ? 'on' : ''}" data-filter="skipped">Left alone <b>${plan.skipped.length}</b></button>` : ''}
    </div>
    ${v.filter === 'skipped' ? renderSkips(plan.skipped) : `
    <div class="toolbar">
      <input type="search" id="opSearch" placeholder="Filter by path…" value="${esc(v.search)}" spellcheck="false">
      <span class="sel">${selectedCount(v).toLocaleString()} of ${ops.length.toLocaleString()} steps selected · ${bytes(copyBytes)} to copy</span>
    </div>
    <table class="ops">
      <colgroup><col class="c-check"><col class="c-act"><col><col class="c-size"><col class="c-why"></colgroup>
      <thead><tr>
        <th><input type="checkbox" id="toggleAll" ${allOn ? 'checked' : ''} aria-label="Select all shown"></th>
        <th>Action</th><th>Path</th><th style="text-align:right">Size</th><th>Why</th>
      </tr></thead>
      <tbody>
        ${page.map((o) => `<tr class="${v.excluded.has(o.id) ? 'off' : ''}">
          <td><input type="checkbox" data-op="${o.id}" ${v.excluded.has(o.id) ? '' : 'checked'} aria-label="Include this step"></td>
          <td>${actionCell(o)}</td>
          <td class="rel">${o.action === 'rename' ? `<span class="from">${esc(o.from_rel)} →</span>` : ''}${esc(o.rel)}</td>
          <td class="size">${o.action === 'copy' ? bytes(o.size) : ''}</td>
          <td class="why">${whyCell(o)}</td>
        </tr>`).join('')}
      </tbody>
    </table>
    ${shown.length > page.length ? `<div class="more">Showing ${page.length.toLocaleString()} of ${shown.length.toLocaleString()} · <button class="link-btn" data-act="more">Show more</button></div>` : ''}
    ${!shown.length ? `<div class="more">Nothing matches.</div>` : ''}`}
  </div>`;
}

function renderSkips(skips) {
  return `<ul class="skips">${skips.slice(0, 500).map((s) => `<li><span class="rel">${esc(s.rel)}</span> — <span class="why">${esc(s.note)}</span></li>`).join('')}</ul>`;
}

// ---------- progress & reports ----------
function renderProgress() {
  const pr = S.running.progress;
  const pair = pr && S.pairs.find((p) => p.id === pr.pairId);
  const multi = S.running.ids.length > 1;
  let pct = 0;
  let line = 'Scanning folders…';
  let indeterminate = true;
  if (pr && pr.phase === 'copying' && pr.total > 0) {
    indeterminate = false;
    pct = pr.bytesTotal > 0 ? (pr.bytesDone / pr.bytesTotal) * 100 : (pr.done / pr.total) * 100;
    line = `${pr.done.toLocaleString()} of ${pr.total.toLocaleString()} steps${pr.bytesTotal ? ` · ${bytes(pr.bytesDone)} of ${bytes(pr.bytesTotal)}` : ''}`;
  }
  return `<div class="panel progress-panel">
    <div class="top"><b>${multi && pair ? `${esc(pair.name)} (${pr.pairIndex + 1} of ${pr.pairCount})` : 'Syncing'}</b><span class="muted">${line}</span></div>
    <div class="bar ${indeterminate ? 'indeterminate' : ''}"><i style="width:${indeterminate ? '' : Math.min(100, pct).toFixed(1) + '%'}"></i></div>
    <div class="current">${esc(pr?.current || '')}</div>
    <div style="margin-top:16px;text-align:right"><button class="btn" data-act="cancel">Cancel</button></div>
  </div>`;
}

function renderReport(r, p) {
  const ok = !r.failure && !r.errors.length && !r.cancelled;
  const icon = ok
    ? '<svg viewBox="0 0 16 16" style="color:var(--ok)"><path d="m3.5 8.5 3 3 6-7"/></svg>'
    : '<svg viewBox="0 0 16 16" style="color:var(--danger)"><circle cx="8" cy="8" r="6.5"/><path d="M8 4.8v3.8M8 11v.2"/></svg>';
  if (r.failure) return renderError(r.failure);
  const c = r.counts;
  const secs = (r.duration_ms / 1000).toFixed(1);
  return `<div class="panel report">
    <h3>${icon}${r.cancelled ? 'Cancelled' : ok ? 'Done' : 'Finished with problems'}</h3>
    <div class="muted" style="color:var(--muted)">${esc(p ? p.last_run?.summary ?? '' : '')} · ${secs}s</div>
    <div class="stats">
      <div class="stat"><b>${c.copied.toLocaleString()}</b><span>copied · ${bytes(r.bytes)}</span></div>
      <div class="stat"><b>${c.renamed.toLocaleString()}</b><span>renamed</span></div>
      <div class="stat"><b>${c.deleted.toLocaleString()}</b><span>deleted</span></div>
      <div class="stat"><b>${(r.errors.length + r.skipped.length).toLocaleString()}</b><span>failed or left alone</span></div>
    </div>
    ${r.errors.length ? `<h4>Failed</h4><ul>${r.errors.slice(0, 200).map((e) => `<li><span class="rel">${esc(e.rel)}</span> — <span class="err">${esc(e.message)}</span></li>`).join('')}</ul>` : ''}
    ${r.skipped.length ? `<h4>Left alone</h4><ul>${r.skipped.slice(0, 200).map((s) => `<li><span class="rel">${esc(s.rel)}</span> — ${esc(s.note)}</li>`).join('')}</ul>` : ''}
    ${r.warnings.length ? `<h4>Warnings</h4><ul>${r.warnings.slice(0, 50).map((w) => `<li>${esc(w)}</li>`).join('')}</ul>` : ''}
  </div>`;
}

function renderRunAll() {
  if (S.running) return `<header class="pair-head"><div class="titles"><h1>Running all active pairs</h1></div></header>${renderProgress()}`;
  const reps = S.runAllReports || [];
  return `<header class="pair-head"><div class="titles"><h1>Run all — results</h1><div class="sub">${reps.length} folder pair${reps.length === 1 ? '' : 's'}</div></div></header>
    <div class="panel">
      <table class="ops"><colgroup><col style="width:34px"><col style="width:28%"><col><col style="width:80px"></colgroup>
      <tbody>${reps.map((r) => {
        const ok = !r.failure && !r.errors.length && !r.cancelled;
        const p = S.pairs.find((x) => x.id === r.pair_id);
        return `<tr><td><span class="dot ${ok ? 'ok' : 'bad'}" style="display:inline-block;margin-top:6px"></span></td>
          <td><button class="link-btn" style="font-weight:600;color:var(--text);padding:0" data-id="${esc(r.pair_id)}">${esc(r.pair_name)}</button></td>
          <td class="why">${esc(p?.last_run?.summary || r.failure || '')}</td>
          <td class="size">${(r.duration_ms / 1000).toFixed(1)}s</td></tr>`;
      }).join('')}</tbody></table>
    </div>`;
}

// ---------- actions ----------
async function doPreview(p) {
  const v = view(p.id);
  v.previewing = true; v.error = null; v.report = null;
  render();
  try {
    v.plan = await api.invoke('preview', { id: p.id });
    v.excluded = new Set(); v.filter = 'all'; v.search = ''; v.limit = PAGE;
  } catch (e) {
    v.plan = null; v.error = String(e);
  }
  v.previewing = false;
  render();
}

async function doRun(ids) {
  const excluded = {};
  for (const id of ids) {
    const v = view(id);
    if (v.plan) excluded[id] = [...v.excluded];
    v.error = null; v.report = null;
  }
  S.running = { ids, progress: null };
  S.menuOpen = false;
  render();
  let reports = [];
  try {
    reports = await api.invoke('run', { ids, excluded });
  } catch (e) {
    toast(String(e));
  }
  S.running = null;
  for (const r of reports) {
    const v = view(r.pair_id);
    v.plan = null; v.excluded = new Set();
    if (r.failure) v.error = r.failure; else v.report = r;
  }
  if (ids.length > 1) S.runAllReports = reports;
  await loadPairs();
  if (ids.length > 1) {
    const bad = reports.filter((r) => r.failure || r.errors.length).length;
    toast(bad ? `${bad} of ${reports.length} pairs had problems` : `All ${reports.length} pairs synced`);
  }
}

// ---------- pair dialog ----------
const form = $('#pairForm');
let editingId = '';

function openPairDialog(p) {
  editingId = p?.id || '';
  $('#pairDialogTitle').textContent = p ? `Edit “${p.name}”` : 'New folder pair';
  const o = p?.options || { include: [], exclude: [], skip_hidden: false, skip_system: true, skip_read_only: false, compare_contents: false, use_recycle_bin: true };
  form.name.value = p?.name || '';
  form.left.value = p?.left || '';
  form.right.value = p?.right || '';
  form.mode.value = p?.mode || 'synchronize';
  form.include.value = o.include.join('\n');
  form.exclude.value = o.exclude.join('\n');
  for (const k of ['skip_hidden', 'skip_system', 'skip_read_only', 'compare_contents', 'use_recycle_bin']) form[k].checked = !!o[k];
  form.active.checked = p ? p.active : true;
  const s = p?.schedule || { enabled: false, repeat: 'daily', time: '02:00', days: [1, 2, 3, 4, 5], every_hours: 4, wake: false };
  form.sched_enabled.checked = s.enabled;
  form.sched_repeat.value = s.repeat;
  form.sched_time.value = s.time;
  form.sched_every.value = String(s.every_hours);
  form.sched_wake.checked = s.wake;
  for (const box of form.querySelectorAll('[name=sched_day]')) box.checked = s.days.includes(+box.value);
  $('#scheduleSection').open = s.enabled;
  updateScheduleFields();
  $('.advanced:not(#scheduleSection)').open = !!(o.include.length || o.exclude.length);
  $('#pairError').hidden = true;
  $('#pairDialog').showModal();
  form.name.focus();
}

const lines = (s) => s.split(/\r?\n|,/).map((x) => x.trim()).filter(Boolean);

function formSchedule() {
  return {
    enabled: form.sched_enabled.checked,
    repeat: form.sched_repeat.value,
    time: form.sched_time.value || '02:00',
    days: [...form.querySelectorAll('[name=sched_day]:checked')].map((b) => +b.value),
    every_hours: +form.sched_every.value,
    wake: form.sched_wake.checked,
  };
}

function updateScheduleFields() {
  const s = formSchedule();
  $('#schedFields').hidden = !s.enabled;
  for (const el of form.querySelectorAll('[data-for]')) el.hidden = !el.dataset.for.split(' ').includes(s.repeat);
  const timeLabel = form.querySelector('[data-label-hourly]');
  timeLabel.textContent = s.repeat === 'hourly' ? timeLabel.dataset.labelHourly : 'At';
  $('#scheduleSummary').textContent = s.enabled ? `· ${describeSchedule(s)}` : '· off';
}
form.addEventListener('change', (e) => { if (e.target.name?.startsWith('sched_')) updateScheduleFields(); });
form.addEventListener('input', (e) => { if (e.target.name === 'sched_time') updateScheduleFields(); });

form.addEventListener('submit', async (e) => {
  if (e.submitter?.value !== 'save') return;
  e.preventDefault();
  const old = S.pairs.find((p) => p.id === editingId);
  const pair = {
    id: editingId,
    name: form.name.value,
    left: form.left.value,
    right: form.right.value,
    mode: form.mode.value,
    active: form.active.checked,
    last_run: old?.last_run ?? null,
    schedule: formSchedule(),
    options: {
      include: lines(form.include.value),
      exclude: lines(form.exclude.value),
      skip_hidden: form.skip_hidden.checked,
      skip_system: form.skip_system.checked,
      skip_read_only: form.skip_read_only.checked,
      compare_contents: form.compare_contents.checked,
      use_recycle_bin: form.use_recycle_bin.checked,
    },
  };
  const btn = $('#pairSave');
  btn.disabled = true;
  try {
    const { pair: saved, warning } = await api.invoke('save_pair', { pair });
    $('#pairDialog').close();
    S.selected = saved.id;
    const v = view(saved.id);
    v.plan = null; v.report = null; v.error = null;
    await loadPairs();
    if (warning) toast(warning);
    else if (saved.schedule.enabled && JSON.stringify(saved.schedule) !== JSON.stringify(old?.schedule)) toast(`Scheduled: ${describeSchedule(saved.schedule)}`);
  } catch (err) {
    const el = $('#pairError');
    el.textContent = String(err);
    el.hidden = false;
  } finally {
    btn.disabled = false;
  }
});

form.addEventListener('click', async (e) => {
  const side = e.target.closest('[data-browse]')?.dataset.browse;
  if (!side) return;
  try {
    const picked = await api.pick();
    if (picked) {
      form[side].value = picked;
      if (!form.name.value && side === 'left') form.name.value = picked.split(/[\\/]/).filter(Boolean).pop() || '';
    }
  } catch (err) {
    toast(String(err));
  }
});

// ---------- events ----------
$('#newPair').addEventListener('click', () => openPairDialog(null));
$('#runAll').addEventListener('click', () => {
  const ids = S.pairs.filter((p) => p.active).map((p) => p.id);
  if (!ids.length) return;
  S.selected = 'runall';
  S.runAllReports = null;
  doRun(ids);
});
$('#openLog').addEventListener('click', () => api.invoke('open_log').catch((e) => toast(String(e))));

// ---------- settings ----------
const settingsForm = $('#settingsForm');
$('#openSettings').addEventListener('click', async () => {
  try {
    const s = await api.invoke('get_settings');
    settingsForm.close_to_tray.checked = !!s.close_to_tray;
    settingsForm.start_with_windows.checked = !!s.start_with_windows;
  } catch (e) {
    return toast(String(e));
  }
  $('#settingsError').hidden = true;
  $('#settingsDialog').showModal();
});
settingsForm.addEventListener('submit', async (e) => {
  if (e.submitter?.value !== 'save') return;
  e.preventDefault();
  try {
    await api.invoke('save_settings', {
      settings: { close_to_tray: settingsForm.close_to_tray.checked, start_with_windows: settingsForm.start_with_windows.checked },
    });
    $('#settingsDialog').close();
    toast('Settings saved');
  } catch (err) {
    $('#settingsError').textContent = String(err);
    $('#settingsError').hidden = false;
  }
});

// Scheduled runs happen in a separate process; pick up their results.
function watchForResult(id, before) {
  let tries = 0;
  const timer = setInterval(async () => {
    tries++;
    if (S.running) return;
    await loadPairs();
    const p = S.pairs.find((x) => x.id === id);
    if ((p && p.last_run?.at !== before) || tries > 600) clearInterval(timer);
  }, 2000);
}
window.addEventListener('focus', () => { if (!S.running) loadPairs(); });
api.listen('tray-run-all', () => { if (!S.running) $('#runAll').click(); });

$('#pairList').addEventListener('click', (e) => {
  const id = e.target.closest('[data-id]')?.dataset.id;
  if (!id) return;
  S.selected = id;
  S.menuOpen = false;
  render();
});

const main = $('#main');
main.addEventListener('click', async (e) => {
  const p = current();
  const jump = e.target.closest('[data-id]');
  if (jump && S.selected === 'runall') { S.selected = jump.dataset.id; return render(); }

  const filter = e.target.closest('[data-filter]')?.dataset.filter;
  if (filter && p) { const v = view(p.id); v.filter = filter; v.limit = PAGE; return render(); }

  const act = e.target.closest('[data-act]')?.dataset.act;
  if (!act) {
    if (S.menuOpen) { S.menuOpen = false; render(); }
    return;
  }
  if (act !== 'menu') S.menuOpen = false;
  switch (act) {
    case 'new': return openPairDialog(null);
    case 'edit': return p && openPairDialog(p);
    case 'menu': S.menuOpen = !S.menuOpen; return render();
    case 'open': return api.invoke('open_folder', { path: e.target.closest('[data-path]').dataset.path }).catch((err) => toast(String(err)));
    case 'preview': return p && doPreview(p);
    case 'run': return p && doRun([p.id]);
    case 'cancel': return api.invoke('cancel');
    case 'test-schedule':
      render();
      try {
        await api.invoke('run_scheduled_now', { id: p.id });
        toast('Task Scheduler started the sync in the background. The result shows here when it finishes.');
        watchForResult(p.id, p.last_run?.at);
      } catch (err) {
        toast(String(err));
      }
      return;
    case 'more': { const v = view(p.id); v.limit += PAGE; return render(); }
    case 'forget':
      render();
      if (p && await confirmBox('Forget sync history?', `Twinfold will treat the next run of “${p.name}” like a first run: it merges both folders and deletes nothing. Use this after restoring a backup or reconnecting a drive under a new letter.`, 'Forget history')) {
        await api.invoke('reset_history', { id: p.id }).catch((err) => toast(String(err)));
        Object.assign(view(p.id), { plan: null, report: null, error: null });
        toast('Sync history forgotten');
        render();
      }
      return;
    case 'delete':
      render();
      if (p && await confirmBox(`Delete “${p.name}”?`, 'This removes the folder pair from Twinfold. Your files in both folders stay exactly where they are.', 'Delete pair')) {
        await api.invoke('delete_pair', { id: p.id }).catch((err) => toast(String(err)));
        delete S.views[p.id];
        S.selected = null;
        await loadPairs();
      }
      return;
  }
});

main.addEventListener('change', (e) => {
  const p = current();
  if (!p) return;
  const v = view(p.id);
  if (e.target.id === 'toggleAll') {
    for (const o of visibleOps(v)) e.target.checked ? v.excluded.delete(o.id) : v.excluded.add(o.id);
    return render();
  }
  const id = e.target.dataset.op;
  if (id !== undefined) {
    e.target.checked ? v.excluded.delete(+id) : v.excluded.add(+id);
    render();
  }
});

main.addEventListener('input', (e) => {
  if (e.target.id !== 'opSearch') return;
  const v = view(current().id);
  v.search = e.target.value;
  v.limit = PAGE;
  const pos = e.target.selectionStart;
  render();
  const box = $('#opSearch');
  box.focus();
  box.setSelectionRange(pos, pos);
});

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && S.menuOpen) { S.menuOpen = false; render(); }
  if (e.key === 'F5' && current() && !S.running) { e.preventDefault(); doPreview(current()); }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'n' && !S.running) { e.preventDefault(); openPairDialog(null); }
});

api.listen('progress', (ev) => {
  if (!S.running) return;
  S.running.progress = ev.payload;
  // Repaint just the bits that change, not the whole page.
  const sel = S.selected;
  if (sel === 'runall' || S.running.ids.includes(sel)) {
    const panel = $('.progress-panel');
    if (panel) panel.outerHTML = renderProgress();
    else render();
  }
  renderSidebar();
});

loadPairs();
// Keep "5 min ago" fresh and pick up scheduled runs that finished while we were open.
setInterval(() => (S.running ? renderSidebar() : loadPairs()), 60_000);
