import { useState } from "react";
import { Button, Dialog, Status } from "./components";

export function App() {
  const [dark, setDark] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  return (
    <div className="app" data-theme={dark ? "dark" : "light"}>
      <a className="skip-link" href="#workspace">
        Skip to workspace
      </a>
      <header className="topbar">
        <div className="brand">
          <svg viewBox="0 0 32 32" aria-hidden="true">
            <path d="M5 9h9v14h13M14 16h13" />
            <circle cx="5" cy="9" r="3" />
            <circle cx="27" cy="16" r="3" />
            <circle cx="27" cy="23" r="3" />
          </svg>
          <span>transflow</span>
        </div>
        <span className="topbar-divider" aria-hidden="true" />
        <span className="workspace-label">Workspace</span>
        <div className="topbar-actions">
          <Status>Not connected</Status>
          <Button
            aria-pressed={dark}
            aria-label="Dark theme"
            onClick={() => setDark(!dark)}
          >
            <span aria-hidden="true">{dark ? "☀" : "◐"}</span>
            <span className="theme-label">{dark ? "Light" : "Dark"}</span>
          </Button>
        </div>
      </header>
      <main id="workspace" tabIndex={-1}>
        <div className="page-heading">
          <div>
            <p className="eyebrow">Your local data workspace</p>
            <h1>Everything starts with a connection.</h1>
          </div>
          <span className="preview-badge">Development preview</span>
        </div>
        <section className="workspace-canvas" aria-labelledby="empty-title">
          <div className="canvas-heading">
            <span className="canvas-label">Lineage</span>
            <span className="canvas-note">Awaiting a workspace</span>
          </div>
          <div className="empty-state">
            <svg
              className="lineage-mark"
              viewBox="0 0 240 112"
              aria-hidden="true"
            >
              <path d="M48 56h48q12 0 12-12V28q0-12 12-12h56M108 44v40q0 12 12 12h56" />
              <rect x="12" y="37" width="48" height="38" rx="9" />
              <rect x="174" y="1" width="48" height="32" rx="9" />
              <rect x="174" y="79" width="48" height="32" rx="9" />
              <path
                className="mark-detail"
                d="M26 50h20M26 58h12M188 12h20M188 20h12M188 90h20M188 98h12"
              />
            </svg>
            <p className="eyebrow">A clear view of your data</p>
            <h2 id="empty-title">No workspace connected</h2>
            <p className="empty-description">
              Your datasets and their relationships will appear here.
              <br className="desktop-break" /> This preview isn’t connected to a
              Transflow coordinator yet.
            </p>
            <Button
              className="button-primary"
              onClick={() => setAboutOpen(true)}
            >
              About this preview <span aria-hidden="true">↗</span>
            </Button>
          </div>
          <div className="canvas-footer">
            <span>Local by design</span>
            <span>No dataset information loaded</span>
          </div>
        </section>
        <section className="principles" aria-label="The Transflow workflow">
          <article>
            <span className="step">01</span>
            <div>
              <h3>Write in Python</h3>
              <p>Define datasets with the tools you already use.</p>
            </div>
          </article>
          <article>
            <span className="step">02</span>
            <div>
              <h3>Build with confidence</h3>
              <p>Check results before publishing a new version.</p>
            </div>
          </article>
          <article>
            <span className="step">03</span>
            <div>
              <h3>Follow the lineage</h3>
              <p>Understand the inputs behind every output.</p>
            </div>
          </article>
        </section>
      </main>
      <footer className="app-footer">
        <span>Transflow</span>
        <span>Connected datasets. Clear provenance.</span>
      </footer>
      <Dialog
        open={aboutOpen}
        onClose={() => setAboutOpen(false)}
        title="A foundation for your workspace"
      >
        <p>
          This is an early interface preview. You can try the theme switch and
          keyboard navigation.
        </p>
        <p>
          Connecting workspaces, browsing datasets and running builds are still
          being implemented. The workflow shown here describes the intended
          product.
        </p>
      </Dialog>
    </div>
  );
}
