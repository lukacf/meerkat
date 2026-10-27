// Minimal vitest-globals compatibility over node:test + node:assert.
//
// PRs #281-#290 landed package tests written against vitest globals
// (describe/test/expect) with vitest itself never added as a dependency, so
// none of them executed anywhere (CI or locally). Rather than adopting a new
// test framework, this shim keeps those assertions alive under the repo's
// established esbuild + `node --test` lanes. It implements exactly the
// matchers the package tests use — extend it deliberately, or migrate the
// tests to node:assert, before reaching for more of the vitest surface.
import { describe as nodeDescribe, test as nodeTest } from "node:test";
import assert from "node:assert/strict";

export const describe = nodeDescribe;
export const test = nodeTest;
export const it = nodeTest;

function isSubsetMatch(actual: unknown, expected: unknown): boolean {
  if (expected === null || typeof expected !== "object") {
    try {
      assert.deepStrictEqual(actual, expected);
      return true;
    } catch {
      return false;
    }
  }
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual) || actual.length !== expected.length) return false;
    return expected.every((item, index) => isSubsetMatch(actual[index], item));
  }
  if (actual === null || typeof actual !== "object") return false;
  return Object.entries(expected as Record<string, unknown>).every(([key, value]) =>
    isSubsetMatch((actual as Record<string, unknown>)[key], value),
  );
}

class Expectation {
  constructor(private readonly actual: unknown, private readonly negated = false) {}

  get not(): Expectation {
    return new Expectation(this.actual, !this.negated);
  }

  private check(pass: boolean, message: string): void {
    if (this.negated ? pass : !pass) {
      assert.fail(this.negated ? `expected NOT: ${message}` : message);
    }
  }

  toBe(expected: unknown): void {
    this.check(Object.is(this.actual, expected), `expected ${format(this.actual)} to be ${format(expected)}`);
  }

  toEqual(expected: unknown): void {
    let pass = true;
    try {
      assert.deepStrictEqual(this.actual, expected);
    } catch {
      pass = false;
    }
    this.check(pass, `expected ${format(this.actual)} to deep-equal ${format(expected)}`);
  }

  toMatchObject(expected: unknown): void {
    this.check(isSubsetMatch(this.actual, expected), `expected ${format(this.actual)} to match ${format(expected)}`);
  }

  toContain(expected: unknown): void {
    const actual = this.actual as { includes?: (value: unknown) => boolean };
    this.check(
      Boolean(actual && typeof actual.includes === "function" && actual.includes(expected)),
      `expected ${format(this.actual)} to contain ${format(expected)}`,
    );
  }

  toHaveLength(expected: number): void {
    const length = (this.actual as { length?: number } | null)?.length;
    this.check(length === expected, `expected length ${format(length)} to be ${expected}`);
  }

  toBeNull(): void {
    this.check(this.actual === null, `expected ${format(this.actual)} to be null`);
  }

  toBeUndefined(): void {
    this.check(this.actual === undefined, `expected ${format(this.actual)} to be undefined`);
  }

  toBeDefined(): void {
    this.check(this.actual !== undefined, `expected value to be defined`);
  }

  toBeTruthy(): void {
    this.check(Boolean(this.actual), `expected ${format(this.actual)} to be truthy`);
  }

  toBeFalsy(): void {
    this.check(!this.actual, `expected ${format(this.actual)} to be falsy`);
  }

  toBeGreaterThanOrEqual(expected: number): void {
    this.check(
      typeof this.actual === "number" && this.actual >= expected,
      `expected ${format(this.actual)} >= ${expected}`,
    );
  }

  toMatch(expected: RegExp | string): void {
    const value = String(this.actual);
    const pass = typeof expected === "string" ? value.includes(expected) : expected.test(value);
    this.check(pass, `expected ${format(value)} to match ${format(expected)}`);
  }
}

function format(value: unknown): string {
  if (value instanceof RegExp) return String(value);
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export function expect(actual: unknown): Expectation {
  return new Expectation(actual);
}
