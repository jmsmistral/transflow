import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "../components";
import { Icon } from "../Icons";

/** A locally rendered, keyboard-operable menu placed below its trigger. */
export function Menu({
  label,
  children,
  items,
}: {
  label: string;
  children?: ReactNode;
  items: readonly {
    label: string;
    action: () => void;
    selected?: boolean;
    icon?: ReactNode;
  }[];
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    root.current
      ?.querySelector<HTMLButtonElement>('[role^="menuitem"]')
      ?.focus();
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target))
        setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  return (
    <div
      className="lineage-menu"
      ref={root}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
    >
      <button
        type="button"
        ref={trigger}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            setOpen(true);
          }
        }}
      >
        {children}
        <Icon name="down" />
      </button>
      {open && (
        <div
          className="lineage-menu-list"
          role="menu"
          tabIndex={-1}
          aria-label={label}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              setOpen(false);
              trigger.current?.focus();
            }
            const buttons = [
              ...event.currentTarget.querySelectorAll<HTMLButtonElement>(
                '[role^="menuitem"]',
              ),
            ];
            const index = buttons.findIndex(
              (button) => button === document.activeElement,
            );
            const next =
              event.key === "ArrowDown"
                ? (index + 1) % buttons.length
                : event.key === "ArrowUp"
                  ? (index + buttons.length - 1) % buttons.length
                  : event.key === "Home"
                    ? 0
                    : event.key === "End"
                      ? buttons.length - 1
                      : -1;
            if (next >= 0) {
              event.preventDefault();
              buttons[next]?.focus();
            }
          }}
        >
          {items.map((item) => (
            <Button
              key={item.label}
              role={item.selected === undefined ? "menuitem" : "menuitemradio"}
              {...(item.selected === undefined
                ? {}
                : { "aria-checked": item.selected })}
              onClick={() => {
                setOpen(false);
                trigger.current?.focus();
                item.action();
              }}
            >
              {item.icon}
              <span className="menu-label">{item.label}</span>
              {item.selected !== undefined && (
                <span className="menu-check" aria-hidden="true">
                  {item.selected ? "✓" : ""}
                </span>
              )}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
}
