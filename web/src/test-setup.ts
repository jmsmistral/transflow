import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

// The real adapter bootstraps Stencil at import time, including a Node timer
// that can outlive an isolated jsdom file. Component tests exercise our grid
// bindings with text DOM; browser journeys qualify the actual virtualized grid.
// Selection-specific tests can override this default with their own adapter.
vi.mock("@revolist/react-datagrid", () => import("./test-grid"));

afterEach(cleanup);

// jsdom has no layout observer. Actual sizing, gestures and worker layout are
// qualified against the coordinator in the internal Browser, separately.
globalThis.ResizeObserver = class {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
};

// jsdom does not implement native dialog opening; browsers are checked separately.
if (typeof HTMLDialogElement !== "undefined") {
  HTMLDialogElement.prototype.showModal = function () {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function () {
    this.removeAttribute("open");
  };
}
