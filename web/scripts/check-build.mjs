// Compare a second production build byte-for-byte with dist; keep measured hashes.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  writeFile,
} from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, URL } from "node:url";
import process from "node:process";
import assert from "node:assert/strict";

const web = fileURLToPath(new URL("..", import.meta.url));
const output = path.resolve(web, "../target/web");
await mkdir(output, { recursive: true });
const temporary = await mkdtemp(path.join(output, "repeat-build-"));
async function hashes(root, prefix = "") {
  const result = {};
  for (const entry of await readdir(path.join(root, prefix), {
    withFileTypes: true,
  })) {
    const relative = path.posix.join(prefix, entry.name);
    if (entry.isDirectory())
      Object.assign(result, await hashes(root, relative));
    else {
      assert(entry.isFile(), `Unexpected build entry: ${relative}`);
      assert(
        /^(index\.html|\.vite\/manifest\.json|assets\/[\w-]+\.(js|css))$/.test(
          relative,
        ),
        `Unexpected production asset: ${relative}`,
      );
      result[relative] = createHash("sha256")
        .update(await readFile(path.join(root, relative)))
        .digest("hex");
    }
  }
  return Object.fromEntries(
    Object.entries(result).sort(([a], [b]) => a.localeCompare(b)),
  );
}
try {
  const first = await hashes(path.join(web, "dist"));
  assert(
    first["index.html"] && first[".vite/manifest.json"],
    "Missing production entry or manifest",
  );
  execFileSync(
    process.execPath,
    [
      path.join(web, "node_modules/vite/bin/vite.js"),
      "build",
      "--outDir",
      temporary,
    ],
    { cwd: web, stdio: "pipe" },
  );
  const second = await hashes(temporary);
  assert.deepEqual(first, second, "Production builds differ");
  await writeFile(
    path.join(output, "build-hashes.json"),
    JSON.stringify(first, null, 2) + "\n",
  );
  process.stdout.write(
    `Production builds match: ${Object.keys(first).length} local assets.\n`,
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
}
