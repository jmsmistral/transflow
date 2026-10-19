import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { SourceLines, sourceTokens } from "./SourceLines";
import { sourceDiff } from "./execution-model";
afterEach(cleanup);

test("Python and SQL highlighting preserve text and multiline string/comment state", () => {
  const python =
    'import polars as pl\ntext = """first\n# still a string\nlast"""\nreturn 0x12 # comment';
  const lines = sourceTokens(python, "src/example.py");
  expect(lines.map((line) => line.map((t) => t.text).join("")).join("\n")).toBe(
    python,
  );
  expect(lines[2]).toEqual([{ text: "# still a string", kind: "string" }]);
  expect(lines[0]?.[0]?.kind).toBe("keyword");
  const sql =
    "SELECT 'it''s safe', 1.2 FROM data /* comment\nstill comment */ WHERE flag IS NULL";
  const tokens = sourceTokens(sql, "query.sql");
  expect(
    tokens
      .flat()
      .filter((t) => t.kind === "keyword")
      .map((t) => t.text),
  ).toEqual(["SELECT", "FROM", "WHERE", "IS", "NULL"]);
  expect(
    tokens.map((line) => line.map((t) => t.text).join("")).join("\n"),
  ).toBe(sql);
  expect(sourceTokens("<script>if</script>", "other.txt")).toEqual([
    [{ text: "<script>if</script>", kind: "plain" }],
  ]);
});

test("highlighting escapes source markup and keeps independent diff string states", () => {
  const source = {
    text: 'x = """old\n<script>alert(1)</script>\n"""',
    path: "a.py",
  };
  const current = { text: "# new\ndef value(): return 2", path: "a.py" };
  const { container } = render(
    <SourceLines
      source={source}
      current={current}
      diff={sourceDiff(source.text, current.text)}
    />,
  );
  expect(container.querySelector("script")).toBeNull();
  expect(container.querySelector(".diff-removed .syntax-string")).toBeTruthy();
  expect(
    container.querySelector(".diff-added .syntax-keyword")?.textContent,
  ).toBe("def");
  expect(container.textContent).toContain("<script>alert(1)</script>");
});

test("side-by-side diff aligns added lines with placeholders and preserves each file's line numbers", () => {
  const source = { path: "old.py", text: "a\nremoved\nz" };
  const current = { path: "new.py", text: "a\nfirst\nsecond\nz" };
  const { container } = render(
    <SourceLines
      source={source}
      current={current}
      diff={sourceDiff(source.text, current.text)}
    />,
  );
  const columns = container.querySelectorAll(".source-diff-columns section");
  const left = columns[0]?.querySelectorAll("li");
  const right = columns[1]?.querySelectorAll("li");
  expect(left).toHaveLength(4);
  expect(right).toHaveLength(4);
  expect(left?.[2]?.getAttribute("aria-hidden")).toBe("true");
  expect(left?.[3]?.getAttribute("value")).toBe("3");
  expect(right?.[3]?.getAttribute("value")).toBe("4");
  expect(left?.[1]?.className).toBe("diff-removed");
  expect(right?.[1]?.className).toBe("diff-added");
});
