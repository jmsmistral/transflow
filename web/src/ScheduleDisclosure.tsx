import { useId, useState, type ReactNode } from "react";

/** Keep draft fields mounted while animating, including invalid unsaved values. */
export function ScheduleDisclosure({
  title,
  description,
  children,
  initialOpen = false,
  className = "",
}: {
  title: string;
  description?: string;
  children: ReactNode;
  initialOpen?: boolean;
  className?: string;
}) {
  const [open, setOpen] = useState(initialOpen);
  const id = useId();
  return (
    <section
      className={`schedule-disclosure ${className}`}
      onInvalidCapture={(event) => {
        if (!open) {
          event.preventDefault();
          setOpen(true);
          const field = event.target;
          requestAnimationFrame(() => {
            if (field instanceof HTMLElement) field.focus();
          });
        }
      }}
    >
      <h4>
        <button
          type="button"
          className="schedule-disclosure-toggle"
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setOpen(!open)}
        >
          <span>
            <span className="schedule-disclosure-title">{title}</span>
            {description && <small>{description}</small>}
          </span>
          <span className="schedule-disclosure-action">
            {open ? "Hide" : "Show"}
          </span>
        </button>
      </h4>
      <div
        id={id}
        className="schedule-disclosure-content"
        data-expanded={open}
        aria-hidden={!open}
        inert={!open}
      >
        <div>
          <div className="schedule-disclosure-body">{children}</div>
        </div>
      </div>
    </section>
  );
}
