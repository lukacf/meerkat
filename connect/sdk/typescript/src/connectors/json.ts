import { visit } from "jsonc-parser";

export type JsonValue = null | boolean | number | string | JsonValue[] | JsonObject;
export interface JsonObject { [key: string]: JsonValue }
const MAX_DEPTH = 128;

function validNumber(value: number): boolean {
  return Number.isFinite(value) && (!Number.isInteger(value) || Number.isSafeInteger(value));
}

/** A plain-data snapshot: never run a getter or toJSON, or silently erase a value. */
export function serviceBody(value: unknown): string {
  const ancestors = new Set<object>();
  function snapshot(input: unknown, depth: number): JsonValue {
    if (input === null || typeof input === "boolean" || typeof input === "string") return input;
    if (typeof input === "number" && validNumber(input) && !Object.is(input, -0)) return input;
    if (typeof input !== "object" || input === null || ancestors.has(input)) throw new TypeError();
    if (depth > MAX_DEPTH) throw new TypeError();
    ancestors.add(input);
    try {
      if (Array.isArray(input)) {
        // Index-only, dense arrays. JSON.stringify otherwise discards extra keys or fills holes.
        const keys = Reflect.ownKeys(input);
        if (keys.length !== input.length + 1) throw new TypeError();
        const result: JsonValue[] = [];
        for (let index = 0; index < input.length; index++) {
          const property = Object.getOwnPropertyDescriptor(input, String(index));
          if (!property || !("value" in property) || !property.enumerable) throw new TypeError();
          result.push(snapshot(property.value, depth + 1));
        }
        return result;
      }
      if (Object.getPrototypeOf(input) !== Object.prototype && Object.getPrototypeOf(input) !== null) {
        throw new TypeError();
      }
      const result: JsonObject = Object.create(null) as JsonObject;
      for (const key of Reflect.ownKeys(input)) {
        const property = Object.getOwnPropertyDescriptor(input, key);
        if (typeof key !== "string" || !property || !("value" in property) || !property.enumerable) {
          throw new TypeError();
        }
        result[key] = snapshot(property.value, depth + 1);
      }
      return result;
    } finally {
      ancestors.delete(input);
    }
  }
  try {
    if (!isObject(value)) throw new TypeError();
    return JSON.stringify(snapshot(value, 1));
  } catch {
    // A host Proxy can throw secrets from reflection. Never retain its exception.
    throw new TypeError("service data must be a plain finite JSON object in the supported profile");
  }
}

export function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Strict JSON plus duplicate-key, finite-number and supported-depth validation. */
export function receiptJson(text: string): JsonValue {
  let depth = 0;
  const objects: Set<string>[] = [];
  const enter = (): void => { if (++depth > MAX_DEPTH) throw new TypeError(); };
  visit(text, {
    onObjectBegin: () => { enter(); objects.push(new Set()); },
    onObjectEnd: () => { objects.pop(); depth--; },
    onArrayBegin: enter,
    onArrayEnd: () => { depth--; },
    onObjectProperty: (key) => {
      const keys = objects[objects.length - 1];
      if (!keys || keys.has(key)) throw new TypeError();
      keys.add(key);
    },
    onLiteralValue: (value: unknown) => {
      if (typeof value === "number" && !validNumber(value)) throw new TypeError();
    },
    onError: () => { throw new TypeError(); },
  }, { disallowComments: true, allowTrailingComma: false, allowEmptyContent: false });
  // Native parsing also refuses non-JSON whitespace; no fault-tolerant result is admitted.
  return JSON.parse(text) as JsonValue;
}
