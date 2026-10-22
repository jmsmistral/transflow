import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { Client } from "../api/client";
import { Workspace } from "../workspace";
import { context, dataset, fixtureTransport, json } from "../test-fixtures";
import { CatalogueSearch } from "./Search";

function page(start: number, count: number, next: string | null, total = 120) {
  return {
    entries: Array.from({ length: count }, (_, offset) => ({
      ...dataset,
      dataset_id: `90000000-0000-4000-8000-${String(start + offset).padStart(12, "0")}`,
      path: `sample/dataset_${String(start + offset).padStart(3, "0")}`,
    })),
    total: String(total),
    next_cursor: next,
    schema: null,
    freshness: "unknown",
  };
}
function scrollList() {
  const list = screen.getByRole("list", { name: "Catalogue datasets" });
  Object.defineProperties(list, {
    clientHeight: { configurable: true, value: 300 },
    scrollHeight: { configurable: true, value: 3000 },
  });
  fireEvent.scroll(list, { target: { scrollTop: 2500 } });
  return list;
}

test("Catalogue appends bounded cursor pages on scroll and ignores overlapping requests", async () => {
  const requests: Record<string, string>[] = [];
  let complete: ((response: Response) => void) | undefined;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, query) => {
        if (!path.endsWith("/datasets") || query.fuzzy === undefined)
          return undefined;
        requests.push(query);
        expect(query.limit).toBe("50");
        if (query.cursor === "next-50")
          return new Promise<Response>((resolve) => {
            complete = resolve;
          });
        return Promise.resolve(
          json(
            query.cursor ? page(100, 20, null) : page(0, 50, "next-50"),
            context(query.branch),
          ),
        );
      }),
    ),
  );
  await workspace.connect("launch");
  const onPage = vi.fn(),
    select = vi.fn();
  render(
    <CatalogueSearch
      workspace={workspace}
      active
      visible={[]}
      select={select}
      onPage={onPage}
    />,
  );
  await screen.findByText("50 of 120 datasets");
  expect(screen.queryByRole("button", { name: "Next page" })).toBeNull();
  expect(screen.queryByRole("button", { name: "First page" })).toBeNull();
  const list = screen.getByRole("list", { name: "Catalogue datasets" });
  Object.defineProperties(list, {
    clientHeight: { configurable: true, value: 300 },
    scrollHeight: { configurable: true, value: 3000 },
  });
  fireEvent.scroll(list, { target: { scrollTop: 100 } });
  fireEvent.wheel(list, { deltaX: 200 });
  expect(requests).toHaveLength(1);
  scrollList();
  scrollList();
  await waitFor(() => expect(complete).toBeDefined());
  scrollList();
  expect(requests).toHaveLength(2);
  await act(async () => {
    complete?.(json(page(50, 50, "next-100"), context()));
  });
  await screen.findByText("100 of 120 datasets");
  expect(
    screen.getByRole("button", { name: /sample\/dataset_000/ }),
  ).toBeTruthy();
  expect(
    screen.getByRole("button", { name: /sample\/dataset_099/ }),
  ).toBeTruthy();
  expect(list.scrollTop).toBe(2500);
  scrollList();
  await screen.findByText("120 datasets");
  scrollList();
  expect(requests.map((query) => query.cursor)).toEqual([
    undefined,
    "next-50",
    "next-100",
  ]);
  await waitFor(() => expect(onPage.mock.lastCall?.[0]).toHaveLength(120));
  fireEvent.click(screen.getByRole("button", { name: /sample\/dataset_000/ }));
  expect(select.mock.lastCall?.[0].path).toBe("sample/dataset_000");
  workspace.dispose();
});

test("Catalogue wheel advances a short page and failed paging retains entries until explicit retry", async () => {
  let pages = 0;
  const workspace = new Workspace(
    new Client(
      fixtureTransport((path, query) => {
        if (!path.endsWith("/datasets") || query.fuzzy === undefined)
          return undefined;
        pages += 1;
        if (pages === 2) return Promise.reject(new Error("Page unavailable"));
        return Promise.resolve(
          json(
            query.cursor ? page(1, 1, null, 2) : page(0, 1, "second", 2),
            context(query.branch),
          ),
        );
      }),
    ),
  );
  await workspace.connect("launch");
  render(
    <CatalogueSearch
      workspace={workspace}
      active
      visible={[]}
      select={vi.fn()}
    />,
  );
  await screen.findByText("1 of 2 datasets");
  const list = screen.getByRole("list", { name: "Catalogue datasets" });
  fireEvent.wheel(list, { deltaY: 100 });
  await screen.findByRole("alert");
  expect(
    screen.getByRole("button", { name: /sample\/dataset_000/ }),
  ).toBeTruthy();
  fireEvent.wheel(list, { deltaY: 100 });
  expect(pages).toBe(2);
  fireEvent.click(
    screen.getByRole("button", { name: "Retry loading datasets" }),
  );
  await screen.findByText("2 datasets");
  expect(screen.queryByRole("alert")).toBeNull();
  expect(pages).toBe(3);
  workspace.dispose();
});

test.each(["filter", "branch"])(
  "Catalogue resets cursors on %s changes and rejects late pages",
  async (change) => {
    const requests: Record<string, string>[] = [];
    let complete: ((response: Response) => void) | undefined;
    const workspace = new Workspace(
      new Client(
        fixtureTransport((path, query) => {
          if (!path.endsWith("/datasets") || query.fuzzy === undefined)
            return undefined;
          requests.push(query);
          if (query.cursor)
            return new Promise<Response>((resolve) => {
              complete = resolve;
            });
          const changed = query.fuzzy === "other" || query.branch === "feature";
          return Promise.resolve(
            json(
              changed ? page(100, 1, null, 1) : page(0, 50, "old-next"),
              context(query.branch),
            ),
          );
        }),
      ),
    );
    await workspace.connect("launch");
    render(
      <CatalogueSearch
        workspace={workspace}
        active
        visible={[]}
        select={vi.fn()}
      />,
    );
    await screen.findByText("50 of 120 datasets");
    scrollList();
    await waitFor(() => expect(complete).toBeDefined());
    if (change === "filter")
      fireEvent.change(screen.getByRole("searchbox"), {
        target: { value: "other" },
      });
    else
      await act(async () => {
        await workspace.select({ branch: "feature" });
      });
    await screen.findByRole("button", { name: /sample\/dataset_100/ });
    expect(requests.at(-1)?.cursor).toBeUndefined();
    await act(async () => {
      complete?.(json(page(50, 50, null), context()));
    });
    expect(
      screen.queryByRole("button", { name: /sample\/dataset_050/ }),
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: /sample\/dataset_000/ }),
    ).toBeNull();
    expect(screen.getByText("1 datasets")).toBeTruthy();
    workspace.dispose();
  },
);
