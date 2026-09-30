import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { forwardRef, useImperativeHandle } from "react";
import { beforeEach, expect, test, vi } from "vitest";
import type { ApiPreviewV1, WireValue } from "./generated/contracts";
import { PreviewGrid, previewSelectionText } from "./PreviewGrid";

const gridState = vi.hoisted(() => ({
  area: null as { x: number; y: number; x1: number; y1: number } | null,
}));

vi.mock("@revolist/react-datagrid", () => ({
  RevoGrid: forwardRef(function TestGrid(_props: object, ref) {
    useImperativeHandle(ref, () => ({
      getSelectedRange: async () => gridState.area,
      getFocused: async () => null,
      setCellsFocus: async (
        start: { x: number; y: number },
        end: { x: number; y: number },
      ) => {
        gridState.area = { ...start, x1: end.x, y1: end.y };
      },
    }));
    return (
      <div>
        <div
          role="gridcell"
          tabIndex={0}
          data-rgcol="1"
          data-rgrow="0"
          onMouseDown={(event) => {
            if (event.button === 2 && !event.defaultPrevented)
              gridState.area = null;
          }}
        >
          one two
        </div>
        <div
          role="gridcell"
          tabIndex={0}
          data-rgcol="0"
          data-rgrow="1"
          onMouseDown={(event) => {
            if (event.button === 2 && !event.defaultPrevented)
              gridState.area = null;
          }}
        >
          2026-10-01
        </div>
      </div>
    );
  }),
}));

beforeEach(() => {
  gridState.area = null;
});

const page = {
  schema: {
    fields: [{ name: "date" }, { name: "description" }, { name: "value" }],
  },
  rows: [
    [
      { value: { type: "string", value: "2026-09-30" }, truncated: false },
      { value: { type: "string", value: "one\ttwo" }, truncated: false },
      { value: { type: "string", value: "=cmd" }, truncated: false },
    ],
    [
      { value: { type: "string", value: "2026-10-01" }, truncated: false },
      { value: { type: "null" }, truncated: false },
      { value: null, truncated: true },
    ],
  ],
} as unknown as ApiPreviewV1;

test("range copy includes selected headers, bounds, and spreadsheet-safe values", () => {
  const format = (value: WireValue | null) =>
    value?.type === "null"
      ? "NULL"
      : value?.type === "string"
        ? value.value
        : "";
  expect(previewSelectionText(page, { x: 2, y: 1, x1: 1, y1: 0 }, format)).toBe(
    'description\tvalue\n"one\ttwo"\t\'=cmd\nNULL\tTruncated · value unavailable',
  );
  expect(
    previewSelectionText(page, { x: 0, y: 0, x1: 2, y1: 9999 }, format),
  ).toContain("date\tdescription\tvalue");
});

test("right-click outside a range still copies the selected range", async () => {
  gridState.area = { x: 1, y: 0, x1: 2, y1: 1 };
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  const format = (value: WireValue | null) =>
    value?.type === "null"
      ? "NULL"
      : value?.type === "string"
        ? value.value
        : "";
  render(
    <PreviewGrid
      page={page}
      path="sample"
      dark={false}
      formatCell={format}
      formatType={() => "string"}
      onError={vi.fn()}
    />,
  );
  const otherCell = screen.getByRole("gridcell", { name: "2026-10-01" });
  fireEvent.mouseDown(otherCell, { button: 2 });
  fireEvent.contextMenu(otherCell, {
    clientX: 12,
    clientY: 12,
  });
  expect(gridState.area).toEqual({ x: 1, y: 0, x1: 2, y1: 1 });
  expect(screen.queryByText("May contain sensitive data")).toBeNull();
  fireEvent.click(
    await screen.findByRole("menuitem", { name: "Copy with headers" }),
  );
  await waitFor(() =>
    expect(writeText).toHaveBeenCalledWith(
      'description\tvalue\n"one\ttwo"\t\'=cmd\nNULL\tTruncated · value unavailable',
    ),
  );
});

test.each([
  { metaKey: true, ctrlKey: false },
  { metaKey: false, ctrlKey: true },
])(
  "keyboard copy works for $metaKey/$ctrlKey with selected cells",
  async (modifiers) => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const format = (value: WireValue | null) =>
      value?.type === "null"
        ? "NULL"
        : value?.type === "string"
          ? value.value
          : "";
    render(
      <PreviewGrid
        page={page}
        path="sample"
        dark={false}
        formatCell={format}
        formatType={() => "string"}
        onError={vi.fn()}
      />,
    );
    const selectAll = screen.getByRole("button", {
      name: "Select all displayed cells",
    });
    fireEvent.click(selectAll);
    fireEvent.keyDown(selectAll, { key: "c", ...modifiers });
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith(
        'date\tdescription\tvalue\n2026-09-30\t"one\ttwo"\t\'=cmd\n2026-10-01\tNULL\tTruncated · value unavailable',
      ),
    );
  },
);

test("native copy event copies the selected range with headers", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  render(
    <PreviewGrid
      page={page}
      path="sample"
      dark={false}
      formatCell={(value) =>
        value?.type === "null"
          ? "NULL"
          : value?.type === "string"
            ? value.value
            : ""
      }
      formatType={() => "string"}
      onError={vi.fn()}
    />,
  );
  const selectAll = screen.getByRole("button", {
    name: "Select all displayed cells",
  });
  fireEvent.click(selectAll);
  fireEvent.copy(selectAll);
  await waitFor(() =>
    expect(writeText).toHaveBeenCalledWith(
      'date\tdescription\tvalue\n2026-09-30\t"one\ttwo"\t\'=cmd\n2026-10-01\tNULL\tTruncated · value unavailable',
    ),
  );
});

test("right-click without a selection copies its cell with its header", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  const format = (value: WireValue | null) =>
    value?.type === "string" ? value.value : "";
  render(
    <PreviewGrid
      page={page}
      path="sample"
      dark={false}
      formatCell={format}
      formatType={() => "string"}
      onError={vi.fn()}
    />,
  );
  const cell = screen.getByRole("gridcell", { name: "2026-10-01" });
  fireEvent.contextMenu(cell, { clientX: 12, clientY: 12 });
  fireEvent.click(
    await screen.findByRole("menuitem", { name: "Copy with headers" }),
  );
  await waitFor(() =>
    expect(writeText).toHaveBeenCalledWith("date\n2026-10-01"),
  );
});
