import { useEffect, useId, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import type { Workspace } from "./workspace";
import { Icon } from "./Icons";
import {
  quoteSqlName,
  sqlCompletion,
  type SqlCompletion,
} from "./sql-catalogue";

type Choice = { path: string; text: string; detail: string };

function caretPosition(editor: HTMLTextAreaElement, caret: number) {
  const mirror = document.createElement("div");
  const style = getComputedStyle(editor);
  const lineHeight = parseFloat(style.lineHeight) || 20;
  for (const name of [
    "font",
    "padding",
    "border",
    "box-sizing",
    "line-height",
    "letter-spacing",
    "tab-size",
  ])
    mirror.style.setProperty(name, style.getPropertyValue(name));
  Object.assign(mirror.style, {
    position: "fixed",
    visibility: "hidden",
    whiteSpace: "pre",
    top: "0",
    left: "-10000px",
  });
  mirror.textContent = editor.value.slice(0, caret);
  const marker = document.createElement("span");
  marker.textContent = "|";
  mirror.append(marker);
  document.body.append(mirror);
  const position = {
    left: marker.offsetLeft - editor.scrollLeft,
    top: marker.offsetTop - editor.scrollTop + lineHeight,
  };
  mirror.remove();
  const rect = editor.getBoundingClientRect();
  const bottom = rect.top + position.top;
  const below = window.innerHeight - bottom - 12;
  const above = bottom - lineHeight - 12;
  const opensBelow = below >= 180 || below >= above;
  return {
    left: Math.max(
      8,
      Math.min(rect.left + position.left, window.innerWidth - 328),
    ),
    maxHeight: Math.max(0, Math.min(250, opensBelow ? below : above)),
    ...(opensBelow
      ? { top: bottom + 4 }
      : { bottom: Math.max(8, window.innerHeight - bottom + lineHeight + 4) }),
  };
}

export function SqlEditor({
  workspace,
  selectedPath,
  value,
  onChange,
}: {
  workspace: Workspace;
  selectedPath: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const editor = useRef<HTMLTextAreaElement>(null);
  const id = useId();
  const [completion, setCompletion] = useState<SqlCompletion | null>(null);
  const [position, setPosition] = useState<CSSProperties>({ top: 0, left: 0 });
  const [active, setActive] = useState(0);
  const [portal, setPortal] = useState<Element | null>(null);
  const [result, setResult] = useState<{
    query: string;
    choices: Choice[];
    more: boolean;
    error?: string;
  }>();
  const query = completion?.query;
  useEffect(() => {
    if (query === undefined) return;
    const abort = new AbortController();
    const timer = setTimeout(() => {
      void workspace
        .read(
          "ApiDatasetsV1",
          "/api/v1/datasets",
          { fuzzy: query, limit: "12" },
          abort.signal,
        )
        .then(
          (page) => {
            if (abort.signal.aborted) return;
            const choices: Choice[] = page.entries
              .filter(
                (entry) =>
                  !entry.tombstone &&
                  entry.path !== selectedPath &&
                  entry.path !== "dataset",
              )
              .map((entry) => ({
                path: entry.path,
                text: quoteSqlName(entry.path),
                detail:
                  entry.origin === "external" ? "External dataset" : "Dataset",
              }));
            if (selectedPath.toLowerCase().includes(query.toLowerCase()))
              choices.unshift({
                path: selectedPath,
                text: quoteSqlName(selectedPath),
                detail: "Selected dataset",
              });
            setResult({ query, choices, more: !!page.next_cursor });
          },
          (error: unknown) => {
            if (!abort.signal.aborted)
              setResult({
                query,
                choices: [],
                more: false,
                error:
                  error instanceof Error
                    ? error.message
                    : "Catalogue search is unavailable.",
              });
          },
        );
    }, 120);
    return () => {
      clearTimeout(timer);
      abort.abort();
    };
  }, [workspace, query, selectedPath]);
  const current = result?.query === query ? result : undefined;
  const choices = current?.choices ?? [];
  const suggest = (input: HTMLTextAreaElement) => {
    const next =
      input.selectionStart === input.selectionEnd
        ? sqlCompletion(input.value, input.selectionStart)
        : null;
    setCompletion(next);
    setActive(0);
    if (next) {
      setPosition(caretPosition(input, input.selectionStart));
      setPortal(input.closest(".app") ?? document.body);
    }
  };
  const choose = (choice: Choice) => {
    const input = editor.current;
    if (!completion || !input) return;
    const suffix = value.slice(completion.end);
    const inserted = choice.text + (/^\s/.test(suffix) ? "" : " ");
    onChange(value.slice(0, completion.start) + inserted + suffix);
    setCompletion(null);
    requestAnimationFrame(() => {
      input.focus();
      input.setSelectionRange(
        completion.start + inserted.length,
        completion.start + inserted.length,
      );
    });
  };
  return (
    <div className="sql-editor-container">
      <textarea
        ref={editor}
        className="scratchpad-editor"
        aria-label="SQL query"
        spellCheck={false}
        wrap="off"
        aria-autocomplete="list"
        aria-controls={completion ? id : undefined}
        aria-activedescendant={
          completion && choices[active] ? `${id}-${active}` : undefined
        }
        title={`Use ${quoteSqlName(selectedPath)} for the selected dataset. Type FROM or JOIN for catalogue suggestions, or press Ctrl+Space.`}
        value={value}
        onChange={(event) => {
          onChange(event.target.value);
          suggest(event.currentTarget);
        }}
        onClick={(event) => suggest(event.currentTarget)}
        onBlur={() => setCompletion(null)}
        onScroll={() => setCompletion(null)}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing) return;
          if ((event.ctrlKey || event.metaKey) && event.code === "Space") {
            event.preventDefault();
            suggest(event.currentTarget);
          } else if (completion && event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            setCompletion(null);
          } else if (
            completion &&
            choices.length &&
            ["ArrowDown", "ArrowUp"].includes(event.key)
          ) {
            event.preventDefault();
            setActive(
              (old) =>
                (old + (event.key === "ArrowDown" ? 1 : choices.length - 1)) %
                choices.length,
            );
          } else if (
            completion &&
            ["Enter", "Tab"].includes(event.key) &&
            choices[active]
          ) {
            event.preventDefault();
            choose(choices[active]);
          } else if (
            ["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)
          )
            setCompletion(null);
        }}
      />
      {completion &&
        portal &&
        createPortal(
          <div className="sql-completions" style={position}>
            <div className="sql-completions-heading">Datasets</div>
            <div id={id} role="listbox" aria-label="Catalogue suggestions">
              {choices.map((choice, index) => (
                <button
                  key={choice.path}
                  id={`${id}-${index}`}
                  role="option"
                  aria-label={`${choice.path}, ${choice.detail}`}
                  aria-selected={index === active}
                  tabIndex={-1}
                  ref={(element) => {
                    if (index === active)
                      element?.scrollIntoView?.({ block: "nearest" });
                  }}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => choose(choice)}
                >
                  <Icon name="database" />
                  <span>
                    {choice.path}
                    <small>{choice.detail}</small>
                  </span>
                </button>
              ))}
            </div>
            {!current && <p role="status">Searching…</p>}
            {current?.error && <p role="status">{current.error}</p>}
            {current && !current.error && !choices.length && (
              <p role="status">No matching datasets</p>
            )}
            {current?.more && <p>Keep typing to narrow the results</p>}
          </div>,
          portal,
        )}
    </div>
  );
}
