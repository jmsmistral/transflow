import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Icon } from "./Icons";
import { previewCellFilter, type PreviewFilter } from "./preview-query";
import type { ApiPreviewCellV1 } from "./generated/contracts";

export function PreviewCellMenu({
  x,
  y,
  cell,
  trigger,
  onFilter,
  onCopy,
  onClose,
}: {
  x: number;
  y: number;
  cell: { column: string; value: ApiPreviewCellV1; label: string } | null;
  trigger: HTMLElement | null;
  onFilter: ((filter: PreviewFilter) => void) | undefined;
  onCopy: () => void;
  onClose: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: x, top: y });
  const restore = () => {
    if (trigger?.isConnected) trigger.focus();
  };
  useLayoutEffect(() => {
    const box = panel.current?.getBoundingClientRect();
    setPosition({
      left: Math.max(
        8,
        Math.min(x, window.innerWidth - (box?.width ?? 280) - 8),
      ),
      top: Math.max(
        8,
        Math.min(y, window.innerHeight - (box?.height ?? 130) - 8),
      ),
    });
    panel.current
      ?.querySelector<HTMLButtonElement>("button:not(:disabled)")
      ?.focus();
  }, [x, y]);
  useEffect(() => {
    const outside = (event: Event) => {
      if (
        event.target instanceof Node &&
        !panel.current?.contains(event.target)
      )
        onClose();
    };
    const resized = () => onClose();
    document.addEventListener("pointerdown", outside);
    window.addEventListener("resize", resized);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("resize", resized);
    };
  }, [onClose]);
  const label =
    cell &&
    (cell.label.length > 80 ? cell.label.slice(0, 80) + "…" : cell.label);
  return (
    <div
      ref={panel}
      className="preview-grid-menu"
      role="menu"
      tabIndex={-1}
      aria-label="Preview cell actions"
      style={position}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onClose();
          restore();
        }
        if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          const buttons = [
            ...(panel.current?.querySelectorAll<HTMLButtonElement>(
              "button:not(:disabled)",
            ) ?? []),
          ];
          const index = buttons.indexOf(
            document.activeElement as HTMLButtonElement,
          );
          const next =
            event.key === "Home"
              ? 0
              : event.key === "End"
                ? buttons.length - 1
                : (index +
                    (event.key === "ArrowUp" ? -1 : 1) +
                    buttons.length) %
                  buttons.length;
          buttons[next]?.focus();
        }
        if (event.key === "Tab") {
          onClose();
          restore();
        }
      }}
    >
      {cell && onFilter && (
        <div className="preview-cell-filters">
          {(["include", "exclude"] as const).map((kind) => {
            const filter = previewCellFilter(cell.column, cell.value, kind);
            return (
              <button
                key={kind}
                type="button"
                role="menuitem"
                disabled={!filter}
                title={
                  filter
                    ? cell.label
                    : "A complete scalar cell value is required for filtering."
                }
                onClick={() => {
                  if (filter) {
                    onFilter(filter);
                    onClose();
                    restore();
                  }
                }}
              >
                <Icon
                  name={kind === "include" ? "filterInclude" : "filterExclude"}
                />
                <span>
                  {kind === "include" ? "Include only" : "Exclude"} {label}
                </span>
              </button>
            );
          })}
        </div>
      )}
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          onCopy();
          onClose();
          restore();
        }}
      >
        <Icon name="copy" />
        <span>Copy with headers</span>
      </button>
    </div>
  );
}
