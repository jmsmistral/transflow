'use strict';
// Finite JSON-RPC transport only; domain features remain in the native service.
const { spawn } = require('node:child_process');
const MAX = 1024 * 1024;
class Transport {
  constructor(executable, args, notification, failure) {
    this.pending = new Map(); this.sequence = 0; this.buffer = Buffer.alloc(0); this.closed = false;
    this.child = spawn(executable, args, { shell: false, stdio: ['pipe', 'pipe', 'pipe'] });
    this.child.stdout.on('data', chunk => {
      try {
        this.buffer = Buffer.concat([this.buffer, chunk]);
        while (this.buffer.length) {
          const boundary = this.buffer.indexOf('\r\n\r\n');
          if (boundary < 0) { if (this.buffer.length > 16384) throw Error('LSP header limit'); break; }
          const header = this.buffer.subarray(0, boundary).toString('ascii');
          const lengths = [...header.matchAll(/^Content-Length: ([0-9]+)$/gmi)];
          if (boundary > 16384 || lengths.length !== 1) throw Error('Invalid LSP header');
          const size = Number(lengths[0][1]);
          if (!size || size > MAX) throw Error('LSP frame limit');
          if (this.buffer.length < boundary + 4 + size) break;
          const frame = JSON.parse(this.buffer.subarray(boundary + 4, boundary + 4 + size));
          this.buffer = this.buffer.subarray(boundary + 4 + size);
          if (frame.jsonrpc !== '2.0') throw Error('Invalid JSON-RPC version');
          if (frame.id !== undefined) {
            const pending = this.pending.get(frame.id);
            if (pending) { clearTimeout(pending.timer); this.pending.delete(frame.id); frame.error ? pending.reject(Error(frame.error.message)) : pending.resolve(frame.result); }
          } else notification(frame.method, frame.params);
        }
      } catch (error) { this.closed = true; this.fail(error); failure(error); this.child.kill(); }
    });
    // The native service emits fixed local diagnostics, never document contents.
    this.child.stderr.resume();
    this.child.on('error', error => { this.closed = true; this.fail(error); failure(error); });
    this.child.on('exit', () => { const error=Error('Transflow dataset service stopped'); this.fail(error); if (!this.closed) failure(error); this.closed=true; });
    this.child.stdin.on('error', error => this.fail(error));
  }
  fail(error) { for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(error); } this.pending.clear(); }
  send(method, params, id) {
    const body = Buffer.from(JSON.stringify({ jsonrpc: '2.0', method, params, ...(id === undefined ? {} : { id }) }));
    if (body.length > MAX) throw Error('Document exceeds LSP frame limit');
    this.child.stdin.write(Buffer.concat([Buffer.from(`Content-Length: ${body.length}\r\n\r\n`), body]));
  }
  call(method, params) {
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(Error('Transflow editor request timed out')); }, 10000);
      this.pending.set(id, { resolve, reject, timer });
      try { this.send(method, params, id); } catch (error) { clearTimeout(timer); this.pending.delete(id); reject(error); }
    });
  }
  async close() {
    if (this.closed) { this.child.kill(); return; }
    this.closed = true;
    try { await this.call('shutdown', {}); this.send('exit', {}); } catch { /* already closed */ }
    this.child.kill(); this.fail(Error('Transflow dataset service stopped'));
  }
}
module.exports = { Transport };
