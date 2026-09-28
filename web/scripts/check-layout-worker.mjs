// Execute the production worker asset in an actual background thread. This catches
// worker-only packaging failures that mocked DOM/Worker component tests cannot.
import { Worker } from "node:worker_threads";
import { pathToFileURL } from "node:url";
import { setTimeout, clearTimeout } from "node:timers";
import assert from "node:assert/strict";
import ELK from "elkjs/lib/elk-api.js";
export async function checkLayoutWorker(file) {
  const worker = new Worker(
    `
    const {parentPort, workerData} = require('node:worker_threads');
    globalThis.self = globalThis;
    globalThis.postMessage = value => parentPort.postMessage(value);
    import(workerData).then(() => parentPort.on('message', data => self.onmessage({data})));
  `,
    { eval: true, workerData: pathToFileURL(file).href },
  );
  const adapter = {
    onmessage: null,
    postMessage(value) {
      worker.postMessage(value);
    },
    terminate() {
      void worker.terminate();
    },
  };
  worker.on("message", (data) => adapter.onmessage?.({ data }));
  const elk = new ELK({ workerFactory: () => adapter });
  let timer;
  try {
    const failure = new Promise((_, reject) => {
      worker.on("error", reject);
      timer = setTimeout(
        () => reject(new Error("Production layout worker did not respond")),
        15000,
      );
    });
    await Promise.race([
      failure,
      (async () => {
        const graph = {
          id: "root",
          layoutOptions: {
            "elk.algorithm": "layered",
            "elk.direction": "RIGHT",
          },
          children: ["a", "b", "c", "d"].map((id) => ({
            id,
            width: 250,
            height: 128,
          })),
          edges: [
            ["a", "b"],
            ["a", "c"],
            ["b", "d"],
            ["c", "d"],
          ].map(([a, b]) => ({ id: a + b, sources: [a], targets: [b] })),
        };
        const result = await elk.layout(graph);
        assert.equal(result.children.length, 4);
        assert.equal(new Set(result.children.map((n) => n.id)).size, 4);
        assert(
          result.children.every(
            (n) => Number.isFinite(n.x) && Number.isFinite(n.y),
          ),
        );
        assert(
          result.children.find((n) => n.id === "a").x <
            result.children.find((n) => n.id === "d").x,
        );
        assert.equal(result.edges.length, 4);
        await assert.rejects(
          elk.layout({
            ...graph,
            edges: [{ id: "invalid", sources: ["missing"], targets: ["d"] }],
          }),
        );
      })(),
    ]);
  } finally {
    clearTimeout(timer);
    await worker.terminate();
  }
}
