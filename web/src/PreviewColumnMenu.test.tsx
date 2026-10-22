import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { PreviewColumnMenu } from "./PreviewColumnMenu";
import { emptyPreviewCriteria } from "./preview-query";
import { PreviewGrid, type GridPage } from "./PreviewGrid";

test("one column popup contains inline filtering, null choices and exactly two sort actions", () => {
  const onChange = vi.fn(),
    onClose = vi.fn();
  const trigger = document.createElement("button");
  document.body.append(trigger);
  const view = render(
    <PreviewColumnMenu
      column="region"
      trigger={trigger}
      criteria={emptyPreviewCriteria}
      onChange={onChange}
      onClose={onClose}
    />,
  );
  expect(screen.getAllByRole("dialog")).toHaveLength(1);
  expect(screen.getByRole("button", { name: "Exclude nulls" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Show only nulls" })).toBeTruthy();
  expect(
    screen.getAllByRole("button", { name: /Sort (ascending|descending)/ }),
  ).toHaveLength(2);
  expect(screen.queryByText("Pin column")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Exclude" }));
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "east" } });
  const form = screen.getByRole("textbox").closest("form");
  if (!form) throw new Error("Missing filter form");
  fireEvent.submit(form);
  expect(onChange).toHaveBeenCalledWith({
    filters: [{ column: "region", kind: "exclude", value: "east" }],
    sort: null,
  });
  expect(onClose).toHaveBeenCalled();
  fireEvent.keyDown(document, { key: "Escape" });
  expect(document.activeElement).toBe(trigger);
  view.unmount();
  trigger.remove();
});

test("filter changes preserve other columns; clicking the selected sort clears it", () => {
  const onChange = vi.fn();
  render(
    <PreviewColumnMenu
      column="region"
      trigger={document.createElement("button")}
      criteria={{
        filters: [
          { column: "id", kind: "not_null" },
          { column: "region", kind: "include", value: "east" },
        ],
        sort: { column: "region", direction: "asc" },
      }}
      onChange={onChange}
      onClose={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Clear region filter" }));
  expect(onChange.mock.calls[0]?.[0].filters).toEqual([
    { column: "id", kind: "not_null" },
  ]);
  fireEvent.click(screen.getByRole("button", { name: "Sort ascending" }));
  expect(onChange.mock.calls[1]?.[0].sort).toBeNull();
});

test("copy and context menu in the filter input remain native instead of copying grid cells", () => {
  const writeText = vi.fn();
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  const page: GridPage = {
    schema: {
      format_version: 1,
      fields: [
        { name: "region", logical_type: { type: "string" }, nullable: true },
      ],
    },
    rows: [[{ value: { type: "string", value: "east" }, truncated: false }]],
  };
  render(
    <PreviewGrid
      page={page}
      path="sample"
      dark={false}
      criteria={emptyPreviewCriteria}
      onCriteriaChange={vi.fn()}
      formatCell={() => "east"}
      formatType={() => "string"}
      onError={vi.fn()}
    />,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Column options for region" }),
  );
  const input = screen.getByRole("textbox", {
    name: "Filter value for region",
  });
  expect(fireEvent.keyDown(input, { key: "c", ctrlKey: true })).toBe(true);
  expect(fireEvent.copy(input)).toBe(true);
  expect(fireEvent.contextMenu(input)).toBe(true);
  expect(writeText).not.toHaveBeenCalled();
  expect(screen.queryByText("Copy with headers")).toBeNull();
});
