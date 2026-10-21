import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import type { ViewPortScrollEvent } from "@revolist/revogrid";

/** Test-only adapter: no custom elements, layout workers or startup timers. */
export const RevoGrid = forwardRef<
  HTMLTableElement,
  {
    columns: { prop: string; name: string }[];
    source: Record<string, string>[];
    "aria-label"?: string;
    className?: string;
    onViewportscroll?: (event: CustomEvent<ViewPortScrollEvent>) => void;
  }
>(function TestGrid(
  { columns, source, "aria-label": label, className, onViewportscroll },
  ref,
) {
  const table = useRef<HTMLTableElement>(null);
  useImperativeHandle(ref, () => {
    const element = table.current;
    if (!element) throw new Error("Test grid has not mounted");
    return Object.assign(element, {
      getProviders: async () => ({
        viewport: { stores: { rgRow: { store: { get: () => 240 } } } },
        dimension: {
          stores: { rgRow: { store: { get: () => source.length * 24 } } },
        },
      }),
    });
  }, [source.length]);
  useEffect(() => {
    const element = table.current;
    const scroll = (event: Event) =>
      onViewportscroll?.(event as CustomEvent<ViewPortScrollEvent>);
    element?.addEventListener("viewportscroll", scroll);
    return () => element?.removeEventListener("viewportscroll", scroll);
  }, [onViewportscroll]);
  return (
    <table ref={table} aria-label={label} className={className}>
      <thead>
        <tr>
          <th>#</th>
          {columns.map((column) => (
            <th key={column.prop}>{column.name}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {source.map((row, index) => (
          <tr key={index}>
            <th>{index + 1}</th>
            {columns.map((column) => (
              <td key={column.prop}>{row[column.prop]}</td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
});
