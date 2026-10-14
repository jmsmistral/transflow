import { forwardRef } from "react";

/** Test-only adapter: no custom elements, layout workers or startup timers. */
export const RevoGrid = forwardRef<
  HTMLTableElement,
  {
    columns: { prop: string; name: string }[];
    source: Record<string, string>[];
    "aria-label"?: string;
    className?: string;
  }
>(function TestGrid({ columns, source, "aria-label": label, className }, ref) {
  return (
    <table ref={ref} aria-label={label} className={className}>
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
