'use strict';
// Runs in the actual desktop extension host, against the installed local VSIX.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vscode = require('vscode');

async function eventually(read, matches, label) {
  const deadline = Date.now() + 15000;
  let value;
  do {
    value = await read();
    if (matches(value)) return value;
    await new Promise(resolve => setTimeout(resolve, 50));
  } while (Date.now() < deadline);
  throw new Error(`Native editor state did not arrive: ${label}`);
}
const label = item => typeof item.label === 'string' ? item.label : item.label.label;
// MarkdownString.appendText safely encodes spaces in the extension API response.
const hoverText = content => String(content.value || content).replaceAll('&nbsp;', ' ');
const reference = 'from transflow.catalog import C\nC.raw.';
async function replace(document, text) {
  const edit = new vscode.WorkspaceEdit();
  edit.replace(document.uri, new vscode.Range(document.positionAt(0), document.positionAt(document.getText().length)), text);
  assert.equal(await vscode.workspace.applyEdit(edit), true);
}
async function completions(document, line, character) {
  return vscode.commands.executeCommand('vscode.executeCompletionItemProvider', document.uri, new vscode.Position(line, character));
}

exports.run = async function run() {
  const roots = [process.env.TRANSFLOW_EDITOR_A, process.env.TRANSFLOW_EDITOR_B];
  const checks = [];
  const report = { editor: vscode.version, checks, outcome: 'running' };
  try {
    assert.equal(vscode.workspace.isTrusted, true);
    const extension = vscode.extensions.getExtension('transflow-local.transflow');
    assert.ok(extension, 'The packaged Transflow extension must be installed');
    assert.ok(extension.extensionPath.startsWith(process.env.TRANSFLOW_INSTALLED_EXTENSIONS + path.sep));
    await extension.activate();
    checks.push('installed VSIX activates in the native trusted desktop host');

    const a = await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(roots[0], 'src/editor-draft.py')));
    const b = await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(roots[1], 'src/editor-draft.py')));
    const aEditor = await vscode.window.showTextDocument(a);
    await replace(a, reference);
    await replace(b, reference);
    const aItems = await eventually(() => completions(a, 1, 6), value => value?.items.some(item => label(item) === 'orders'), 'orders completion');
    const bItems = await eventually(() => completions(b, 1, 6), value => value?.items.some(item => label(item) === 'invoices'), 'invoices completion');
    assert.ok(!aItems.items.some(item => label(item) === 'invoices'));
    assert.ok(!bItems.items.some(item => label(item) === 'orders'));
    checks.push('simultaneous workspace catalogue completions remain isolated');

    await replace(a, 'from transflow.catalog import C\nC.raw.invoices');
    await eventually(() => vscode.languages.getDiagnostics(a.uri), items => items.some(d => d.source === 'Transflow' && d.code === 'unknown-dataset'), 'cross-workspace typo diagnostic');
    await replace(a, 'from transflow.catalog import C\nC.raw.orders');
    await eventually(() => vscode.languages.getDiagnostics(a.uri), items => !items.some(d => d.source === 'Transflow' && d.code === 'unknown-dataset'), 'corrected reference diagnostic');
    checks.push('typo diagnostics appear and clear through real document edits');

    const point = new vscode.Position(1, 8);
    const hovers = await eventually(() => vscode.commands.executeCommand('vscode.executeHoverProvider', a.uri, point), values => values?.some(h => h.contents.some(c => hoverText(c).includes('Descriptor source snapshot:'))), 'retained metadata hover');
    assert.ok(hovers.length);
    const definitions = await vscode.commands.executeCommand('vscode.executeDefinitionProvider', a.uri, point);
    assert.ok(definitions.some(d => (d.uri || d.targetUri).fsPath === path.join(roots[0], 'src/orders.py')));
    await vscode.window.showTextDocument(a);
    aEditor.selection = new vscode.Selection(point, point);
    await vscode.commands.executeCommand('transflow.openProducer');
    await eventually(() => vscode.window.activeTextEditor?.document.uri.fsPath, value => value === path.join(roots[0], 'src/orders.py'), 'Open Dataset Producer command');
    checks.push('hover, definition provider and producer command use retained metadata');

    const registry = path.join(roots[0], '.transflow/catalog.toml');
    const original = fs.readFileSync(registry, 'utf8');
    try {
      fs.writeFileSync(registry, original.replaceAll('raw/orders', 'raw/shipments'));
      await eventually(() => vscode.languages.getDiagnostics(a.uri), items => items.some(d => d.source === 'Transflow' && d.code === 'unknown-dataset'), 'catalogue watcher refresh without typing');
      await replace(a, reference);
      await eventually(() => completions(a, 1, 6), value => value?.items.some(item => label(item) === 'shipments') && !value.items.some(item => label(item) === 'orders'), 'refreshed completion');
      const unchanged = await completions(b, 1, 6);
      assert.ok(unchanged.items.some(item => label(item) === 'invoices'));
      assert.ok(!unchanged.items.some(item => label(item) === 'shipments'));
    } finally { fs.writeFileSync(registry, original); }
    checks.push('catalogue changes refresh diagnostics and keep other workspaces isolated');

    const sentinel = path.join(roots[0], 'must-not-execute');
    await replace(a, `from pathlib import Path\nPath(${JSON.stringify(sentinel)}).touch()\nfrom transflow.catalog import C\nC.raw.orders`);
    await eventually(() => vscode.commands.executeCommand('vscode.executeHoverProvider', a.uri, new vscode.Position(3, 8)), values => values?.length > 0, 'draft parsing');
    assert.equal(fs.existsSync(sentinel), false);
    checks.push('editing and inspecting source never executes the draft');

    if (process.env.TRANSFLOW_PYTHON_EXTENSION) {
      const pythonExtension = vscode.extensions.getExtension(process.env.TRANSFLOW_PYTHON_EXTENSION);
      assert.ok(pythonExtension, 'The Python language extension must be installed');
      await pythonExtension.activate();
      await replace(a, 'from pathlib import Pa');
      await eventually(() => completions(a, 0, 22), value => value?.items.some(item => label(item) === 'Path'), 'ordinary Python completion');
      await replace(a, 'from transflow import trans');
      await eventually(() => completions(a, 0, 27), value => value?.items.some(item => label(item) === 'transform'), 'installed SDK root export');
      await replace(a, 'from transflow.catalog import ');
      await eventually(() => completions(a, 0, 30), value => value?.items.some(item => label(item) === 'C'), 'installed SDK submodule');
      await replace(b, 'from transflow import trans');
      await eventually(() => completions(b, 0, 27), value => value?.items.some(item => label(item) === 'transform'), 'second workspace SDK root export');
      await replace(b, reference);
      const bItems = await completions(b, 1, 6);
      assert.ok(bItems.items.some(item => label(item) === 'invoices'));
      assert.ok(!bItems.items.some(item => label(item) === 'orders'));
      await replace(a, reference);
      await eventually(() => completions(a, 1, 6), value => value?.items.some(item => label(item) === 'orders'), 'Transflow completion alongside Python');
      checks.push('ordinary Python and installed SDK completion coexist with Transflow');
      report.pythonExtension = { id: pythonExtension.id, version: pythonExtension.packageJSON.version };
    }

    await vscode.commands.executeCommand('transflow.restart');
    await vscode.window.showTextDocument(a);
    await replace(a, reference);
    await eventually(() => completions(a, 1, 6), value => value?.items.some(item => label(item) === 'orders'), 'restart completion');
    checks.push('dataset service restarts and resumes completion');
    report.outcome = 'passed';
  } catch (error) {
    report.outcome = 'failed';
    report.error = error.stack || String(error);
    throw error;
  } finally {
    fs.writeFileSync(process.env.TRANSFLOW_EDITOR_REPORT, JSON.stringify(report, null, 2) + '\n');
  }
};
