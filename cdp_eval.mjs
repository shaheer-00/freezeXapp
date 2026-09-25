// Minimal CDP driver: evaluate a JS expression in the live WebView2 page.
// Usage: node cdp_eval.mjs <file.js>
import fs from 'node:fs';

const jsFile = process.argv[2];
if (!jsFile) { console.error('usage: node cdp_eval.mjs <file.js>'); process.exit(1); }
const expr = fs.readFileSync(jsFile, 'utf8');

const targets = await (await fetch('http://127.0.0.1:9222/json')).json();
const page = targets.find(t => t.type === 'page');
if (!page) { console.error('FAIL: no page target'); process.exit(1); }
console.error(`[cdp] ${page.title} ${page.url}`);

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.addEventListener('open', res, { once: true });
  ws.addEventListener('error', rej, { once: true });
});

const reply = await new Promise((res, rej) => {
  const timer = setTimeout(() => rej(new Error('CDP timeout')), 20000);
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id === 1) { clearTimeout(timer); res(m); }
  });
  ws.send(JSON.stringify({
    id: 1,
    method: 'Runtime.evaluate',
    params: {
      expression: expr,
      awaitPromise: true,
      returnByValue: true,
      includeCommandLineAPI: true,
      userGesture: true,
    },
  }));
});
ws.close();

const r = reply.result;
if (r.exceptionDetails) {
  console.log('EXCEPTION: ' + r.exceptionDetails.text);
  if (r.exceptionDetails.exception) console.log('  ' + r.exceptionDetails.exception.description);
  process.exit(2);
}
const v = r.result.value;
console.log(typeof v === 'string' ? v : JSON.stringify(v, null, 2));
