/** Header-inclusive TSV, with quoted fields and spreadsheet-formula protection. */
export function tabSeparatedText(
  headers: readonly string[],
  rows: readonly (readonly string[])[],
): string {
  const field = (value: string) => {
    const safe = /^[\s]*[=+\-@]/.test(value) ? `'${value}` : value;
    return /[\t\r\n"]/.test(safe) ? `"${safe.replaceAll('"', '""')}"` : safe;
  };
  return [headers, ...rows].map((row) => row.map(field).join("\t")).join("\n");
}
