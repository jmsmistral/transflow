import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./components";
import "./styles.css";
import { Workspace } from "./workspace";
import { Qualification } from "./Qualification";
const workspace = new Workspace();
const launchCode = window.location.hash.slice(1);
window.history.replaceState(
  null,
  "",
  window.location.pathname + window.location.search,
);
if (launchCode) void workspace.connect(launchCode);
window.addEventListener("pagehide", () => workspace.dispose(), { once: true });

const root = document.getElementById("root");
if (!root) throw new Error("The Transflow application root is missing");
createRoot(root).render(
  <StrictMode>
    <ErrorBoundary>
      <App workspace={workspace} />
      {import.meta.env.MODE === "qualification" && <Qualification />}
    </ErrorBoundary>
  </StrictMode>,
);
