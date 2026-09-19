import { Component, useEffect, useId, useRef } from "react";
import type { ButtonHTMLAttributes, ReactNode } from "react";

export function Button({
  className = "",
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button className={`button ${className}`} type={type} {...props} />;
}

export function Status({ children }: { children: ReactNode }) {
  return (
    <span className="status">
      <span aria-hidden="true" className="status-dot" />
      {children}
    </span>
  );
}

export function Dialog({
  open,
  onClose,
  title,
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    const dialog = ref.current;
    if (open && dialog && !dialog.open) dialog.showModal();
    if (!open && dialog?.open) dialog.close();
    return () => {
      if (dialog?.open) dialog.close();
    };
  }, [open]);
  return (
    <dialog ref={ref} aria-labelledby={titleId} onCancel={onClose}>
      <p className="eyebrow">Development preview</p>
      <h2 id={titleId}>{title}</h2>
      {children}
      <Button className="button-primary" onClick={onClose}>
        Close preview information
      </Button>
    </dialog>
  );
}

export class ErrorBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    if (this.state.failed) {
      return (
        <main className="error-state">
          <h1>The workspace could not be displayed</h1>
          <p>Reload this page to try again. No build was started.</p>
          <Button onClick={() => window.location.reload()}>
            Reload preview
          </Button>
        </main>
      );
    }
    return this.props.children;
  }
}
