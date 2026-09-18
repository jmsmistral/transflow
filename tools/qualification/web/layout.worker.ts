import ELK from 'elkjs/lib/elk.bundled.js';
import type { ElkNode } from 'elkjs/lib/elk-api';

const elk = new ELK();
self.onmessage = async (event: MessageEvent<ElkNode>) => {
  const result = await elk.layout(event.data, { layoutOptions: { 'elk.algorithm': 'layered' } });
  self.postMessage(result);
};
