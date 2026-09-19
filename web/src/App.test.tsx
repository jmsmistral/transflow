import { fireEvent, render, screen } from "@testing-library/react";
import { userEvent } from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { App } from "./App";
import { Button, ErrorBoundary, Status } from "./components";

test("disconnected preview exposes no invented datasets or build controls", () => {
  render(<App />);
  expect(
    screen.getByRole("heading", { name: "No workspace connected" }),
  ).toBeTruthy();
  expect(screen.getByText("No dataset information loaded")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /^build$/i })).toBeNull();
});

test("theme button supports keyboard activation and preserves connection state", async () => {
  const user = userEvent.setup();
  render(<App />);
  const button = screen.getByRole("button", { name: "Dark theme" });
  button.focus();
  await user.keyboard("{Enter}");
  expect(button.getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByText("Not connected")).toBeTruthy();
  await user.keyboard(" ");
  expect(button.getAttribute("aria-pressed")).toBe("false");
});

test("button defaults to non-submit and disabled actions do not fire", async () => {
  const click = vi.fn();
  const user = userEvent.setup();
  render(
    <Button disabled onClick={click}>
      Unavailable action
    </Button>,
  );
  const button = screen.getByRole("button");
  expect(button.getAttribute("type")).toBe("button");
  await user.click(button);
  expect(click).not.toHaveBeenCalled();
});

test("status values are rendered as text", () => {
  render(<Status>{'<img src=x onerror="alert(1)">'}</Status>);
  expect(screen.getByText(/<img/)).toBeTruthy();
  expect(screen.queryByRole("img")).toBeNull();
});

test("render failure has a safe recovery message without exposing error details", () => {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  function Broken(): never {
    throw new Error("synthetic private detail");
  }
  render(
    <ErrorBoundary>
      <Broken />
    </ErrorBoundary>,
  );
  expect(
    screen.getByRole("heading", {
      name: "The workspace could not be displayed",
    }),
  ).toBeTruthy();
  expect(screen.queryByText(/synthetic private detail/)).toBeNull();
});

test("button forwards the accessible name and click callback", () => {
  const click = vi.fn();
  render(
    <Button aria-label="Open details" onClick={click}>
      Details
    </Button>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Open details" }));
  expect(click).toHaveBeenCalledOnce();
});
