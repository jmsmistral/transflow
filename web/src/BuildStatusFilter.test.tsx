import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { BuildStatusFilter } from "./BuildStatusFilter";

test("status picker searches and selects with the keyboard, then restores focus", async () => {
  const user = userEvent.setup();
  const change = vi.fn();
  render(
    <BuildStatusFilter
      value=""
      statuses={["SUCCEEDED", "FAILED"]}
      onChange={change}
    />,
  );
  const trigger = screen.getByRole("button", {
    name: "Filter by job status: All statuses",
  });
  await user.click(trigger);
  const search = screen.getByRole("textbox", { name: "Search job statuses" });
  expect(document.activeElement).toBe(search);
  expect(
    screen
      .getByRole("option", { name: "All statuses" })
      .getAttribute("aria-selected"),
  ).toBe("true");
  await user.type(search, "suc");
  expect(screen.queryByRole("option", { name: "FAILED" })).toBeNull();
  await user.keyboard("{ArrowDown}{Enter}");
  expect(change).toHaveBeenCalledWith("SUCCEEDED");
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
test("outside clicks dismiss without blocking the clicked widget and Escape restores focus", async () => {
  const user = userEvent.setup();
  const outside = vi.fn();
  render(
    <>
      <BuildStatusFilter
        value="FAILED"
        statuses={["FAILED"]}
        onChange={() => {}}
      />
      <button onClick={outside}>Outside</button>
    </>,
  );
  const trigger = screen.getByRole("button", {
    name: "Filter by job status: FAILED",
  });
  await user.click(trigger);
  fireEvent.pointerMove(screen.getByRole("button", { name: "Outside" }));
  expect(screen.getByRole("dialog")).toBeTruthy();
  await user.click(screen.getByRole("button", { name: "Outside" }));
  expect(outside).toHaveBeenCalledOnce();
  expect(screen.queryByRole("dialog")).toBeNull();
  await user.click(trigger);
  await user.keyboard("{Escape}");
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
test("a selected status remains clearable if live counts no longer contain it", async () => {
  const user = userEvent.setup();
  const change = vi.fn();
  const view = render(
    <BuildStatusFilter
      value="RUNNING"
      statuses={["RUNNING"]}
      onChange={change}
    />,
  );
  view.rerender(
    <BuildStatusFilter
      value="RUNNING"
      statuses={["SUCCEEDED"]}
      onChange={change}
    />,
  );
  await user.click(
    screen.getByRole("button", { name: "Filter by job status: RUNNING" }),
  );
  expect(
    screen
      .getByRole("option", { name: "RUNNING" })
      .getAttribute("aria-selected"),
  ).toBe("true");
  await user.type(screen.getByRole("textbox"), "missing");
  expect(screen.getByText("No matching statuses")).toBeTruthy();
  await user.clear(screen.getByRole("textbox"));
  await user.keyboard("{ArrowUp}");
  expect(document.activeElement).toBe(
    screen.getByRole("option", { name: "RUNNING" }),
  );
  await user.keyboard("{Home}{Enter}");
  expect(change).toHaveBeenCalledWith("");
});
