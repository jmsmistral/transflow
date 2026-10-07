import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { Button } from "./components";
import { Icon } from "./Icons";

export function BuildStatusFilter({
  value,
  statuses,
  onChange,
}: {
  value: string;
  statuses: readonly string[];
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const id = useId();
  const label = (status: string) => status || "All statuses";
  const choices = [
    ...new Set(["", ...statuses, ...(value ? [value] : [])]),
  ].filter((status) =>
    label(status).toLowerCase().includes(search.toLowerCase()),
  );
  useEffect(() => {
    if (!open) return;
    input.current?.focus();
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target))
        setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      root.current
        ?.querySelector<HTMLButtonElement>(":scope > button")
        ?.focus();
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape, true);
    };
  }, [open]);
  const optionKey = (
    event: ReactKeyboardEvent<HTMLElement>,
    current: number,
  ) => {
    if (!choices.length) return;
    let index: number;
    if (event.key === "ArrowDown") index = (current + 1) % choices.length;
    else if (event.key === "ArrowUp")
      index =
        current < 0
          ? choices.length - 1
          : (current - 1 + choices.length) % choices.length;
    else if (current >= 0 && event.key === "Home") index = 0;
    else if (current >= 0 && event.key === "End") index = choices.length - 1;
    else return;
    event.preventDefault();
    document.getElementById(`${id}-option-${index}`)?.focus();
  };
  return (
    <div className="build-status-picker" ref={root}>
      <Button
        aria-label={`Filter by job status: ${label(value)}`}
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-controls={id}
        onClick={() => {
          setSearch("");
          setOpen(!open);
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            setSearch("");
            setOpen(true);
          }
        }}
      >
        <span>{label(value)}</span>
        <Icon name="down" />
      </Button>
      {open && (
        <div
          id={id}
          className="branch-menu build-status-menu"
          role="dialog"
          aria-label="Job status filter"
        >
          <label>
            Search statuses
            <input
              ref={input}
              aria-label="Search job statuses"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              onKeyDown={(event) => optionKey(event, -1)}
            />
          </label>
          <div
            className="branch-results"
            role="listbox"
            aria-label="Job statuses"
          >
            {choices.map((status, index) => (
              <button
                type="button"
                key={status}
                id={`${id}-option-${index}`}
                role="option"
                aria-selected={value === status}
                onKeyDown={(event) => optionKey(event, index)}
                onClick={() => {
                  onChange(status);
                  setOpen(false);
                  root.current
                    ?.querySelector<HTMLButtonElement>(":scope > button")
                    ?.focus();
                }}
              >
                <span aria-hidden="true">{value === status ? "✓" : ""}</span>
                {label(status)}
              </button>
            ))}
            {!choices.length && <p>No matching statuses</p>}
          </div>
        </div>
      )}
    </div>
  );
}
