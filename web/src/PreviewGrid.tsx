import { useEffect, useMemo, useRef, useState } from "react";
import { RevoGrid } from "@revolist/react-datagrid";
import "./revogrid.css";
import type { ColumnTemplateFunc } from "@revolist/revogrid";
import { tabSeparatedText } from "./tabular-text";
import { PreviewColumnMenu } from "./PreviewColumnMenu";
import type { PreviewCriteria } from "./preview-query";
import type {
  ApiPreviewV1,
  ApiPreviewCellV1,
  WireValue,
} from "./generated/contracts";

type Area = { x: number; y: number; x1: number; y1: number };
export type GridPage = Pick<ApiPreviewV1, "schema" | "rows">;
const inColumnMenu = (target: EventTarget | null) =>
  target instanceof Element && !!target.closest(".preview-column-menu");

export function previewSelectionText(
  page: GridPage,
  area: Area,
  formatCell: (value: WireValue | null) => string,
): string {
  const left = Math.max(0, Math.min(area.x, area.x1));
  const right = Math.min(
    page.schema.fields.length - 1,
    Math.max(area.x, area.x1),
  );
  const top = Math.max(0, Math.min(area.y, area.y1));
  const bottom = Math.min(page.rows.length - 1, Math.max(area.y, area.y1));
  if (left > right || top > bottom) return "";
  const headers = page.schema.fields
    .slice(left, right + 1)
    .map((field) => field.name);
  const rows: string[][] = [];
  for (let row = top; row <= bottom; row += 1) {
    rows.push(
      (page.rows[row] ?? [])
        .slice(left, right + 1)
        .map((cell: ApiPreviewCellV1) =>
          cell.truncated
            ? "Truncated · value unavailable"
            : formatCell(cell.value),
        ),
    );
  }
  return tabSeparatedText(headers, rows);
}

