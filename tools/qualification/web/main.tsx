// Build-only dependency probe, not the Transflow application UI.
import { createRoot } from 'react-dom/client';
import { ReactFlow } from '@xyflow/react';
import '@xyflow/react/dist/style.css';

const root = document.getElementById('root');
if (root === null) throw new Error('Missing probe root');
createRoot(root).render(
  <main aria-label="Dependency qualification" style={{ height: 400 }}>
    <ReactFlow nodes={[{ id: 'synthetic', position: { x: 0, y: 0 }, data: { label: 'Synthetic probe' } }]} />
  </main>,
);
const worker = new Worker(new URL('./layout.worker.ts', import.meta.url), { type: 'module' });
worker.postMessage({ id: 'probe', children: [{ id: 'synthetic', width: 100, height: 50 }] });
worker.onmessage = () => worker.terminate();
