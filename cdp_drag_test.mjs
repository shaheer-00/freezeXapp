// Verifies which header points arm the Tauri drag region.
// Hooks __TAURI_INTERNALS__.invoke so we see the real plugin call Tauri's drag.js makes.
const targets = await (await fetch('http://127.0.0.1:9222/json')).json();
const page = targets.find(t => t.type === 'page');
if (!page) { console.log('FAIL: no page target'); process.exit(1); }

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
const call = (method, params = {}) => {
  const id = nextId++;
  return new Promise((res, rej) => {
    const t = setTimeout(() => { pending.delete(id); rej(new Error(method + ' timeout')); }, 12000);
    pending.set(id, (m) => { clearTimeout(t); res(m); });
    ws.send(JSON.stringify({ id, method, params }));
  });
};
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

// record every window-plugin call drag.js makes
await evalJs(`
  window.__dragLog = [];
  if (!window.__invokeHooked) {
    window.__invokeHooked = true;
    const orig = window.__TAURI_INTERNALS__.invoke.bind(window.__TAURI_INTERNALS__);
    window.__TAURI_INTERNALS__.invoke = function (cmd, args, opts) {
      if (/dragging|toggle_maximize/.test(String(cmd))) {
        window.__dragLog.push(String(cmd));
      }
      return orig(cmd, args, opts);
    };
  }
  'hooked'`);

async function press(idSel, label) {
  const info = JSON.parse(await evalJs(`JSON.stringify((()=>{
    const el = document.querySelector(${JSON.stringify(idSel)});
    if (!el) return {err:'no element'};
    const r = el.getBoundingClientRect();
    const x = r.left + r.width/2, y = r.top + r.height/2;
    const hit = document.elementFromPoint(x, y);
    return {x, y, hit: hit ? (hit.id ? '#'+hit.id : hit.tagName.toLowerCase()+'.'+(hit.className||'').split(' ')[0]) : null};
  })())`));

  if (info.err) { console.log(`${label}: ${info.err}`); return; }

  await evalJs(`window.__dragLog = []; 'cleared'`);
  await call('Input.dispatchMouseEvent', { type: 'mouseMoved', x: info.x, y: info.y, button: 'none', buttons: 0 });
  await sleep(60);
  await call('Input.dispatchMouseEvent', { type: 'mousePressed', x: info.x, y: info.y, button: 'left', buttons: 1, clickCount: 1 });
  await sleep(400);
  await call('Input.dispatchMouseEvent', { type: 'mouseReleased', x: info.x, y: info.y, button: 'left', buttons: 0, clickCount: 1 });
  await sleep(500);

  const log = await evalJs(`JSON.stringify(window.__dragLog)`);
  const armed = JSON.parse(log).some(c => c.includes('start_dragging'));
  console.log(`${label.padEnd(26)} hit=${String(info.hit).padEnd(14)} dragArmed=${armed}  calls=${log}`);
}

await press('#hud', 'header empty space');
await press('.glitch', 'logo text (FREEGUN)');
await press('.sub', 'subtitle text');
await press('.chip', 'status chip');
await press('#w-max', 'maximise button');
await press('#hud-help', 'help (?) button');
await press('#filter-input', 'filter input');

ws.close();
