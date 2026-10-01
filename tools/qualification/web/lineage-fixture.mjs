// Public synthetic 500-node / 1,500-edge DAG. Never executes a producer or changes an existing module.
import { writeFileSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
const root = process.argv[2];
if (!root)
  throw new Error("Pass an initialized synthetic workspace directory.");
const imports = [
  "import polars as pl",
  "from transflow import Input, Output, transform",
  "",
];
const directory = join(resolve(root), "src", "synthetic_graph");
mkdirSync(directory);
let edges = 0;
for (let i = 0; i < 500; i++) {
  // Shallow fan-in avoids exponentially many causal paths while stressing 1,500 edges.
  const parents =
    i < 480
      ? []
      : i < 499
        ? Array.from({ length: 75 }, (_, j) => ((i - 480) * 25 + j) % 480)
        : [
            ...Array.from({ length: 19 }, (_, j) => 480 + j),
            ...Array.from({ length: 56 }, (_, j) => j),
          ];
  edges += parents.length;
  const bindings = parents.map(
    (p, j) => `input_${j}=Input("synthetic/n${String(p).padStart(3, "0")}")`,
  );
  bindings.push(`output=Output("synthetic/n${String(i).padStart(3, "0")}")`);
  const source = [...imports];
  source.push(
    `@transform(${bindings.join(", ")})`,
    `def n${i}(${parents.map((_, j) => `input_${j}`).join(", ")}):`,
    '    return pl.DataFrame({"id": [1, 2]})',
    "",
  );
  writeFileSync(
    join(directory, `n${String(i).padStart(3, "0")}.py`),
    source.join("\n"),
    { flag: "wx" },
  );
}
console.log(JSON.stringify({ nodes: 500, edges, target: "synthetic/n499" }));
