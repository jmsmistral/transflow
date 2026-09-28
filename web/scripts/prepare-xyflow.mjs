// The generic Omit in 0.0.82 loses its NodeBase constraint under exact optional
// property checking. Restate that existing constraint; runtime JS is unchanged.
import { readFile, writeFile } from "node:fs/promises";
import process from "node:process";
import { URL, pathToFileURL } from "node:url";
export function prepare(version, source, apply) {
  if (version !== "0.0.82")
    throw new Error("Review the xyflow declaration patch for this version");
  const original = "= Omit<NodeType, 'measured'> & {";
  const corrected = "= NodeBase & Omit<NodeType, 'measured'> & {";
  if (source.split(corrected).length === 2 && !source.includes(original))
    return source;
  if (
    apply &&
    source.split(original).length === 2 &&
    !source.includes(corrected)
  )
    return source.replace(original, corrected);
  throw new Error(
    "xyflow declarations are unprepared or changed; run npm run prepare:types and review unexpected changes",
  );
}
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const root = new URL("../node_modules/@xyflow/system/", import.meta.url);
  const { version } = JSON.parse(
    await readFile(new URL("package.json", root), "utf8"),
  );
  const declaration = new URL("dist/esm/types/nodes.d.ts", root);
  const source = await readFile(declaration, "utf8");
  const prepared = prepare(version, source, process.argv[2] === "--apply");
  if (prepared !== source) await writeFile(declaration, prepared);
}
