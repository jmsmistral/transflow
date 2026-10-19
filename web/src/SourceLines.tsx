import { useMemo } from "react";
import type { sourceDiff } from "./execution-model";

type Token = { text: string; kind: string };
const pythonWords = new Set(
  "False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield match case".split(
    " ",
  ),
);
const sqlWords = new Set(
  "select from where group by having order asc desc limit offset join left right full inner outer cross on as with recursive union all distinct intersect except case when then else end null true false and or not in is like ilike between exists over partition rows range unbounded preceding following current row create table view insert into values update set delete cast filter qualify".split(
    " ",
  ),
);
/** A display-only lexer. Keep source text intact and let React escape every token. */
export function sourceTokens(text: string, path: string): Token[][] {
  const sql = /\.sql$/i.test(path);
  if (!sql && !/\.py$/i.test(path))
    return text.split("\n").map((text) => [{ text, kind: "plain" }]);
  const pattern = sql
    ? /(--[^\n]*|\/\*[\s\S]*?(?:\*\/|$))|('(?:''|[^'])*(?:'|$)|"(?:""|[^"])*(?:"|$))|(\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b)|([A-Za-z_]\w*)/g
    : /(#[^\n]*)|((?:[rRuUbBfF]{1,2})?(?:"""[\s\S]*?(?:"""|$)|'''[\s\S]*?(?:'''|$)|"(?:\\[\s\S]|[^"\\\n])*(?:"|$)|'(?:\\[\s\S]|[^'\\\n])*(?:'|$)))|(\b(?:0[xX][\da-fA-F_]+|0[bB][01_]+|0[oO][0-7_]+|\d[\d_]*(?:\.[\d_]*)?(?:[eE][+-]?[\d_]+)?j?)\b)|([A-Za-z_]\w*)/g;
  const lines: Token[][] = [[]];
  function append(value: string, kind: string) {
    value.split("\n").forEach((text, i) => {
      if (i) lines.push([]);
      if (text) lines.at(-1)?.push({ text, kind });
    });
  }
  let offset = 0;
  for (const match of text.matchAll(pattern)) {
    append(text.slice(offset, match.index), "plain");
    const word = match[4];
    const kind = match[1]
      ? "comment"
      : match[2]
        ? "string"
        : match[3]
          ? "number"
          : word &&
              (sql ? sqlWords.has(word.toLowerCase()) : pythonWords.has(word))
            ? "keyword"
            : "plain";
    append(match[0], kind);
    offset = match.index + match[0].length;
  }
  append(text.slice(offset), "plain");
  return lines;
}
export function SourceLines({
  source,
  current,
  diff,
}: {
  source: { text: string; path: string } | undefined;
  current: { text: string; path: string } | undefined;
  diff: ReturnType<typeof sourceDiff> | null;
}) {
  const before = useMemo(
    () => sourceTokens(source?.text ?? "", source?.path ?? ""),
    [source],
  );
  const after = useMemo(
    () => sourceTokens(current?.text ?? "", current?.path ?? ""),
    [current],
  );
  if (diff) {
    type Line = { number: number; tokens: Token[]; kind: string };
    const left: (Line | undefined)[] = [],
      right: (Line | undefined)[] = [];
    let oldIndex = 0,
      newIndex = 0;
    let removed: Line[] = [],
      added: Line[] = [];
    const flush = () => {
      for (let i = 0; i < Math.max(removed.length, added.length); i++) {
        left.push(removed[i]);
        right.push(added[i]);
      }
      removed = [];
      added = [];
    };
    for (const line of diff) {
      if (line.kind === "same") {
        flush();
        left.push({
          number: oldIndex + 1,
          tokens: before[oldIndex++] ?? [],
          kind: "same",
        });
        right.push({
          number: newIndex + 1,
          tokens: after[newIndex++] ?? [],
          kind: "same",
        });
      } else if (line.kind === "removed") {
        removed.push({
          number: oldIndex + 1,
          tokens: before[oldIndex++] ?? [],
          kind: "removed",
        });
      } else {
        added.push({
          number: newIndex + 1,
          tokens: after[newIndex++] ?? [],
          kind: "added",
        });
      }
    }
    flush();
    return (
      <div className="source-diff-columns">
        {[
          { title: "Data publish version", path: source?.path, lines: left },
          { title: "Latest saved version", path: current?.path, lines: right },
        ].map(({ title, path, lines }) => (
          <section key={title} aria-label={title}>
            <header>
              <h3>{title}</h3>
              <span>{path}</span>
            </header>
            <ol className="source-lines">
              {lines.map((line, i) => (
                <li
                  key={i}
                  value={line?.number}
                  className={`diff-${line?.kind ?? "empty"}`}
                  aria-hidden={!line}
                >
                  <code>
                    {line?.kind === "removed"
                      ? "− "
                      : line?.kind === "added"
                        ? "+ "
                        : "  "}
                    {line?.tokens.length
                      ? line.tokens.map((token, j) => (
                          <span key={j} className={`syntax-${token.kind}`}>
                            {token.text}
                          </span>
                        ))
                      : " "}
                  </code>
                </li>
              ))}
            </ol>
          </section>
        ))}
      </div>
    );
  }
  return (
    <ol className="source-lines">
      {before.map((tokens, i) => (
        <li key={i} className="diff-plain">
          <code>
            {tokens.length
              ? tokens.map((token, j) => (
                  <span key={j} className={`syntax-${token.kind}`}>
                    {token.text}
                  </span>
                ))
              : " "}
          </code>
        </li>
      ))}
    </ol>
  );
}
