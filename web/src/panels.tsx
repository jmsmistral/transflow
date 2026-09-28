import { useRef } from "react";
import type { KeyboardEvent, PointerEvent } from "react";

/** Native pointer capture and keyboard resizing share the same bounded panel state. */
export function ResizeHandle({
  axis,
  value,
  change,
  controls,
}: {
  axis: "horizontal" | "vertical";
  value: number;
  change: (value: number) => void;
  controls: string;
}) {
  const drag = useRef<{ coordinate: number; value: number } | null>(null);
  const clamp = (n: number): number => Math.max(20, Math.min(55, n));
  const coordinate = (event: PointerEvent): number =>
    axis === "horizontal" ? event.clientX : event.clientY;
  function key(event: KeyboardEvent): void {
    const keys =
      axis === "horizontal"
        ? ["ArrowLeft", "ArrowRight"]
        : ["ArrowUp", "ArrowDown"];
    if (![...keys, "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    change(
      event.key === "Home"
        ? 20
        : event.key === "End"
          ? 55
          : clamp(value + (event.key === keys[0] ? 2 : -2)),
    );
  }
  return (
    // WAI-ARIA window splitters are focusable separators; this plugin treats all separators as static.
    // eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions
    <div
      className={`resizer resizer-${axis}`}
      role="separator"
      // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- Keyboard-operable WAI-ARIA window splitter.
      tabIndex={0}
      aria-label={
        axis === "horizontal" ? "Resize inspector" : "Resize bottom panel"
      }
      aria-orientation={axis === "horizontal" ? "vertical" : "horizontal"}
      aria-controls={controls}
      aria-valuemin={20}
      aria-valuemax={55}
      aria-valuenow={value}
      onKeyDown={key}
      onPointerDown={(event) => {
        drag.current = { coordinate: coordinate(event), value };
        event.currentTarget.setPointerCapture(event.pointerId);
      }}
      onPointerMove={(event) => {
        if (!drag.current) return;
        const bounds =
          event.currentTarget.parentElement?.getBoundingClientRect();
        const size = axis === "horizontal" ? bounds?.width : bounds?.height;
        if (size)
          change(
            clamp(
              drag.current.value +
                ((drag.current.coordinate - coordinate(event)) / size) * 100,
            ),
          );
      }}
      onPointerUp={() => {
        drag.current = null;
      }}
      onLostPointerCapture={() => {
        drag.current = null;
      }}
    />
  );
}
