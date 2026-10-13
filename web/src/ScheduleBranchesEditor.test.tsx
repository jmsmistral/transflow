import { useState } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { Client } from "./api/client";
import { Workspace } from "./workspace";
import { context, fixtureTransport, json } from "./test-fixtures";
import {
  ScheduleBranchPicker,
  ScheduleBranchesEditor,
} from "./ScheduleBranchesEditor";

test("schedule branches search all live pages, exclude deleted names and never switch lineage context", async () => {
  const pages: Record<string, string>[] = [];
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, query) => {
        if (path !== "/api/v1/branches") return;
        pages.push(query);
        return Promise.resolve(
          json(
            query.cursor
              ? { entries: [{ name: "later-page" }], next_cursor: null }
              : {
                  entries: [
                    { name: "master" },
                    { name: "deleted", deleted: true },
                  ],
                  next_cursor: "next",
                },
            context(),
          ),
        );
      }),
    ),
  );
  await workspace.connect("test");
  pages.length = 0;
  const selection = workspace.snapshot().selection,
    onChange = vi.fn();
  render(
    <ScheduleBranchPicker
      workspace={workspace}
      label="Data branch"
      value="master"
      onChange={onChange}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Data branch" }));
  await screen.findByRole("button", { name: "later-page" });
  expect(pages).toHaveLength(2);
  expect(pages.every((q) => !q.datasets)).toBe(true);
  expect(screen.queryByRole("button", { name: "deleted" })).toBeNull();
  fireEvent.change(
    screen.getByRole("textbox", { name: "Search data branch" }),
    { target: { value: "later" } },
  );
  expect(screen.queryByRole("button", { name: "master" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "later-page" }));
  expect(onChange).toHaveBeenCalledWith("later-page");
  expect(workspace.snapshot().selection).toEqual(selection);
  expect(screen.queryByRole("dialog")).toBeNull();
  workspace.dispose();
});

test("repeated branch cursors fail without presenting a partial successful list and support retry", async () => {
  let repeated = false;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path) =>
        path === "/api/v1/branches"
          ? Promise.resolve(
              json(
                {
                  entries: [{ name: "master" }],
                  next_cursor: repeated ? "loop" : null,
                },
                context(),
              ),
            )
          : undefined,
      ),
    ),
  );
  await workspace.connect("test");
  repeated = true;
  render(
    <ScheduleBranchPicker
      workspace={workspace}
      label="Data branch"
      value="master"
      onChange={vi.fn()}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Data branch" }));
  expect((await screen.findByRole("alert")).textContent).toContain(
    "Repeated branch page",
  );
  expect(screen.queryByRole("button", { name: "master" })).toBeNull();
  repeated = false;
  fireEvent.click(screen.getByRole("button", { name: "Retry branches" }));
  await screen.findByRole("button", { name: /master/ });
  fireEvent.pointerDown(document.body);
  expect(screen.queryByRole("dialog")).toBeNull();
  workspace.dispose();
});

test("fallback editor adds, orders and removes branches as draft state; Escape preserves lineage context", async () => {
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("test");
  const selection = workspace.snapshot().selection;
  function Draft() {
    const [branch, setBranch] = useState("master"),
      [fallback, setFallback] = useState<readonly string[]>([
        "stable",
        "public",
      ]);
    return (
      <ScheduleBranchesEditor
        workspace={workspace}
        branch={branch}
        fallback={fallback}
        onBranch={setBranch}
        onFallback={setFallback}
      />
    );
  }
  render(<Draft />);
  const order = () =>
    screen.getByRole("list", { name: "Schedule fallback order" }).textContent;
  fireEvent.click(screen.getByRole("button", { name: "Move public up" }));
  expect(order()).toMatch(/public.*stable/);
  fireEvent.click(
    screen.getByRole("button", { name: "Remove stable fallback" }),
  );
  expect(order()).not.toContain("stable");
  fireEvent.click(screen.getByRole("button", { name: "Add fallback branch" }));
  fireEvent.click(await screen.findByRole("button", { name: "feature" }));
  expect(order()).toMatch(/public.*feature/);
  fireEvent.click(screen.getByRole("button", { name: "Data branch" }));
  await screen.findByRole("textbox", { name: "Search data branch" });
  fireEvent.keyDown(
    screen.getByRole("textbox", { name: "Search data branch" }),
    { key: "Escape" },
  );
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: "Data branch" }),
  );
  expect(workspace.snapshot().selection).toEqual(selection);
  workspace.dispose();
});
