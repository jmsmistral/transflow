import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { PreviewGrid, type GridPage } from "./PreviewGrid";
import { emptyPreviewCriteria } from "./preview-query";

const page: GridPage = {
  schema: {
    format_version: 1,
    fields: [
      { name: "amount", nullable: false, logical_type: { type: "i64" } },
    ],
  },
  rows: [[{ value: { type: "i64", value: "42" }, truncated: false }]],
};
test("resized columns survive menus, criteria, loading and appended rows, but reset for another view", () => {
  const props = {
    page,
    path: "raw/orders",
    dark: false,
    formatCell: () => "42",
    formatType: () => "integer",
    onError: vi.fn(),
    criteria: emptyPreviewCriteria,
    onCriteriaChange: vi.fn(),
  };
  const view = render(<PreviewGrid {...props} />);
  fireEvent(
    screen.getByRole("table"),
    new CustomEvent("aftercolumnresize", {
      detail: { 0: { prop: "c0", size: 480 } },
    }),
  );
  const width = () =>
    (screen.getByRole("columnheader", { name: "amount" }) as HTMLElement).style
      .width;
  expect(width()).toBe("480px");
  fireEvent.click(
    screen.getByRole("button", { name: "Column options for amount" }),
  );
  expect(screen.getByRole("dialog")).toBeTruthy();
  expect(width()).toBe("480px");
  const criteria = {
    ...emptyPreviewCriteria,
    sorts: [{ column: "amount", direction: "desc" as const }],
  };
  view.rerender(<PreviewGrid {...props} criteria={criteria} busy />);
  expect(width()).toBe("480px");
  view.rerender(
    <PreviewGrid
      {...props}
      criteria={criteria}
      page={{ ...page, rows: [...page.rows, ...page.rows] }}
    />,
  );
  expect(width()).toBe("480px");
  view.rerender(<PreviewGrid {...props} path="curated/orders" />);
  expect(width()).toBe("170px");
});
