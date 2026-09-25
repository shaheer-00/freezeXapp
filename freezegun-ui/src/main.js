// ── FREEGUN · cyberdeck frontend ─────────────────────────────────────────
// Uses injected Tauri globals (withGlobalTauri: true) — no bundler.

const { invoke } = window.__TAURI__.core;
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);
const tlist   = $('tlist');
const logEl   = $('log');
const cmdIn   = $('cmd-input');
const filIn   = $('filter-input');
const statusEl= $('status-msg');

const OFFSET_100NS = -3600_000 * 10_000;   // −1h

let procs       = [];
let selPid      = null;
let filter      = '';
let netSet      = new Set();
let frozenSet   = new Set();
let selSet      = new Set();      // bulk-selection set
let dllPath     = '';
let installed   = [];
let history     = [];
let histIdx     = -1;
let msgTimer    = null;
const DELTA_KEY = 'fz_deltas';    // localStorage { "name": "-1:00" }

// ── log ──────────────────────────────────────────────────────────────────
const pad = (n) => String(n).padStart(2, '0');
const stamp = () => { const d = new Date(); return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`; };

function log(msg, kind = 'mu') {
  const ln = document.createElement('div');
  ln.className = 'ln';
  const ts = document.createElement('span'); ts.className = 'ts'; ts.textContent = `[${stamp()}] `;
  const bd = document.createElement('span'); bd.className = kind; bd.textContent = msg;
  ln.append(ts, bd);
  logEl.appendChild(ln);
  while (logEl.childElementCount > 500) logEl.removeChild(logEl.firstChild);
  logEl.scrollTop = logEl.scrollHeight;
}

function say(msg, isErr = false) {
  statusEl.textContent = msg;
  statusEl.className = 'msg' + (isErr ? ' er' : '');
  clearTimeout(msgTimer);
  msgTimer = setTimeout(() => { statusEl.textContent = 'ready'; statusEl.className = 'msg'; }, 4200);
}

function flash(el) {
  el.classList.remove('flash');
  void el.offsetWidth;
  el.classList.add('flash');
}

// ── render ───────────────────────────────────────────────────────────────
const visible = () => filter
  ? procs.filter(p => p.name.toLowerCase().includes(filter.toLowerCase()))
  : procs;

const visibleApps = () => filter
  ? installed.filter(a => a.name.toLowerCase().includes(filter.toLowerCase()))
  : installed;

function buildAppRow(a) {
  const row = document.createElement('div');
  row.className = 'trow approw';
  row.dataset.path = a.path;

  const cb = document.createElement('input');
  cb.type = 'checkbox';
  cb.className = 'bulk';
  cb.disabled = true;

  const chev = document.createElement('div'); chev.className = 'chev'; chev.textContent = '▸';
  const led = document.createElement('div'); led.className = 'led';

  const nm = document.createElement('div'); nm.className = 'nm';
  nm.title = a.path;
  const nmText = document.createElement('span'); nmText.textContent = a.name;
  nm.append(nmText);

  const tags = document.createElement('div'); tags.className = 'tags';

  const chips = document.createElement('div'); chips.className = 'chips';
  const mk = (cls, label, act) => {
    const b = document.createElement('button');
    b.className = 'act ' + cls;
    b.textContent = label;
    b.dataset.act = act;
    b.dataset.path = a.path;
    chips.appendChild(b);
  };
  mk('frz', 'FREEZE', 'freeze-app');
  mk('inj', 'INJECT', 'inj-app');
  mk('net', 'NET', 'net-app');
  mk('', 'LAUNCH', 'launch-app');

  row.append(cb, chev, led, nm, tags, chips);
  return row;
}

function buildRow(p) {
  const frozen = p.frozen || frozenSet.has(p.pid);
  const netcut = netSet.has(p.pid);
  const sel = selSet.has(p.pid);

  const row = document.createElement('div');
  row.className = 'trow' + (p.pid === selPid ? ' sel' : '')
                        + (frozen ? ' frozen' : '')
                        + (netcut ? ' netcut' : '');
  row.dataset.pid = p.pid;

  const cb = document.createElement('input');
  cb.type = 'checkbox';
  cb.className = 'bulk';
  cb.checked = sel;
  cb.addEventListener('change', (e) => {
    if (e.target.checked) selSet.add(p.pid); else selSet.delete(p.pid);
    render(); updateBulkBar();
  });
  row.appendChild(cb);

  const chev = document.createElement('div'); chev.className = 'chev'; chev.textContent = '▸';

  const led = document.createElement('div'); led.className = 'led';

  const nm = document.createElement('div'); nm.className = 'nm';
  nm.title = p.exe || p.name;
  const nmText = document.createElement('span'); nmText.textContent = p.name;
  const pidv = document.createElement('span'); pidv.className = 'pidv'; pidv.textContent = p.pid;
  nm.append(nmText, pidv);

  const tags = document.createElement('div'); tags.className = 'tags';
  if (frozen) { const t = document.createElement('span'); t.className = 'tag ice'; t.textContent = 'FROZEN'; tags.appendChild(t); }
  if (netcut) { const t = document.createElement('span'); t.className = 'tag cut'; t.textContent = 'CUT'; tags.appendChild(t); }

  const chips = document.createElement('div'); chips.className = 'chips';
  const mk = (cls, label, act) => {
    const b = document.createElement('button');
    b.className = 'act ' + cls + ((act === 'net' && netcut) ? ' on' : '');
    b.textContent = label;
    b.dataset.act = act;
    chips.appendChild(b);
  };
  mk('frz', frozen ? 'UNFRZ' : 'FREEZE', frozen ? 'unf' : 'frz');
  mk('inj', 'INJECT', 'inj');
  mk('net', 'NET', 'net');

  row.append(chev, led, nm, tags, chips);
  return row;
}

function render() {
  const vis = visible();
  tlist.replaceChildren();
  vis.forEach(p => {
    const r = buildRow(p);
    r.addEventListener('click', (e) => {
      if (e.target.classList.contains('bulk')) return; // checkbox handles own logic
      selPid = p.pid; render(); telemetry();
    });
    tlist.appendChild(r);
  });
  $('empty').classList.toggle('hidden', vis.length !== 0);
  $('counts').textContent = String(vis.length);

  // — installed apps panel —
  const applist = $('applist');
  applist.replaceChildren();
  const visApps = visibleApps();
  visApps.forEach(a => {
    const r = buildAppRow(a);
    applist.appendChild(r);
  });
  $('apps-split').classList.toggle('hidden', visApps.length === 0);

  const fz = procs.filter(p => p.frozen || frozenSet.has(p.pid)).length;
  const total = Math.max(1, procs.length);

  $('st-frozen').querySelector('span').textContent = String(fz);
  $('st-frozen').classList.toggle('hot', fz > 0);
  $('st-net').querySelector('span').textContent = String(netSet.size);
  $('st-net').classList.toggle('hot', netSet.size > 0);

  // frozen-bar chip
  $('m-frozen-n2').textContent = String(fz);
  $('m-frozen2').style.width = `${Math.min(100, (fz / total) * 100)}%`;

  $('m-frozen').style.width = `${Math.min(100, (fz / total) * 100)}%`;
  $('m-frozen-n').textContent = String(fz);
  $('m-net').style.width = `${Math.min(100, (netSet.size / total) * 100)}%`;
  $('m-net-n').textContent = String(netSet.size);

  // update sel-all checkbox state
  const selAll = $('sel-all');
  if (selAll) {
    const vis = visible();
    selAll.indeterminate = selSet.size > 0 && selSet.size < vis.length;
    selAll.checked = vis.length > 0 && vis.every(p => selSet.has(p.pid));
  }
}

// ── bulk select ─────────────────────────────────────────────────────────────
function updateBulkBar() {
  const n = selSet.size;
  $('bulk-bar').classList.toggle('hidden', n === 0);
  $('bulk-count').textContent = `${n} selected`;
}

async function bulkAction(act) {
  const pids = [...selSet];
  if (pids.length === 0) return;
  for (const pid of pids) {
    switch (act) {
      case 'freeze': openFreezeDialog(pid); break;
      case 'unfreeze': await doUnfreeze(pid); break;
      case 'inject': await doInject(pid); break;
      case 'net': await doNet(pid); break;
    }
  }
  if (act !== 'freeze') await refresh();
  selSet.clear();
  updateBulkBar();
  render();
}

$('sel-all').addEventListener('change', (e) => {
  const vis = visible();
  if (e.target.checked) vis.forEach(p => selSet.add(p.pid));
  else selSet.clear();
  render(); updateBulkBar();
});

$('bulk-freeze').addEventListener('click', () => bulkAction('freeze'));
$('bulk-unfreeze').addEventListener('click', () => bulkAction('unfreeze'));
$('bulk-inject').addEventListener('click', () => bulkAction('inject'));
$('bulk-net').addEventListener('click', () => bulkAction('net'));
$('bulk-clear').addEventListener('click', () => { selSet.clear(); render(); updateBulkBar(); });

function sel() { return procs.find(p => p.pid === selPid) || null; }

function telemetry() {
  const p = sel();
  $('t-target').textContent = p ? `${p.name} · ${p.pid}` : '—';
  const pm = p && (p.frozen_mode || (frozenSet.has(p.pid) ? 'offset' : ''));
  $('t-mode').textContent = p && pm
    ? (pm === 'abs' ? 'INTERDICT (dead-stop)' : 'INTERDICT (shifted)')
    : (p ? 'PASSTHROUGH' : '—');
  const d = new Date();
  $('t-clock').textContent = `${d.toISOString().slice(0, 19)}Z`;
  $('t-dll').textContent = dllPath ? dllPath.split('\\').pop() : '…';
}

// ── backend ──────────────────────────────────────────────────────────────
async function refresh() {
  try {
    procs = await invoke('refresh_processes');
    frozenSet = new Set(procs.filter(p => p.frozen).map(p => p.pid));
    if (selPid === null && procs.length) selPid = procs[0].pid;
    const live = new Set(procs.map(p => p.pid));
    for (const pid of [...netSet]) if (!live.has(pid)) netSet.delete(pid);
    telemetry();
  } catch (e) { log(`rescan failed: ${e}`, 'er'); }

  // fetch installed apps (best-effort — don't block render on failure)
  try { installed = await invoke('list_installed_apps'); }
  catch (e) { log(`app scan: ${e}`, 'wa'); }

  render();
}

async function doFreeze(pid) {
  // CLI path (from `freeze <pid>` command) keeps the original −1h offset freeze.
  // UI FREEZE chip → modal dialog (openFreezeDialog).
  const p = procs.find(x => x.pid === pid); if (!p) return;
  try {
    await invoke('inject', { pid });
    await invoke('freeze', { req: { pid, delta_100ns: OFFSET_100NS } });
    frozenSet.add(pid);
    log(`interdict pid=${pid} ${p.name} Δ−1h`, 'ok');
    say(`interdicted ${p.name}`);
    flash(document.querySelector(`.trow[data-pid="${pid}"]`));
  } catch (e) { log(`interdict failed pid=${pid}: ${e}`, 'er'); say('failed — try elevated', true); }
  await refresh();
}

// ── freeze confirm modal ────────────────────────────────────────────────────
let fmPid = null;
let fmLaunch = null; // path to launch before freezing (installed-app flow)
let fmClockTimer = null;

function fmtLocalParts(d) {
  // browser local breakdown — what the human types in HOLD AT
  const pad = (n) => String(n).padStart(2, '0');
  return {
    year: d.getFullYear(), month: d.getMonth() + 1, day: d.getDate(),
    hour: d.getHours(), minute: d.getMinutes(), second: d.getSeconds(), millis: 0,
    zone: (d.getTimezoneOffset() <= 0 ? '+' : '-') +
      String(Math.abs(d.getTimezoneOffset())/60).padStart(2,'0') + ':' +
      String(Math.abs(d.getTimezoneOffset())%60).padStart(2,'0'),
  };
}

function lastDelta(name) {
  try { return (JSON.parse(localStorage.getItem(DELTA_KEY)) || {})[String(name).toLowerCase()] || '-1:00'; }
  catch { return '-1:00'; }
}
function saveDelta(name, val) {
  try {
    const m = JSON.parse(localStorage.getItem(DELTA_KEY)) || {};
    m[String(name).toLowerCase()] = val;
    localStorage.setItem(DELTA_KEY, JSON.stringify(m));
  } catch {}
}

function fmUpdateClock() {
  const fmp = $('fm-clock');
  const n = new Date();
  fmp.textContent = `${n.toLocaleString()}`;
}

function openFreezeDialog(pid, launchPath) {
  const p = procs.find(x => x.pid === pid);
  fmPid = pid ?? null;
  fmLaunch = launchPath || null;
  if (p) {
    $('fm-target').textContent = `${p.name} · pid ${p.pid}`;
  } else if (fmLaunch) {
    const stem = fmLaunch.split(/[\\/]/).pop() || fmLaunch;
    $('fm-target').textContent = `${stem} · (pending launch)`;
  } else {
    $('fm-target').textContent = '—';
  }
  $('fm-clock').textContent = new Date().toLocaleString();
  // SHIFT delta: remember last per app name, else −1h
  const keyName = p ? p.name : (fmLaunch ? fmLaunch.split(/[\\/]/).pop() : '');
  $('fm-delta').value = keyName ? lastDelta(keyName) : '-1:00';
  $('fm-date').valueAsDate = null;
  $('fm-time').value = '';
  // mode defaults
  const dead = document.querySelector('input[name="fm-mode"][value="dead"]');
  dead.checked = true;
  fmToggleFields();
  $('freeze-modal').classList.remove('hidden');
  clearInterval(fmClockTimer);
  fmClockTimer = setInterval(fmUpdateClock, 500);
  fmUpdateClock();
}

function fmToggleFields() {
  const mode = document.querySelector('input[name="fm-mode"]:checked').value;
  $('fm-dt').classList.toggle('hidden', mode !== 'hold');
  $('fm-shift').classList.toggle('hidden', mode !== 'shift');
}

function closeFreezeModal() {
  clearInterval(fmClockTimer);
  $('freeze-modal').classList.add('hidden');
  fmPid = null;
  fmLaunch = null;
}

// Resolve the pid to freeze, launching the app first if the dialog was opened
// from the installed-apps panel (fmLaunch set, pid unknown).
async function resolveFmPid() {
  if (fmPid !== null) return fmPid;
  if (!fmLaunch) return null;
  if (!(await doLaunch(fmLaunch))) return null;
  // wait for the launched app to appear in the process list
  for (let i = 0; i < 30; i++) {
    await new Promise(r => setTimeout(r, 100));
    await refresh();
    const p = procs.find(p => p.exe.toLowerCase() === fmLaunch.toLowerCase());
    if (p) return p.pid;
  }
  return null;
}

async function fmEngage() {
  const mode = document.querySelector('input[name="fm-mode"]:checked').value;
  const pid = await resolveFmPid();
  if (pid === null) return say('target not running — launch failed', true);
  const p = procs.find(x => x.pid === pid); if (!p) return closeFreezeModal();
  try {
    if (mode === 'dead') {
      // halt at now → freeze_absolute with live local breakdown
      const parts = fmtLocalParts(new Date());
      await invoke('inject', { pid: p.pid });
      await invoke('freeze_abs', {
        req: { pid: p.pid, year: parts.year, month: parts.month, day: parts.day,
          hour: parts.hour, minute: parts.minute, second: parts.second, millis: parts.millis }
      });
      frozenSet.add(p.pid);
      log(`dead-stop pid=${p.pid} ${p.name} halt@now`, 'ok');
      say(`frozen ${p.name}`);
    } else if (mode === 'hold') {
      const dv = $('fm-date').value, tv = $('fm-time').value;
      if (!dv || !tv) return say('pick date + time', true);
      const dt = new Date(`${dv}T${tv}:00`);
      if (Number.isNaN(dt.getTime())) return say('invalid time', true);
      const parts = fmtLocalParts(dt);
      await invoke('inject', { pid: p.pid });
      await invoke('freeze_abs', {
        req: { pid: p.pid, year: parts.year, month: parts.month, day: parts.day,
          hour: parts.hour, minute: parts.minute, second: parts.second, millis: parts.millis }
      });
      frozenSet.add(p.pid);
      log(`hold-at pid=${p.pid} ${p.name} @${dv} ${tv}`, 'ok');
      say(`frozen ${p.name} at ${dv} ${tv}`);
    } else if (mode === 'shift') {
      const delta = parseDelta($('fm-delta').value);
      if (delta === null) return say('bad offset (hh:mm)', true);
      await invoke('inject', { pid: p.pid });
      await invoke('freeze', { req: { pid: p.pid, delta_100ns: delta } });
      frozenSet.add(p.pid);
      log(`shift pid=${p.pid} ${p.name} Δ${$('fm-delta').value}`, 'ok');
      say(`frozen ${p.name} offset ${$('fm-delta').value}`);
    }
    flash(document.querySelector(`.trow[data-pid="${p.pid}"]`));
    try { await invoke('save_settings'); } catch {}
  } catch (e) {
    log(`interdict failed pid=${p.pid}: ${e}`, 'er');
    say('failed — try elevated', true);
  }
  closeFreezeModal();
  await refresh();
}

function parseDelta(s) {
  // "−1:30", "-1:30", "+2:00" → 100ns units
  const m = String(s).trim().match(/^([+-]?\d+):(\d{1,2})$/);
  if (!m) return null;
  let h = parseInt(m[1], 10), min = parseInt(m[2], 10);
  const sign = s.trim().startsWith('-') ? -1 : 1;
  h = Math.abs(h);
  const totalNs = (h * 3600 + min * 60) * 1_000_000_000;
  return sign * totalNs / 100; // → 100ns ticks
}

// ── frozen targets panel ─────────────────────────────────────────────────────
let frozenSort = { key: 'name', dir: 1 };

async function refresh_frozen() {
  try {
    let frozen = await invoke('frozen_targets');
    // sort
    const { key, dir } = frozenSort;
    frozen.sort((a, b) => {
      let av = a[key], bv = b[key];
      if (typeof av === 'string' && typeof bv === 'string')
        av = av.toLowerCase(), bv = bv.toLowerCase();
      return av < bv ? -dir : av > bv ? dir : 0;
    });
    const flist = $('flist');
    flist.replaceChildren();
    if (frozen.length === 0) { $('fempty').classList.remove('hidden'); return; }
    $('fempty').classList.add('hidden');
    frozen.forEach(t => {
      const row = document.createElement('div');
      row.className = 'trow froz-row';
      row.dataset.pid = t.pid;
      // left padding for checkbox column spacing
      const pad = document.createElement('span'); pad.className = 'pad';

      const nm = document.createElement('div'); nm.className = 'nm';
      const n = document.createElement('span'); n.textContent = t.name;
      const pidv = document.createElement('span'); pidv.className = 'pidv'; pidv.textContent = t.pid;
      nm.append(n, pidv);

      const tag = document.createElement('span'); tag.className = 'tag ice';
      tag.textContent = t.frozen_mode === 'abs' ? 'DEAD' : 'SHIFT';
      tag.title = 'click to toggle DEAD ↔ SHIFT';
      tag.addEventListener('click', async (e) => {
        e.stopPropagation();
        const next = t.frozen_mode === 'abs' ? 'offset' : 'abs';
        if (next === 'offset') {
          await invoke('retune', { pid: t.pid, mode: 'offset', delta_100ns: t.offset_100ns });
        } else {
          // switch back to abs: reconstruct from stored components
          await invoke('retune', { pid: t.pid, mode: 'abs', delta_100ns: 0 });
        }
        log(`retune pid=${t.pid} → ${next}`, 'mu');
        refresh_frozen();
      });

      const at = document.createElement('span'); at.className = 'tag';
      at.textContent = t.frozen_at;
      at.title = 'frozen-at (target sees this)';

      const chips = document.createElement('div'); chips.className = 'chips';
      const b = document.createElement('button');
      b.className = 'act unfrz'; b.textContent = 'UNFREEZE';
      b.dataset.pid = t.pid;
      chips.appendChild(b);
      // recover chip (last-resort terminate)
      const rc = document.createElement('button');
      rc.className = 'act warn'; rc.textContent = '⚠';
      rc.title = 'recover: unfreeze or terminate stuck target';
      rc.dataset.pid = t.pid;
      chips.appendChild(rc);

      row.append(pad, nm, tag, at, chips);
      flist.appendChild(row);
    });
  } catch (e) { log(`frozen scan: ${e}`, 'er'); }
}

// frozen-panel sortable headers
document.querySelectorAll('.fhead-nm, .fhead-tag').forEach(h => {
  h.addEventListener('click', () => {
    const key = h.dataset.key;
    if (!key) return;
    if (frozenSort.key === key) frozenSort.dir = -frozenSort.dir;
    else { frozenSort.key = key; frozenSort.dir = 1; }
    refresh_frozen();
  });
});

$('frozen-bar').addEventListener('click', () => {
  $('frozen-panel').classList.toggle('hidden');
  refresh_frozen();
});
$('frozen-close').addEventListener('click', () => {
  $('frozen-panel').classList.add('hidden');
});
$('flist').addEventListener('click', async (e) => {
  const btn = e.target.closest('.act');
  if (!btn) return;
  const pid = Number(btn.closest('.trow').dataset.pid);
  if (btn.classList.contains('warn')) {
    // recovery: try unfreeze first, then offer terminate
    await doUnfreeze(pid);
    const still = procs.find(p => p.pid === pid);
    if (still && (still.frozen || frozenSet.has(pid))) {
      if (confirm(`pid ${pid} still frozen after unfreeze. Terminate?`)) {
        const ok = await invoke('soft_terminate', { pid });
        log(ok ? `terminated pid=${pid}` : `terminate failed pid=${pid}`, ok ? 'ok' : 'er');
      }
    }
    refresh_frozen();
  } else {
    await doUnfreeze(pid);
    refresh_frozen();
  }
});

async function doUnfreeze(pid) {
  const p = procs.find(x => x.pid === pid); if (!p) return;
  try {
    await invoke('unfreeze', { pid });
    frozenSet.delete(pid);
    log(`released pid=${pid} ${p.name}`, 'ok');
    say(`released ${p.name}`);
    try { await invoke('save_settings'); } catch {}
  } catch (e) { log(`release failed: ${e}`, 'er'); }
  await refresh();
}

async function doInject(pid) {
  const p = procs.find(x => x.pid === pid); if (!p) return;
  try {
    await invoke('inject', { pid });
    log(`payload injected pid=${pid} ${p.name}`, 'in');
    say('payload injected');
  } catch (e) { log(`inject failed: ${e} (elevation?)`, 'er'); say('inject failed', true); }
}

async function doNet(pid) {
  const p = procs.find(x => x.pid === pid); if (!p) return;
  try {
    if (netSet.has(pid)) {
      await invoke('unblock_net', { pid });
      netSet.delete(pid);
      log(`egress restored pid=${pid}`, 'ok');
      try { await invoke('save_settings'); } catch {}
    } else {
      const ok = await invoke('block_net', { pid });
      if (ok) {
        netSet.add(pid);
        log(`egress severed pid=${pid} ${p.name}`, 'wa');
        try { await invoke('save_settings'); } catch {}
      } else {
        log(`egress block refused pid=${pid} — needs admin`, 'er');
        say('needs admin — escalating', true);
        // auto-escalate via self-relaunch as admin
        if (await invoke('is_admin')) { /* shouldn't happen */ }
        else if (await invoke('relaunch_admin')) {
          say('relaunched as admin — retry NET', true);
        }
      }
    }
  } catch (e) { log(`net failed: ${e}`, 'er'); }
  await refresh();
}

async function globalToggle(force) {
  try {
    const cur = await invoke('global_active');
    if (force === undefined || cur !== force) await invoke('toggle_global');
    const active = await invoke('global_active');
    const chip = $('st-global');
    chip.querySelector('span').textContent = active ? 'ONLINE' : 'HALTED';
    chip.classList.toggle('live', !!active);
    chip.classList.toggle('off', !active);
    log(`hooks ${active ? 'ENGAGED' : 'HALTED'}`, active ? 'ok' : 'wa');
    say(active ? 'hooks engaged' : 'all hooks halted');
  } catch (e) { log(`global failed: ${e}`, 'er'); }
  await refresh();
}

// ── manual target selection ────────────────────────────────────────────────
async function listApps() {
  try {
    const apps = await invoke('list_installed_apps');
    if (apps.length === 0) { log('no installed apps found', 'wa'); say('no apps found', true); return; }
    apps.forEach(a => log(`${a.name}  →  ${a.path}`, 'mu'));
    log(`${apps.length} apps indexed — use launch <path>`, 'mu');
    say(`found ${apps.length} apps`);
  } catch (e) { log(`app scan failed: ${e}`, 'er'); say('scan failed', true); }
}

async function doLaunch(target) {
  if (!target) { log('usage: launch <path>', 'er'); say('specify path', true); return false; }
  try {
    const ok = await invoke('launch_app', { path: target });
    if (ok) {
      log(`launched: ${target}`, 'ok');
      say('app launched');
      return true;
    } else {
      log(`launch failed: ${target}`, 'er'); say('launch failed', true);
      return false;
    }
  } catch (e) { log(`launch error: ${e}`, 'er'); say('launch error', true); return false; }
}

// ── shell ────────────────────────────────────────────────────────────────
async function cmd(raw) {
  const [c, ...rest] = raw.trim().split(/\s+/);
  const a0 = rest[0];
  const byPid = () => (a0 ? procs.find(p => String(p.pid) === a0) : sel());
  const need = (p) => { if (!p) { log(`no target: ${a0 || '(none selected)'}`, 'er'); say('no such target', true); return false; } return true; };

  switch (c) {
    case '': return;
    case 'help': case '?': $('help').classList.remove('hidden'); return;
    case 'quit': case 'exit': case 'q': appWindow.close(); return;
    case 'freeze':   {
      const p = byPid();
      if (p) await openFreezeDialog(p.pid);
      else {
        // substring match on process name → glob freeze
        const needle = (a0 || '').toLowerCase();
        if (!needle) { say('specify pid or name', true); return; }
        selSet.clear(); procs.forEach(pp => { if (pp.name.toLowerCase().includes(needle)) selSet.add(pp.pid); });
        await bulkAction('freeze');
      }
      return;
    }
    case 'unfreeze': { const p = byPid(); if (need(p)) await doUnfreeze(p.pid); return; }
    case 'inject':   { const p = byPid(); if (need(p)) await doInject(p.pid);   return; }
    case 'net':      { const p = byPid(); if (need(p)) await doNet(p.pid);      return; }
    case 'halt': await globalToggle(false); return;
    case 'wake': await globalToggle(true);  return;
    case 'ls': case 'ps': log('rescan', 'mu'); await refresh(); return;
    case 'filter': filter = rest.join(' '); filIn.value = filter; render(); return;
    case 'clear': logEl.replaceChildren(); return;
    case 'install': case 'apps':
      await listApps(); return;
    case 'launch':
      await doLaunch(rest.join(' ')); return;
    case 'crt':
      document.body.classList.toggle('no-crt');
      localStorage.setItem('fz_crt', document.body.classList.contains('no-crt') ? '0' : '1');
      return;
    default: log(`unknown command: ${c} — try help`, 'er'); say('unknown command', true);
  }
}

cmdIn.addEventListener('keydown', async (e) => {
  if (e.key === 'Enter') {
    const v = cmdIn.value;
    if (v.trim()) { history.push(v); histIdx = history.length; }
    cmdIn.value = '';
    await cmd(v);
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    if (!history.length) return;
    histIdx = Math.max(0, histIdx - 1);
    cmdIn.value = history[histIdx] ?? '';
  } else if (e.key === 'ArrowDown') {
    e.preventDefault();
    if (!history.length) return;
    histIdx = Math.min(history.length, histIdx + 1);
    cmdIn.value = histIdx >= history.length ? '' : history[histIdx];
  } else if (e.key === 'Escape') {
    cmdIn.value = '';
    $('help').classList.add('hidden');
  }
});

// row action chips (delegated)
tlist.addEventListener('click', async (e) => {
  const btn = e.target.closest('.act');
  if (!btn) return;
  e.stopPropagation();
  const pid = Number(btn.closest('.trow').dataset.pid);
  selPid = pid;
  switch (btn.dataset.act) {
    case 'frz': openFreezeDialog(pid); break;
    case 'unf': await doUnfreeze(pid); break;
    case 'inj': await doInject(pid); break;
    case 'net': await doNet(pid); break;
  }
  render();
});

// freeze modal wiring
$('fm-close').addEventListener('click', closeFreezeModal);
$('fm-cancel').addEventListener('click', closeFreezeModal);
$('fm-go').addEventListener('click', fmEngage);
document.querySelectorAll('input[name="fm-mode"]').forEach(r =>
  r.addEventListener('change', fmToggleFields));
$('freeze-modal').addEventListener('click', (e) => {
  if (e.target === $('freeze-modal')) closeFreezeModal();
});
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !$('freeze-modal').classList.contains('hidden'))
    closeFreezeModal();
});

$('applist').addEventListener('click', async (e) => {
  const btn = e.target.closest('.act');
  if (!btn) return;
  e.stopPropagation();
  const path = btn.dataset.path;
  if (!path) return;
  switch (btn.dataset.act) {
    case 'launch-app':
      await doLaunch(path);
      break;
    case 'freeze-app':
      // open modal FIRST (let user choose mode/clock), launch happens on ENGAGE
      selPid = null;
      openFreezeDialog(null, path);
      break;
    case 'inj-app':
      if (await doLaunch(path)) {
        await new Promise(r => setTimeout(r, 1500));
        await refresh();
        const p = procs.find(p => p.exe.toLowerCase() === path.toLowerCase());
        if (p) { selPid = p.pid; await doInject(p.pid); }
      }
      break;
    case 'net-app':
      if (await doLaunch(path)) {
        await new Promise(r => setTimeout(r, 1500));
        await refresh();
        const p = procs.find(p => p.exe.toLowerCase() === path.toLowerCase());
        if (p) { selPid = p.pid; await doNet(p.pid); }
      }
      break;
  }
});

filIn.addEventListener('input', (e) => { filter = e.target.value; render(); });

let appsCollapsed = false;
let procsCollapsed = false;
$('apps-toggle').addEventListener('click', () => {
  appsCollapsed = !appsCollapsed;
  $('apps-toggle').textContent = appsCollapsed ? '▶' : '▼';
  $('applist').classList.toggle('collapsed', appsCollapsed);
});
$('proc-toggle').addEventListener('click', () => {
  procsCollapsed = !procsCollapsed;
  $('proc-toggle').textContent = procsCollapsed ? '▶' : '▼';
  $('tlist').classList.toggle('collapsed', procsCollapsed);
});

// help / info overlay
$('hud-help').addEventListener('click', () => {
  $('info').classList.remove('hidden');
  cmdIn.focus();
});
$('info-close').addEventListener('click', () => {
  $('info').classList.add('hidden');
  cmdIn.focus();
});
$('info-ok').addEventListener('click', () => {
  $('info').classList.add('hidden');
  cmdIn.focus();
});

$('log-clear').addEventListener('click', () => { logEl.replaceChildren(); cmdIn.focus(); });

// disable context menu (cyberdeck discipline)
document.addEventListener('contextmenu', (e) => { e.preventDefault(); });

// keep the shell ready: clicks anywhere that aren't inputs/buttons/dragzones refocus it
document.addEventListener('mousedown', (e) => {
  if (e.target.closest('input, button, .act, .overlay, [data-tauri-drag-region], .rz')) return;
  setTimeout(() => cmdIn.focus(), 0);
});

// ── window chrome ────────────────────────────────────────────────────────
$('w-min').addEventListener('click', () => appWindow.minimize());
$('w-max').addEventListener('click', async () =>
  (await appWindow.isMaximized()) ? appWindow.unmaximize() : appWindow.maximize());
$('w-close').addEventListener('click', () => appWindow.close());

document.querySelectorAll('.rz').forEach(el => {
  el.addEventListener('mousedown', async (e) => {
    e.preventDefault();
    try { await appWindow.startResizeDragging(el.dataset.dir); } catch { /* noop */ }
  });
});

// ── consent ──────────────────────────────────────────────────────────────
$('consent-accept').addEventListener('click', () => {
  localStorage.setItem('fz_consent', '1');
  $('consent').classList.add('hidden');
  log('consent accepted — operator authorised', 'ok');
  cmdIn.focus();
});
$('consent-exit').addEventListener('click', () => appWindow.close());

$('help-close').addEventListener('click', () => {
  $('help').classList.add('hidden');
  cmdIn.focus();
});

// ── clock ────────────────────────────────────────────────────────────────
setInterval(() => {
  const d = new Date();
  $('st-clock').querySelector('span').textContent = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  telemetry();
}, 1000);

// ── boot ─────────────────────────────────────────────────────────────────
async function boot() {
  if (localStorage.getItem('fz_crt') === '0') document.body.classList.add('no-crt');
  $('consent').classList.toggle('hidden', !!localStorage.getItem('fz_consent'));

  log('FREEGUN v0.1 — chrono interdict deck online', 'ok');
  log('control block Local\\FreezeGun.Ctl mapped', 'mu');
  log('hooks staged: kernel32 ×6 · ntdll ×1 · winmm ×1', 'mu');

  try {
    const pid = await invoke('host_pid');
    $('st-host').querySelector('span').textContent = String(pid);
    log(`host pid=${pid}`, 'mu');
  } catch { $('st-host').querySelector('span').textContent = '····'; }

  try {
    dllPath = await invoke('resolve_dll_path');
    log(`payload ${dllPath.split('\\').pop()}`, 'mu');
  } catch { dllPath = ''; }

  try {
    const active = await invoke('global_active');
    const chip = $('st-global');
    chip.querySelector('span').textContent = active ? 'ONLINE' : 'HALTED';
    chip.classList.toggle('live', !!active);
    chip.classList.toggle('off', !active);
  } catch {}

  await refresh();
  log(`${procs.length} targets acquired`, 'mu');
  say('ready — type help');
  cmdIn.focus();
  setInterval(refresh, 2500);
}

boot();
