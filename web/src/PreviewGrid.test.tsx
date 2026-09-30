import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { forwardRef, useImperativeHandle } from "react";
import { expect, test, vi } from "vitest";
import type { ApiPreviewV1, WireValue } from "./generated/contracts";
import { PreviewGrid, previewSelectionText } from "./PreviewGrid";

vi.mock("@revolist/react-datagrid", () => ({
  RevoGrid: forwardRef(function TestGrid(_props: object, ref) {
    useImperativeHandle(ref, () => ({
      getSelectedRange: async () => ({ x: 1, y: 0, x1: 2, y1: 1 }),
      getFocused: async () => null,
      setCellsFocus: async () => undefined,
    }));
    return (
      <div role="gridcell" data-rgcol="1" data-rgrow="0">
        one two
      </div>
    );
  }),
}));

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

test("right-click copies a selected range with its column headers", async () => {
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
  fireEvent.contextMenu(screen.getByRole("gridcell"), {
    clientX: 12,
    clientY: 12,
  });
  fireEvent.click(
    await screen.findByRole("menuitem", { name: "Copy with headers" }),
  );
  await waitFor(() =>
    expect(writeText).toHaveBeenCalledWith(
      'description\tvalue\n"one\ttwo"\t\'=cmd\nNULL\tTruncated · value unavailable',
    ),
  );
});
