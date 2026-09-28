import ELK from "elkjs/lib/elk-api.js";
import type { LayoutWorker } from "./positions";
/** Only ELK's small message adapter runs here; the layout engine lives in the worker asset. */
export function createLayoutWorker(): LayoutWorker {
  const worker = new Worker(new URL("./layout-worker.ts", import.meta.url), {
    type: "module",
  });
  const elk = new ELK({ workerFactory: () => worker });
  const adapter: LayoutWorker = {
    onmessage: null,
    onerror: null,
    postMessage({ id, graph }) {
      void elk
        .layout(graph)
        .then((result) => {
          const positions = Object.fromEntries(
            (result.children ?? []).map((node) => {
              if (!Number.isFinite(node.x) || !Number.isFinite(node.y))
                throw new Error("Invalid layout position");
              return [node.id, { x: node.x ?? 0, y: node.y ?? 0 }];
            }),
          );
          adapter.onmessage?.(
            new MessageEvent("message", { data: { id, positions } }),
          );
        })
        .catch(() =>
          adapter.onmessage?.(
            new MessageEvent("message", {
              data: {
                id,
                error:
                  "Layout could not be calculated. Use the dataset list or retry layout.",
              },
            }),
          ),
        );
    },
    terminate() {
      worker.terminate();
    },
  };
  worker.addEventListener("error", (event) => adapter.onerror?.(event));
  return adapter;
}
