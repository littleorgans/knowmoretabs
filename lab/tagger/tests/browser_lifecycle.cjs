// Exercise the real harness at its process and CDP boundaries, without launching a browser.
const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { run } = require('./frontend_dom.cjs');

function setup({ stopAt = 'Target.createTarget', event = 'exit', reply = false, stubborn = false } = {}) {
  const child = new EventEmitter(), sent = [], signals = [], delays = [];
  child.pid = event === 'spawnError' ? undefined : 12345;
  child.stdio = Array.from({ length: 5 }, () => new EventEmitter());
  let alive = true;
  const answer = (msg) => child.stdio[4].emit('data', JSON.stringify({ id: msg.id,
    result: msg.method === 'Runtime.evaluate' ? { exceptionDetails: { text: 'case failed' } } : {} }) + '\0');
  child.stdio[3].write = (data) => {
    const msg = JSON.parse(data.slice(0, -1));
    sent.push(msg.method);
    if (msg.method === 'Browser.close') { if (!stubborn) alive = false; return; }
    setImmediate(() => {
      if (msg.method !== stopAt || reply) answer(msg);
      if (msg.method === stopAt) {
        if (event === 'exit') { alive = false; child.emit('exit', 1, null); }
        if (event === 'pipeError') child.stdio[3].emit('error', new Error('CDP pipe failed'));
        if (event === 'pipeEnd') child.stdio[4].emit('end');
        if (event === 'spawnError') child.emit('error', Object.assign(new Error('spawn failed'), { code: 'ENOENT' }));
      }
      if (msg.method === 'Page.navigate' && msg.method !== stopAt) {
        child.stdio[4].emit('data', JSON.stringify({ method: 'Page.loadEventFired' }) + '\0');
      }
    });
  };
  const context = vm.createContext({
    require: (name) => name === 'node:child_process' ? { spawn: () => child } : require(name),
    process: { argv: ['node', 'browser.cjs', 'test-chrome', 'test-profile', 'about:blank', 'identity'],
      kill(pid, signal) {
        assert.equal(pid, -child.pid, 'only the owned browser process group is signalled');
        signals.push(signal);
        if (!alive) throw Object.assign(new Error('gone'), { code: 'ESRCH' });
      } },
    setTimeout(fn, ms) { delays.push(ms); return setTimeout(fn, ms >= 10000 ? 20 : 0); }, clearTimeout, console, URL,
  });
  const source = fs.readFileSync(path.join(__dirname, 'browser.cjs'), 'utf8');
  const main = vm.runInContext(source.slice(0, source.lastIndexOf('\nmain().catch')) + '\nmain;', context);
  return { main, sent, signals, delays };
}

async function disconnectedCommand() {
  const browser = setup();
  await assert.rejects(browser.main(), /Chrome.*exit/);
  assert.ok(browser.sent.includes('Browser.close'), 'a failed command still reaches teardown');
}

async function disconnectedLoad() {
  const browser = setup({ stopAt: 'Page.navigate', reply: true });
  await assert.rejects(browser.main(), /Chrome.*exit/);
  assert.ok(browser.sent.includes('Browser.close'), 'a missing load event still reaches teardown');
}

async function launchFailure() {
  const browser = setup({ event: 'spawnError' });
  await assert.rejects(browser.main(), /spawn failed/);
  assert.deepEqual(browser.signals, [], 'a failed spawn has no process group to signal');
}

async function pipeFailure() {
  const browser = setup({ event: 'pipeError' });
  await assert.rejects(browser.main(), /CDP pipe failed/);
  assert.ok(browser.sent.includes('Browser.close'), 'a broken pipe still reaches teardown');
}

async function pipeEnded() {
  const browser = setup({ event: 'pipeEnd' });
  await assert.rejects(browser.main(), /Chrome.*(pipe|disconnect)/);
  assert.ok(browser.sent.includes('Browser.close'), 'an ended CDP stream still reaches teardown');
}

async function deadline(browser) {
  await assert.rejects(browser.main(), /browser case timed out/i);
  assert.ok(browser.sent.includes('Browser.close'), 'a hung case reaches teardown before Python kills Node');
  const timeout = browser.delays.find((ms) => ms >= 10000);
  assert.ok(timeout + 10000 < 60000, 'the case leaves time for both shutdown waits before the parent timeout');
}

async function unresponsiveCommand() {
  await deadline(setup({ event: null }));
}

async function unresponsiveLoad() {
  await deadline(setup({ stopAt: 'Page.navigate', reply: true, event: null }));
}

run({ disconnectedCommand, disconnectedLoad, launchFailure, pipeFailure, pipeEnded, unresponsiveCommand, unresponsiveLoad });
