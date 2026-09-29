/** Local SVG icon set: fixed metrics, currentColor, and no font/CDN dependency. */
const paths = {
  new: "M14 2H4v20h16V8zM14 2v6h6M8 15h8M12 11v8",
  save: "M4 3h13l4 4v14H3V3zM7 3v6h10V3M7 21v-8h10v8",
  edit: "M4 16L16 4l4 4L8 20H4zM14 6l4 4",
  open: "M3 7h7l2 3h9l-3 11H3zM3 7V3h7l2 3h8v4",
  export: "M12 16V3M7 8l5-5 5 5M3 14v7h18v-7",
  undo: "M3 10h10a7 7 0 0 1 7 7M3 10l6-6M3 10l6 6",
  redo: "M21 10H11a7 7 0 0 0-7 7M21 10l-6-6M21 10l-6 6",
  legend: "M3 4h4v4H3zM3 10h4v4H3zM3 16h4v4H3zM11 6h10M11 12h10M11 18h10",
  search: "M21 21l-5-5 M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0",
  list: "M9 5h12M9 12h12M9 19h12M3 5h1M3 12h1M3 19h1",
  build: "M14 3l7 7-4 4-3-3-9 10-3-3 10-9-2-3z",
  calendar: "M3 5h18v16H3zM3 10h18M7 2v6M17 2v6M7 14h2M13 14h2M7 18h2M13 18h2",
  health: "M12 2l9 4v6c0 5-5 8-9 10-4-2-9-5-9-10V6zM7 12l3 3 7-7",
  branch:
    "M6 8v8M9 19h3a6 6 0 0 0 6-6V8M9 5a3 3 0 1 1-6 0 3 3 0 0 1 6 0M9 19a3 3 0 1 1-6 0 3 3 0 0 1 6 0M21 5a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
  left: "M15 5l-7 7 7 7",
  right: "M9 5l7 7-7 7",
  up: "M5 15l7-7 7 7",
  down: "M5 9l7 7 7-7",
  collapse: "M5 5l7 7-7 7M13 5l7 7-7 7",
  reveal: "M11 5l-7 7 7 7M19 5l-7 7 7 7",
  layout: "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z",
  select: "M4 3l16 8-7 2-2 7z",
  expand: "M3 12h18M7 8l-4 4 4 4M17 8l4 4-4 4",
  align:
    "M3 3h4v4H3zM10 3h4v4h-4zM17 3h4v4h-4zM3 10h4v4H3zM10 10h4v4h-4zM17 10h4v4h-4zM3 17h4v4H3zM10 17h4v4h-4zM17 17h4v4h-4z",
  grip: "M5 5h14M5 10h14M5 15h14M5 20h14",
  remove: "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM7 12h10",
  fit: "M8 3H3v5M16 3h5v5M3 16v5h5M21 16v5h-5",
  external: "M8 3h13v13M21 3L3 21",
  table: "M3 3h18v18H3zM3 9h18M9 9v12",
  transform: "M3 7h5l8 10h5M3 17h5L16 7h5M18 4l3 3-3 3M18 14l3 3-3 3",
  close: "M5 5l14 14M5 19L19 5",
} as const;
export type IconName = keyof typeof paths;
export function Icon({ name }: { name: IconName }) {
  return (
    <svg
      className="icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={paths[name]} />
    </svg>
  );
}
