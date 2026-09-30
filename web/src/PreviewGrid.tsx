import { useMemo, useRef, useState } from "react";
import { RevoGrid } from "@revolist/react-datagrid";
import "./revogrid.css";
import type {
  ApiPreviewV1,
  ApiPreviewCellV1,
  WireValue,
} from "./generated/contracts";

type Area = { x: number; y: number; x1: number; y1: number };

function clipboardField(value: string): string {
  // A pasted preview must never turn provider data into a spreadsheet formula.
  const safe = /^[\s]*[=+\-@]/.test(value) ? `'${value}` : value;
  return /[\t\r\n"]/.test(safe) ? `"${safe.replaceAll('"', '""')}"` : safe;
}

export function previewSelectionText(
  page: ApiPreviewV1,
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
    .map((field) => clipboardField(field.name));
  const lines = [headers.join("\t")];
  for (let row = top; row <= bottom; row += 1) {
    lines.push(
      (page.rows[row] ?? [])
        .slice(left, right + 1)
        .map((cell: ApiPreviewCellV1) =>
          clipboardField(
            cell.truncated
              ? "Truncated · value unavailable"
              : formatCell(cell.value),
          ),
        )
        .join("\t"),
    );
  }
  return lines.join("\n");
}

export function PreviewGrid({
  page,
  path,
  dark,
  formatCell,
  formatType,
  onError,
}: {
  page: ApiPreviewV1;
  path: string;
  dark: boolean;
  formatCell: (value: WireValue | null) => string;
  formatType: (index: number) => string;
  onError: (message: string) => void;
}) {
  const grid = useRef<HTMLRevoGridElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; area: Area } | null>(
    null,
  );
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
        cellProperties: ({ rowIndex }: { rowIndex: number }) => ({
          class: page.rows[rowIndex]?.[index]?.truncated
            ? "preview-grid-truncated"
            : page.rows[rowIndex]?.[index]?.value?.type === "null"
              ? "preview-grid-null"
              : "",
          title: formatType(index),
        }),
      })),
    [page, formatType],
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
      onContextMenu={(event) => {
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
        if (clicked)
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
            area: { ...clicked, x1: clicked.x, y1: clicked.y },
          });
        void selectedArea().then((selection) => {
          const containsClicked =
            clicked &&
            selection &&
            clicked.x >= Math.min(selection.x, selection.x1) &&
            clicked.x <= Math.max(selection.x, selection.x1) &&
            clicked.y >= Math.min(selection.y, selection.y1) &&
            clicked.y <= Math.max(selection.y, selection.y1);
          const area = containsClicked
            ? selection
            : clicked
              ? { ...clicked, x1: clicked.x, y1: clicked.y }
              : selection;
          if (!area) return;
          if (clicked && !containsClicked)
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
        onKeyDown={(event) => {
          if (
            (event.metaKey || event.ctrlKey) &&
            event.key.toLowerCase() === "c"
          ) {
            event.preventDefault();
            void copySelection();
          }
        }}
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
          <small>May contain sensitive data</small>
        </div>
      )}
    </div>
  );
}
