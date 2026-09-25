// Click the close button and report whether the target survives.
const targets = await (await fetch('http://127.0.0.1:9222/json')).json();
const page = targets.find(t => t.type === 'page');
if (!page) { console.log('no page target'); process.exit(1); }

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

const r = await call('Runtime.evaluate', {
  expression: `JSON.stringify((()=>{const b=document.getElementById('w-close').getBoundingClientRect();
    return {x:b.left+b.width/2,y:b.top+b.height/2};})())`,
  returnByValue: true,
});
const { x, y } = JSON.parse(r.result.result.value);
console.log(`close button at ${x},${y}`);

await call('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y, button: 'none', buttons: 0 });
await new Promise(r => setTimeout(r, 80));
await call('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', buttons: 1, clickCount: 1 });
await new Promise(r => setTimeout(r, 60));
try {
  await call('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', buttons: 0, clickCount: 1 });
} catch (e) {
  console.log('socket died during click (window closed mid-event)');
}

// did the app survive?
await new Promise(r => setTimeout(r, 2500));
try {
  const after = await (await fetch('http://127.0.0.1:9222/json')).json();
  const p = after.find(t => t.type === 'page');
  console.log(p ? 'RESULT: FAIL — window still alive' : 'RESULT: PASS — app closed');
} catch (e) {
  console.log('RESULT: PASS — CDP endpoint gone, app closed (exit code 1 expected)');
  process.exit(0);
}
process.exit(1);
