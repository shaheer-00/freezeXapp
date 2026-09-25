// Real-input test: dispatch genuine mouse events at each window-chrome button.
import fs from 'node:fs';

const targets = await (await fetch('http://127.0.0.1:9222/json')).json();
const page = targets.find(t => t.type === 'page');
if (!page) { console.error('FAIL: no page target'); process.exit(1); }

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.addEventListener('open', res, { once: true });
  ws.addEventListener('error', rej, { once: true });
});

let nextId = 1;
const pending = new Map();
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
});

function call(method, params = {}) {
  const id = nextId++;
  return new Promise((res, rej) => {
    const t = setTimeout(() => { pending.delete(id); rej(new Error(method + ' timeout')); }, 15000);
    pending.set(id, (m) => { clearTimeout(t); res(m); });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

async function evalJs(expression) {
  const m = await call('Runtime.evaluate', {
    expression, awaitPromise: true, returnByValue: true, includeCommandLineAPI: true,
  });
  const r = m.result;
  if (r.exceptionDetails) throw new Error('JS: ' + r.exceptionDetails.text + ' ' +
    (r.exceptionDetails.exception?.description || ''));
  return r.result.value;
}

const sleep = (ms) => new Promise(r => setTimeout(r, ms));

async function rectOf(id) {
  return JSON.parse(await evalJs(
    `JSON.stringify((()=>{const r=document.getElementById(${JSON.stringify(id)}).getBoundingClientRect();
      return {x:r.left+r.width/2, y:r.top+r.height/2};})())`));
}

async function realClick(id) {
  const { x, y } = await rectOf(id);
  await call('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y, button: 'none', buttons: 0 });
  await sleep(80);
  await call('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', buttons: 1, clickCount: 1 });
  await sleep(60);
  await call('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', buttons: 0, clickCount: 1 });
  return { x, y };
}

const out = {};
const state = async () => JSON.parse(await evalJs(
  `(async()=>{const w=window.__TAURI__.window.getCurrentWindow();
   return JSON.stringify({min:await w.isMinimized(),max:await w.isMaximized()});})()`));

out.start = await state();

// ── w-min ──────────────────────────────────────────────────────────────
out.minPtr = await realClick('w-min');
await sleep(1000);
out.afterMinClick = await state();
await evalJs(`(async()=>{await window.__TAURI__.window.getCurrentWindow().unminimize();return 'ok';})()`);
await sleep(900);
out.afterUnmin = await state();

// ── w-max ──────────────────────────────────────────────────────────────
out.maxPtr = await realClick('w-max');
await sleep(1000);
out.afterMaxClick = await state();
out.maxPtr2 = await realClick('w-max');
await sleep(1000);
out.afterMaxClick2 = await state();

console.log(JSON.stringify(out, null, 1));
ws.close();
