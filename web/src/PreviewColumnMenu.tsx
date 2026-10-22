import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Icon } from "./Icons";
import type { PreviewCriteria, PreviewFilter } from "./preview-query";

export function PreviewColumnMenu({
  column,
  trigger,
  criteria,
  onChange,
  onClose,
}: {
  column: string;
  trigger: HTMLElement;
  criteria: PreviewCriteria;
  onChange: (criteria: PreviewCriteria) => void;
  onClose: () => void;
}) {
  const activeSort = criteria.sorts.find((sort) => sort.column === column);
  const active = criteria.filters.find((f) => f.column === column);
  const [mode, setMode] = useState<"include" | "exclude">(
    active?.kind === "exclude" ? "exclude" : "include",
  );
  const [value, setValue] = useState(
    active && "value" in active ? active.value : "",
  );
  const panel = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const [position, setPosition] = useState({ left: 0, top: 0 });
  useLayoutEffect(() => {
    const anchor = trigger.getBoundingClientRect();
    const box = panel.current?.getBoundingClientRect();
    const height = box?.height ?? 290,
      width = box?.width ?? 280;
    const below = anchor.bottom + 4;
    setPosition({
      left: Math.max(8, Math.min(anchor.left, window.innerWidth - width - 8)),
      top: Math.max(
        8,
        Math.min(
          below + height <= window.innerHeight
            ? below
            : anchor.top - height - 4,
          window.innerHeight - height - 8,
        ),
      ),
    });
    input.current?.focus();
  }, [trigger]);
  useEffect(() => {
    const outside = (event: Event) => {
      if (
        event.target instanceof Node &&
        !panel.current?.contains(event.target) &&
        !trigger.contains(event.target)
      )
        onClose();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        trigger.focus();
      }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape, true);
    window.addEventListener("resize", onClose);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape, true);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose, trigger]);
  function filter(next: PreviewFilter | null) {
    onChange({
      ...criteria,
      filters: [
        ...criteria.filters.filter((f) => f.column !== column),
        ...(next ? [next] : []),
      ],
    });
    onClose();
    trigger.focus();
  }
  function sort(direction: "asc" | "desc") {
    onChange({
      ...criteria,
      sorts:
        activeSort?.direction === direction
          ? criteria.sorts.filter((sort) => sort.column !== column)
          : activeSort
            ? criteria.sorts.map((sort) =>
                sort.column === column ? { column, direction } : sort,
              )
            : [...criteria.sorts, { column, direction }],
    });
    onClose();
    trigger.focus();
  }
  return (
    <div
      ref={panel}
      role="dialog"
      aria-label={`${column} column options`}
      className="preview-column-menu"
      style={position}
    >
      <section aria-label="Add filter" className="preview-column-filter">
        <div className="preview-filter-heading">
          <Icon name="filter" />
          <strong>Add filter</strong>
          {active && (
            <button
              type="button"
              className="preview-filter-clear"
              aria-label={`Clear ${column} filter`}
              onClick={() => filter(null)}
              title="Clear filter"
            >
              <Icon name="close" />
            </button>
          )}
        </div>
        <div className="preview-filter-modes" aria-label="Filter mode">
          <button
            type="button"
            aria-pressed={mode === "include"}
            onClick={() => setMode("include")}
          >
            Include
          </button>
          <button
            type="button"
            aria-pressed={mode === "exclude"}
            onClick={() => setMode("exclude")}
          >
            Exclude
          </button>
        </div>
        <form
          className="preview-filter-value"
          onSubmit={(event) => {
            event.preventDefault();
            filter({ column, kind: mode, value });
          }}
        >
          <input
            ref={input}
            aria-label={`Filter value for ${column}`}
            maxLength={4096}
            placeholder={`${mode === "include" ? "Include" : "Exclude"} rows with value…`}
            value={value}
            onChange={(event) => setValue(event.target.value)}
            title="Exact value match. Numeric and date values use the column's data type. Nested values use their SQL text representation."
          />
          <button
            type="submit"
            aria-label={`Apply ${column} filter`}
            title="Apply filter"
          >
            <Icon name="check" />
          </button>
        </form>
        <button
          type="button"
          className="preview-column-action"
          aria-pressed={active?.kind === "not_null"}
          onClick={() =>
            filter(
              active?.kind === "not_null" ? null : { column, kind: "not_null" },
            )
          }
        >
          <Icon name="filterExclude" />
          Exclude nulls
        </button>
        <button
          type="button"
          className="preview-column-action"
          aria-pressed={active?.kind === "null"}
          onClick={() =>
            filter(active?.kind === "null" ? null : { column, kind: "null" })
          }
        >
          <Icon name="filterNull" />
          Show only nulls
        </button>
      </section>
      <section className="preview-column-sorts" aria-label="Sort column">
        <button
          type="button"
          className="preview-column-action"
          aria-pressed={activeSort?.direction === "asc"}
          onClick={() => sort("asc")}
          title="Click again to clear this sort"
        >
          <Icon name="sortAscending" />
          Sort ascending
        </button>
        <button
          type="button"
          className="preview-column-action"
          aria-pressed={activeSort?.direction === "desc"}
          onClick={() => sort("desc")}
          title="Click again to clear this sort"
        >
          <Icon name="sortDescending" />
          Sort descending
        </button>
      </section>
    </div>
  );
}
