import assert from 'node:assert/strict';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ReactFlow } from '@xyflow/react';
import ELK from 'elkjs/lib/elk.bundled.js';
import { mkdtemp, mkdir, readFile, writeFile, copyFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

test('React and React Flow import together', () => {
  assert.ok(ReactFlow);
  assert.equal(renderToStaticMarkup(createElement('button', null, 'Probe')), '<button>Probe</button>');
});

test('ELK lays out a connected synthetic graph', async () => {
  const graph = await new ELK().layout({
    id: 'probe',
    layoutOptions: { 'elk.algorithm': 'layered', 'elk.direction': 'RIGHT' },
    children: [{ id: 'a', width: 100, height: 40 }, { id: 'b', width: 100, height: 40 }],
    edges: [{ id: 'ab', sources: ['a'], targets: ['b'] }],
  });
  assert.equal(graph.children.length, 2);
  assert.ok(graph.children.every(node => Number.isFinite(node.x) && Number.isFinite(node.y)));
  assert.ok(graph.children[1].x > graph.children[0].x);
  assert.equal(graph.edges[0].sections.length, 1);
});

test('ELK rejects an edge with an unknown target', async () => {
  await assert.rejects(new ELK().layout({
    id: 'probe', children: [{ id: 'a', width: 100, height: 40 }],
    edges: [{ id: 'invalid', sources: ['a'], targets: ['missing'] }],
  }));
});

async function withDeclaration(version, declaration, check) {
  const root = await mkdtemp(join(tmpdir(), 'transflow-elk-patch-'));
  try {
    await mkdir(join(root, 'node_modules/elkjs/lib'), { recursive: true });
    await writeFile(join(root, 'node_modules/elkjs/package.json'), JSON.stringify({ version }));
    const path = join(root, 'node_modules/elkjs/lib/elk-api.d.ts');
    await writeFile(path, declaration);
    const script = join(root, 'patch-elk.mjs');
    await copyFile(new URL('./patch-elk.mjs', import.meta.url), script);
    await check(path, (apply = false) => spawnSync(process.execPath, [script, ...(apply ? ['--apply'] : [])], { encoding: 'utf8' }));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

test('ELK declaration preparation is explicit and idempotent', async () => {
  await withDeclaration('0.12.0', "type Child<T> = T['children'][number];", async (path, run) => {
    assert.notEqual(run().status, 0);
    assert.equal(run(true).status, 0);
    const prepared = await readFile(path, 'utf8');
    assert.match(prepared, /NonNullable/);
    assert.equal(run(true).status, 0);
    assert.equal(run().status, 0);
    assert.equal(await readFile(path, 'utf8'), prepared);
  });
});

test('ELK declaration preparation rejects version/content drift without writing', async () => {
  for (const [version, source] of [['0.13.0', "T['children'][number]"], ['0.12.0', 'unexpected content']]) {
    await withDeclaration(version, source, async (path, run) => {
      assert.notEqual(run(true).status, 0);
      assert.equal(await readFile(path, 'utf8'), source);
    });
  }
});
