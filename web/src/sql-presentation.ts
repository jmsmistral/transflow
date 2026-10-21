/** Remove only the shared display margin; retained/executed SQL is untouched. */
export function sqlPresentation(sql: string): string {
  const lines = sql.split("\n");
  let first = 0;
  let end = lines.length;
  while (first < end && !lines[first]?.trim()) first++;
  while (end > first && !lines[end - 1]?.trim()) end--;
  const body = lines.slice(first, end);
  let margin: string | undefined;
  for (const line of body) {
    if (!line.trim()) continue;
    const prefix = /^[\t ]*/u.exec(line)?.[0] ?? "";
    margin ??= prefix;
    while (margin && !prefix.startsWith(margin)) margin = margin.slice(0, -1);
  }
  return body.map((line) => line.slice(margin?.length ?? 0)).join("\n");
}
