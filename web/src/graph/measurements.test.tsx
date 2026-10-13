import { useEffect, useRef } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { ReactFlowProps } from "@xyflow/react";
import { Client } from "../api/client";
import { fixtureTransport } from "../test-fixtures";
import { Workspace } from "../workspace";
import { GraphExplorer } from "./Graph";

// Simulate the renderer's initial size notification, without another resize when
// only role/colour data changes. Actual layout remains a separate Browser journey.
vi.mock("@xyflow/react", async (original) => {
  const actual = await original<typeof import("@xyflow/react")>();
  return {
    ...actual,
    ReactFlow: function MeasuredFlow(props: ReactFlowProps) {
      const ids = (props.nodes ?? []).map((node) => node.id).join("\n");
      const notify = useRef(props.onNodesChange);
      useEffect(() => {
        notify.current = props.onNodesChange;
      }, [props.onNodesChange]);
      useEffect(() => {
        if (!ids) return;
        notify.current?.(
          ids.split("\n").map((id) => ({
            type: "dimensions" as const,
            id,
            dimensions: { width: 250, height: 40 },
          })),
        );
      }, [ids]);
      return (
        <div>
          {(props.nodes ?? []).map((node) => (
            <output key={node.id} data-testid="measured-node">
              {node.measured?.width ?? "unmeasured"} ×{" "}
              {node.measured?.height ?? "unmeasured"}
            </output>
          ))}
        </div>
      );
    },
  };
});

test("controlled nodes retain measured dimensions through role and selection data refreshes", async () => {
  const workspace = new Workspace(new Client(fixtureTransport()));
  await workspace.connect("launch");
  const request = { path: "data/a", revision: 1 };
  const view = render(
    <GraphExplorer workspace={workspace} focusRequest={request} />,
  );
  await waitFor(() => {
    expect(screen.getAllByTestId("measured-node").length).toBeGreaterThan(0);
    expect(
      screen
        .getAllByTestId("measured-node")
        .every((node) => node.textContent === "250 × 40"),
    ).toBe(true);
  });
  view.rerender(
    <GraphExplorer
      workspace={workspace}
      focusRequest={request}
      scheduleActive
    />,
  );
  expect(
    screen
      .getAllByTestId("measured-node")
      .every((node) => node.textContent === "250 × 40"),
  ).toBe(true);
  view.rerender(
    <GraphExplorer
      workspace={workspace}
      focusRequest={{ path: "data/a", revision: 2 }}
    />,
  );
  expect(
    screen
      .getAllByTestId("measured-node")
      .every((node) => node.textContent === "250 × 40"),
  ).toBe(true);
  view.unmount();
  workspace.dispose();
});