export function PreviewGrid({
  page,
  path,
  dark,
  formatCell,
  formatType,
  onError,
  onNearEnd,
  criteria,
  onCriteriaChange,
}: {
  page: GridPage;
  path: string;
  dark: boolean;
  formatCell: (value: WireValue | null) => string;
  formatType: (index: number) => string;
  onError: (message: string) => void;
  onNearEnd?: () => void;
  criteria?: PreviewCriteria;
  onCriteriaChange?: (criteria: PreviewCriteria) => void;
}) {
  const grid = useRef<HTMLRevoGridElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; area: Area } | null>(
    null,
  );
  const [columnMenu, setColumnMenu] = useState<{
    column: string;
    trigger: HTMLElement;
  } | null>(null);
  const previousCriteria = useRef(criteria);
  useEffect(() => {
    if (previousCriteria.current === criteria) return;
    previousCriteria.current = criteria;
    // A new result ordering starts at its first row; appended pages preserve
    // the current scroll position and rectangular selection.
    void grid.current?.scrollToRow?.(0);
    void grid.current?.clearFocus?.();
  }, [criteria]);
  const source = useMemo(
    () =>
      page.rows.map((row) =>
        Object.fromEntries(
          row.map((cell, index) => [
            `c${index}`,
            cell.truncated
              ? "Truncated · value unavailable"
              : formatCell(cell.value),
          ]),
        ),
      ),
    [page, formatCell],
  );
  const columns = useMemo(
    () =>
      page.schema.fields.map((field, index) => ({
        prop: `c${index}`,
        name: field.name,
        size: Math.max(170, Math.min(310, field.name.length * 9 + 52)),
        sortable: false,
        filter: false,
        columnProperties: () => ({ "aria-label": field.name }),
        ...(criteria && onCriteriaChange
          ? {
              columnTemplate: ((h) =>
                h("span", { class: "preview-column-heading" }, [
                  h(
                    "span",
                    { class: "preview-column-name", title: formatType(index) },
                    field.name,
                  ),
                  ...(criteria.sort?.column === field.name
                    ? [
                        h(
                          "span",
                          {
                            class: "preview-column-sort",
                            "aria-label": `Sorted ${criteria.sort.direction === "asc" ? "ascending" : "descending"}`,
                          },
                          criteria.sort.direction === "asc" ? "↑" : "↓",
                        ),
                      ]
                    : []),
                  h(
                    "button",
                    {
                      type: "button",
                      class: `preview-column-button${criteria.filters.some((f) => f.column === field.name) ? " is-filtered" : ""}`,
                      "aria-label": `Column options for ${field.name}`,
                      "aria-haspopup": "dialog",
                      "aria-expanded": columnMenu?.column === field.name,
                      title: `Filter or sort ${field.name}`,
                      onMouseDown: (event: MouseEvent) =>
                        event.stopPropagation(),
                      onClick: (event: MouseEvent) => {
                        event.stopPropagation();
                        setMenu(null);
                        if (event.currentTarget instanceof HTMLElement) {
                          const trigger = event.currentTarget;
                          setColumnMenu((old) =>
                            old?.column === field.name
                              ? null
                              : { column: field.name, trigger },
                          );
                        }
                      },
                    },
                    "▾",
                  ),
                ])) satisfies ColumnTemplateFunc,
            }
          : {}),
        cellProperties: ({ rowIndex }: { rowIndex: number }) => ({
          class: page.rows[rowIndex]?.[index]?.truncated
            ? "preview-grid-truncated"
            : page.rows[rowIndex]?.[index]?.value?.type === "null"
              ? "preview-grid-null"
              : "",
          title: formatType(index),
        }),
      })),
    [page, formatType, criteria, onCriteriaChange, columnMenu?.column],
  );

  async function selectedArea(): Promise<Area | null> {
    const selection = await grid.current?.getSelectedRange();
    if (selection) return selection;
    const focus = await grid.current?.getFocused();
    return focus ? { ...focus.cell, x1: focus.cell.x, y1: focus.cell.y } : null;
  }

  async function copySelection(areaOverride?: Area) {
    setMenu(null);
    const area = areaOverride ?? (await selectedArea());
    if (!area) return;
    const text = previewSelectionText(page, area, formatCell);
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      onError("The selected preview cells could not be copied.");
    }
  }

  return (
    <div
      ref={container}
      className="preview-grid-wrap"
      aria-label={`${path} preview`}
      onWheel={async (event) => {
        if (inColumnMenu(event.target)) return;
        if (!onNearEnd || event.deltaY <= Math.abs(event.deltaX)) return;
        const providers = await grid.current?.getProviders();
        if (!providers) return;
        // A byte-limited page can be shorter than a screen. Let a downward
        // wheel gesture request its next page even without a scrollbar yet.
        const height = providers.viewport.stores.rgRow.store.get("virtualSize");
        const total = providers.dimension.stores.rgRow.store.get("realSize");
        if (height > 0 && total <= height) onNearEnd();
      }}
      onMouseDownCapture={(event) => {
        if (inColumnMenu(event.target)) return;
        // RevoGrid focuses the cell under a right-click on mousedown, which
        // replaces an existing range before the context menu can copy it.
        if (event.button === 2) event.preventDefault();
      }}
      onKeyDownCapture={(event) => {
        if (inColumnMenu(event.target)) return;
        if (
          (event.metaKey || event.ctrlKey) &&
          event.key.toLowerCase() === "c"
        ) {
          event.preventDefault();
          void copySelection();
        }
      }}
      onCopyCapture={(event) => {
        if (inColumnMenu(event.target)) return;
        // Some browsers dispatch their native copy command without exposing the
        // shortcut keydown to the grid. Keep keyboard copy header-inclusive.
        event.preventDefault();
        void copySelection();
      }}
      onContextMenu={(event) => {
        if (inColumnMenu(event.target)) return;
        event.preventDefault();
        const bounds = container.current?.getBoundingClientRect();
        const cellSelector = '[role="gridcell"][data-rgcol][data-rgrow]';
        const target =
          (event.nativeEvent
            .composedPath()
            .find(
              (item) =>
                item instanceof HTMLElement && item.matches(cellSelector),
            ) as HTMLElement | undefined) ??
          document
            .elementFromPoint(event.clientX, event.clientY)
            ?.closest<HTMLElement>(cellSelector);
        const clicked = target
          ? {
              x: Number(target.dataset.rgcol),
              y: Number(target.dataset.rgrow),
            }
          : null;
        void selectedArea().then((selection) => {
          const area =
            selection ??
            (clicked ? { ...clicked, x1: clicked.x, y1: clicked.y } : null);
          if (!area) return;
          if (!selection && clicked)
            void grid.current?.setCellsFocus(clicked, clicked);
          setMenu({
            x: Math.max(
              0,
              Math.min(
                event.clientX - (bounds?.left ?? 0),
                (bounds?.width ?? 200) - 200,
              ),
            ),
            y: Math.max(
              0,
              Math.min(
                event.clientY - (bounds?.top ?? 0),
                (bounds?.height ?? 70) - 70,
              ),
            ),
            area,
          });
        });
      }}
    >
      {columnMenu && criteria && onCriteriaChange && (
        <PreviewColumnMenu
          key={columnMenu.column}
          column={columnMenu.column}
          trigger={columnMenu.trigger}
          criteria={criteria}
          onChange={onCriteriaChange}
          onClose={() => setColumnMenu(null)}
        />
      )}
      {page.rows.length > 0 && (
        <button
          type="button"
          className="preview-grid-select-all"
          aria-label="Select all displayed cells"
          title="Select all displayed cells"
          onClick={() => {
            void grid.current?.setCellsFocus(
              { x: 0, y: 0 },
              { x: page.schema.fields.length - 1, y: page.rows.length - 1 },
            );
          }}
        >
          ▦
        </button>
      )}
      <RevoGrid
        ref={grid}
        onClick={() => setMenu(null)}
        aria-label={`${path} data grid`}
        className="preview-grid"
        theme={dark ? "dark" : "default"}
        columns={columns}
        source={source}
        rowHeaders
        range
        readonly
        useClipboard={false}
        stretch={false}
        resize
        canMoveColumns={false}
        hideAttribution
        onViewportscroll={async ({ detail }) => {
          setColumnMenu(null);
          if (
            !onNearEnd ||
            detail.dimension !== "rgRow" ||
            detail.coordinate <= 0
          )
            return;
          const providers = await grid.current?.getProviders();
          if (!providers) return;
          const height =
            providers.viewport.stores.rgRow.store.get("virtualSize");
          const total = providers.dimension.stores.rgRow.store.get("realSize");
          // Prefetch within one screen of the end, using the grid's logical
          // dimensions rather than its compressed DOM scrollbar coordinates.
          if (height > 0 && detail.coordinate + height * 2 >= total)
            onNearEnd();
        }}
      />
      {menu && (
        <div
          className="preview-grid-menu"
          role="menu"
          style={{ left: menu.x, top: menu.y }}
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => void copySelection(menu.area)}
          >
            Copy with headers
          </button>
        </div>
      )}
    </div>
  );
}
