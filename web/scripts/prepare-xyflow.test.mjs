import { test } from "node:test";
import assert from "node:assert/strict";
import { prepare } from "./prepare-xyflow.mjs";
test("exact xyflow declaration correction is idempotent and never a runtime patch", () => {
  const source =
    "export type InternalNodeBase<NodeType extends NodeBase = NodeBase> = Omit<NodeType, 'measured'> & { measured: {} };";
  const corrected = prepare("0.0.82", source, true);
  assert.equal(corrected, source.replace("= Omit", "= NodeBase & Omit"));
  assert.equal(prepare("0.0.82", corrected, false), corrected);
  assert.equal(prepare("0.0.82", corrected, true), corrected);
  for (const bad of ["", source + source, corrected + source])
    assert.throws(() => prepare("0.0.82", bad, true));
  assert.throws(() => prepare("0.0.82", source, false));
  assert.throws(() => prepare("0.0.83", source, true));
});
