'use strict';
const vscode = require('vscode');
const { existsSync } = require('node:fs');
const { join } = require('node:path');
const { Transport } = require('./transport.cjs');
const clients = new Map();
const asRange = r => new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character);
const point = position => ({ line: position.line, character: position.character });
let diagnostics;
async function client(document) {
  if (!vscode.workspace.isTrusted || document.uri.scheme !== 'file') return;
  const folder = vscode.workspace.getWorkspaceFolder(document.uri);
  if (!folder || !existsSync(join(folder.uri.fsPath, 'workspace.toml'))) return;
  const key = folder.uri.toString();
  if (!clients.has(key)) {
    const ready = (async () => {
      const config = vscode.workspace.getConfiguration('transflow', folder.uri);
      const args = ['--workspace', folder.uri.fsPath, 'lsp'];
      if (config.get('python')) args.push('--python', config.get('python'));
      if (config.get('branch')) args.push('--branch', config.get('branch'));
      const service = new Transport(config.get('executable'), args, (method, params) => {
        if (method === 'textDocument/publishDiagnostics') diagnostics.set(vscode.Uri.parse(params.uri), params.diagnostics.map(d => {
          const value = new vscode.Diagnostic(asRange(d.range), d.message, d.severity === 1 ? vscode.DiagnosticSeverity.Error : d.severity === 2 ? vscode.DiagnosticSeverity.Warning : vscode.DiagnosticSeverity.Information);
          value.source = 'Transflow'; value.code = d.code; return value;
        }));
      }, () => { clients.delete(key); vscode.window.showWarningMessage('Transflow dataset service unavailable. Check the executable/interpreter settings and restart it.'); });
      try { await service.call('initialize', { processId: process.pid, rootUri: key, workspaceFolders: [{ uri: key, name: folder.name }], capabilities: {} }); }
      catch (error) { await service.close(); throw error; }
      service.send('initialized', {});
      return { service, versions: new Map() };
    })();
    clients.set(key, ready);
    ready.catch(() => clients.delete(key));
  }
  const state = await clients.get(key);
  const version = state.versions.get(document.uri.toString());
  if (version !== document.version) {
    if (version === undefined) state.service.send('textDocument/didOpen', { textDocument: { uri: document.uri.toString(), languageId: 'python', version: document.version, text: document.getText() } });
    else state.service.send('textDocument/didChange', { textDocument: { uri: document.uri.toString(), version: document.version }, contentChanges: [{ text: document.getText() }] });
    state.versions.set(document.uri.toString(), document.version);
  }
  return state.service;
}
async function request(document, position, method) {
  try { const service = await client(document); return service && await service.call(method, { textDocument: { uri: document.uri.toString() }, position: point(position) }); } catch { return undefined; }
}
async function stop() {
  const all = [...clients.values()]; clients.clear();
  await Promise.allSettled(all.map(async p => (await p).service.close()));
  if (diagnostics) diagnostics.clear();
}
function activate(context) {
  diagnostics = vscode.languages.createDiagnosticCollection('transflow');
  const selector = { language: 'python', scheme: 'file' };
  context.subscriptions.push(diagnostics,
    vscode.languages.registerCompletionItemProvider(selector, { async provideCompletionItems(doc, pos) {
      const result = await request(doc, pos, 'textDocument/completion');
      return result && new vscode.CompletionList(result.items.map(i => {
        const item = new vscode.CompletionItem(i.label, i.kind === 9 ? vscode.CompletionItemKind.Module : vscode.CompletionItemKind.Variable);
        item.detail = i.detail; item.range = asRange(i.textEdit.range); item.insertText = i.textEdit.newText; return item;
      }), result.isIncomplete);
    } }, '.', '/', '"', "'"),
    vscode.languages.registerHoverProvider(selector, { async provideHover(doc, pos) {
      const value = await request(doc, pos, 'textDocument/hover');
      return value && new vscode.Hover(new vscode.MarkdownString().appendText(value.contents.value), asRange(value.range));
    } }),
    vscode.languages.registerDefinitionProvider(selector, { async provideDefinition(doc, pos) {
      const value = await request(doc, pos, 'textDocument/definition');
      return value && new vscode.Location(vscode.Uri.parse(value.uri), asRange(value.range));
    } }),
    vscode.workspace.onDidOpenTextDocument(doc => { if (doc.languageId === 'python') client(doc).catch(() => {}); }),
    vscode.workspace.onDidChangeTextDocument(event => { if (event.document.languageId === 'python') client(event.document).catch(() => {}); }),
    vscode.workspace.onDidCloseTextDocument(doc => {
      diagnostics.delete(doc.uri);
      for (const pending of clients.values()) pending.then(state => { if (state.versions.delete(doc.uri.toString())) state.service.send('textDocument/didClose', { textDocument: { uri: doc.uri.toString() } }); }).catch(() => {});
    }),
    vscode.workspace.onDidChangeConfiguration(event => { if (event.affectsConfiguration('transflow')) void stop(); }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => { void stop(); }),
    vscode.commands.registerCommand('transflow.restart', async () => { await stop(); const editor = vscode.window.activeTextEditor; if (editor) await client(editor.document); }),
    vscode.commands.registerCommand('transflow.openProducer', async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) return;
      const version = editor.document.version;
      const position = editor.selection.active;
      const value = await request(editor.document, position, 'textDocument/definition');
      if (!value || vscode.window.activeTextEditor !== editor || editor.document.version !== version || !editor.selection.active.isEqual(position)) return;
      await vscode.window.showTextDocument(vscode.Uri.parse(value.uri), { selection: asRange(value.range) });
    }),
    { dispose: () => { void stop(); } },
  );
  const watcher = vscode.workspace.createFileSystemWatcher('**/{catalog.toml,workspace.toml,transflow.local.toml,current}');
  const refresh = () => { for (const pending of clients.values()) pending.then(state => state.service.send('workspace/didChangeWatchedFiles', { changes: [] })).catch(() => {}); };
  context.subscriptions.push(watcher, watcher.onDidChange(refresh), watcher.onDidCreate(refresh), watcher.onDidDelete(refresh));
  for (const doc of vscode.workspace.textDocuments) if (doc.languageId === 'python') client(doc).catch(() => {});
}
module.exports = { activate, deactivate: stop };
