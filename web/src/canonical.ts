/** Canonical JSON v1. Domain selection belongs to backend services, never UI inference. */
export const MAX_CANONICAL_BYTES = 16 * 1024 * 1024;
export type DigestKind =
  "artifact" | "source" | "compute" | "catalog" | "schema";
export interface ContentDigest {
  readonly kind: DigestKind;
  readonly hex: string;
}
export class CanonicalError extends Error {}

/** Unicode scalar ordering differs from JavaScript's default UTF-16 key order. */
function compareKeys(left: string, right: string): number {
  const a = Array.from(left, (ch) => ch.codePointAt(0) ?? 0);
  const b = Array.from(right, (ch) => ch.codePointAt(0) ?? 0);
  for (let i = 0; i < Math.min(a.length, b.length); i += 1) {
    const x = a[i];
    const y = b[i];
    if (x !== undefined && y !== undefined && x !== y) return x - y;
  }
  return a.length - b.length;
}

/** Accept decoded JSON only: no lossy bare numbers, surrogate strings or JS objects. */
export function canonicalJson(value: unknown): Uint8Array<ArrayBuffer> {
  const encoder = new TextEncoder();
  let buffer = new Uint8Array(1024);
  let length = 0;
  function append(text: string) {
    const bytes = encoder.encode(text);
    if (bytes.length > MAX_CANONICAL_BYTES - length) {
      throw new CanonicalError("Canonical metadata exceeds its byte limit");
    }
    if (length + bytes.length > buffer.length) {
      const grown = new Uint8Array(
        Math.min(
          MAX_CANONICAL_BYTES,
          Math.max(buffer.length * 2, length + bytes.length),
        ),
      );
      grown.set(buffer);
      buffer = grown;
    }
    buffer.set(bytes, length);
    length += bytes.length;
  }
  function string(text: string) {
    if (text.length > MAX_CANONICAL_BYTES) {
      throw new CanonicalError("Canonical strings exceed the byte limit");
    }
    append('"');
    // Split at scalar boundaries, with bounded escaping allocations.
    let chunk = "";
    for (const ch of text) {
      const code = ch.codePointAt(0) ?? 0;
      if (code >= 0xd800 && code <= 0xdfff) {
        throw new CanonicalError("Canonical strings require Unicode scalars");
      }
      chunk += ch;
      if (chunk.length >= 256) {
        append(JSON.stringify(chunk).slice(1, -1));
        chunk = "";
      }
    }
    append(JSON.stringify(chunk).slice(1, -1));
    append('"');
  }
  function encode(item: unknown, depth: number) {
    if (depth > 64)
      throw new CanonicalError("Canonical nesting limit exceeded");
    if (item === null) append("null");
    else if (typeof item === "boolean") append(item ? "true" : "false");
    else if (typeof item === "string") string(item);
    else if (typeof item === "number") {
      if (!Number.isSafeInteger(item))
        throw new CanonicalError("Bare numbers must be safe integers");
      append(String(item));
    } else if (Array.isArray(item)) {
      if (Object.keys(item).length !== item.length)
        throw new CanonicalError("Sparse or extended array");
      append("[");
      for (let i = 0; i < item.length; i += 1) {
        if (i) append(",");
        encode(item[i], depth + 1);
      }
      append("]");
    } else if (typeof item === "object") {
      const prototype: unknown = Object.getPrototypeOf(item);
      if (prototype !== Object.prototype && prototype !== null) {
        throw new CanonicalError("Only decoded JSON objects are supported");
      }
      const entries = Object.getOwnPropertyDescriptors(item);
      if (Reflect.ownKeys(item).length !== Object.keys(entries).length) {
        throw new CanonicalError("Symbol members are not JSON");
      }
      append("{");
      let first = true;
      for (const key of Object.keys(entries).sort(compareKeys)) {
        const descriptor = entries[key];
        if (!descriptor || !descriptor.enumerable || !("value" in descriptor)) {
          throw new CanonicalError("Only JSON data properties are supported");
        }
        if (!first) append(",");
        first = false;
        string(key);
        append(":");
        encode(descriptor.value, depth + 1);
      }
      append("}");
    } else throw new CanonicalError("Only decoded JSON values are supported");
  }
  encode(value, 0);
  return buffer.slice(0, length);
}

/** Hash fixed purpose/version prefix + NUL + canonical bytes using browser Web Crypto. */
export async function contentDigest(
  kind: DigestKind,
  value: unknown,
): Promise<ContentDigest> {
  if (!["artifact", "source", "compute", "catalog", "schema"].includes(kind)) {
    throw new CanonicalError("Unknown digest purpose");
  }
  const prefix = new TextEncoder().encode(`transflow.${kind}.v1\0`);
  const bytes = canonicalJson(value);
  const payload = new Uint8Array(prefix.length + bytes.length);
  payload.set(prefix);
  payload.set(bytes, prefix.length);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", payload));
  return Object.freeze({
    kind,
    hex: Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join(
      "",
    ),
  });
}
