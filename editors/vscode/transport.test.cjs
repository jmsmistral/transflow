'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { pathToFileURL } = require('node:url');
const { Transport } = require('./transport.cjs');
test('native service framing, completion and graceful lifecycle', async () => {
  const root = process.env.TRANSFLOW_EDITOR_A;
  assert.ok(root && process.env.TRANSFLOW_CLI && process.env.TRANSFLOW_PYTHON);
  const service = new Transport(process.env.TRANSFLOW_CLI, ['--workspace', root, 'lsp', '--python', process.env.TRANSFLOW_PYTHON], () => {}, error => { throw error; });
  try {
    const uri = pathToFileURL(root).href;
    const result = await service.call('initialize', {rootUri:uri, capabilities:{}});
    assert.equal(result.capabilities.positionEncoding, 'utf-16');
    service.send('initialized', {});
    const file = pathToFileURL(`${root}/src/node-draft.py`).href;
    service.send('textDocument/didOpen', {textDocument:{uri:file,languageId:'python',version:1,text:'from transflow.catalog import C\nC.raw.'}});
    const completions = await service.call('textDocument/completion', {textDocument:{uri:file},position:{line:1,character:6}});
    assert.deepEqual(completions.items.map(i => i.label), ['orders']);
  } finally { await service.close(); }
});
test('malformed framed output rejects pending requests without an unhandled rejection', async () => {
  const service = new Transport(process.execPath, ['-e', 'process.stdout.write("Content-Length: 999999999\\r\\n\\r\\n")'], () => {}, () => {});
  await assert.rejects(service.call('initialize', {}), /frame limit/);
  await service.close();
});
