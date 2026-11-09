/**
 * Tests for module authoring helpers and console route builders.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  defineModuleSpec,
  decorateModuleSpec,
  decorateModuleTool,
  defineModuleTool,
  defineModule,
  buildConsoleRoute,
  buildConsoleModulesRoute,
  buildConsoleExperienceRoute,
  buildConsoleRoutes,
} from "../dist/index.js";

// ---------------------------------------------------------------------------
// defineModuleSpec
// ---------------------------------------------------------------------------

describe("defineModuleSpec()", () => {
  it("creates a spec with defaults", () => {
    const spec = defineModuleSpec({ id: "mod-a", command: "/bin/mod-a" });
    assert.equal(spec.id, "mod-a");
    assert.equal(spec.command, "/bin/mod-a");
    assert.deepEqual(spec.args, []);
    assert.equal(spec.restart_policy, "never");
  });

  it("creates a spec with custom args and restart policy", () => {
    const spec = defineModuleSpec({
      id: "mod-b",
      command: "/bin/mod-b",
      args: ["--verbose", "--port=8080"],
      restartPolicy: "always",
    });
    assert.equal(spec.id, "mod-b");
    assert.equal(spec.command, "/bin/mod-b");
    assert.deepEqual(spec.args, ["--verbose", "--port=8080"]);
    assert.equal(spec.restart_policy, "always");
  });

  it("supports on_failure restart policy", () => {
    const spec = defineModuleSpec({
      id: "mod-c",
      command: "cmd",
      restartPolicy: "on_failure",
    });
    assert.equal(spec.restart_policy, "on_failure");
  });

  it("preserves module boundary for gateway init payloads", () => {
    const spec = defineModuleSpec({
      id: "router",
      command: "python3",
      boundary: "mcp",
    });

    assert.equal(spec.boundary, "mcp");
  });
});

// ---------------------------------------------------------------------------
// decorateModuleSpec
// ---------------------------------------------------------------------------

describe("decorateModuleSpec()", () => {
  it("applies decorators in left-to-right order", () => {
    const base = defineModuleSpec({ id: "mod", command: "cmd", args: ["a"] });

    const addB = (spec: any) => ({ ...spec, args: [...spec.args, "b"] });
    const addC = (spec: any) => ({ ...spec, args: [...spec.args, "c"] });

    const result = decorateModuleSpec(base, addB, addC);
    assert.deepEqual(result.args, ["a", "b", "c"]);
  });

  it("returns a copy — does not modify original", () => {
    const base = defineModuleSpec({ id: "mod", command: "cmd", args: ["x"] });
    const noop = (spec: any) => spec;
    const result = decorateModuleSpec(base, noop);

    // result should be a copy (different reference for args)
    assert.notEqual(result.args, base.args);
    assert.deepEqual(result.args, base.args);
  });

  it("copies boundary and env when decorating", () => {
    const base = defineModuleSpec({
      id: "mod",
      command: "cmd",
      boundary: "mcp",
      env: { ROUTER_FIXTURE: "example" },
    });
    const result = decorateModuleSpec(base);

    assert.equal(result.boundary, "mcp");
    assert.notEqual(result.env, base.env);
    assert.deepEqual(result.env, base.env);
  });

  it("works with no decorators", () => {
    const base = defineModuleSpec({ id: "mod", command: "cmd" });
    const result = decorateModuleSpec(base);
    assert.equal(result.id, "mod");
    assert.equal(result.command, "cmd");
    assert.deepEqual(result.args, []);
  });
});

// ---------------------------------------------------------------------------
// decorateModuleTool
// ---------------------------------------------------------------------------

describe("decorateModuleTool()", () => {
  it("applies decorators in right-fold (reversed) order", async () => {
    const order: string[] = [];

    const handler = async (input: string, _ctx: any): Promise<string> => {
      order.push("handler");
      return `result:${input}`;
    };

    // outer wraps inner — right-fold means last decorator is outermost
    const decoratorA = (next: any) => async (input: string, ctx: any) => {
      order.push("A-before");
      const result = await next(input, ctx);
      order.push("A-after");
      return result;
    };

    const decoratorB = (next: any) => async (input: string, ctx: any) => {
      order.push("B-before");
      const result = await next(input, ctx);
      order.push("B-after");
      return result;
    };

    const decorated = decorateModuleTool(handler, decoratorA, decoratorB);
    const ctx = { moduleId: "mod", requestId: "req" };
    const result = await decorated("test", ctx);

    // reduceRight: decoratorB wraps (decoratorA wraps handler)
    // So call order is: B-before, A-before, handler, A-after, B-after
    // Wait — reduceRight means: decorators.reduceRight((next, decorate) => decorate(next), handler)
    // Start with handler, then decoratorB(handler) = B-wrapping-handler
    // then decoratorA(B-wrapping-handler) = A-wrapping-B-wrapping-handler
    // So call order is: A-before, B-before, handler, B-after, A-after
    assert.deepEqual(order, ["A-before", "B-before", "handler", "B-after", "A-after"]);
    assert.equal(result, "result:test");
  });

  it("works with no decorators", async () => {
    const handler = async (input: number) => input * 2;
    const decorated = decorateModuleTool(handler);
    const result = await decorated(5, { moduleId: "m", requestId: "r" } as any);
    assert.equal(result, 10);
  });
});

// ---------------------------------------------------------------------------
// defineModuleTool
// ---------------------------------------------------------------------------

describe("defineModuleTool()", () => {
  it("creates a tool definition with name and handler", () => {
    const handler = async () => "ok";
    const tool = defineModuleTool({ name: "my-tool", handler });
    assert.equal(tool.name, "my-tool");
    assert.equal(tool.description, undefined);
    assert.equal(typeof tool.handler, "function");
  });

  it("includes description when provided", () => {
    const tool = defineModuleTool({
      name: "tool-2",
      handler: async () => {},
      description: "A useful tool",
    });
    assert.equal(tool.description, "A useful tool");
  });

  it("applies decorators to handler", async () => {
    const calls: string[] = [];
    const handler = async (input: string) => {
      calls.push("handler");
      return input;
    };
    const decorator = (next: any) => async (input: string, ctx: any) => {
      calls.push("decorated");
      return next(input, ctx);
    };

    const tool = defineModuleTool({
      name: "dec-tool",
      handler,
      decorators: [decorator],
    });

    await tool.handler("test", { moduleId: "m", requestId: "r" });
    assert.deepEqual(calls, ["decorated", "handler"]);
  });
});

// ---------------------------------------------------------------------------
// defineModule
// ---------------------------------------------------------------------------

describe("defineModule()", () => {
  it("creates a module definition with spec and tools", () => {
    const spec = defineModuleSpec({ id: "mod", command: "cmd", args: ["a"] });
    const tool = defineModuleTool({ name: "tool-1", handler: async () => {} });

    const mod = defineModule({ spec, tools: [tool], description: "My module" });
    assert.equal(mod.spec.id, "mod");
    assert.equal(mod.spec.command, "cmd");
    assert.equal(mod.description, "My module");
    assert.equal(mod.tools.length, 1);
    assert.equal(mod.tools[0].name, "tool-1");
  });

  it("creates copies of spec and tools arrays", () => {
    const spec = defineModuleSpec({
      id: "mod",
      command: "cmd",
      args: ["a"],
      boundary: "mcp",
      env: { ROUTER_FIXTURE: "example" },
    });
    const tools = [defineModuleTool({ name: "t", handler: async () => {} })];

    const mod = defineModule({ spec, tools });

    // Spec args should be a different array reference
    assert.notEqual(mod.spec.args, spec.args);
    assert.deepEqual(mod.spec.args, spec.args);
    assert.equal(mod.spec.boundary, "mcp");
    assert.notEqual(mod.spec.env, spec.env);
    assert.deepEqual(mod.spec.env, spec.env);

    // Tools should be a different array reference
    assert.notEqual(mod.tools, tools);
    assert.deepEqual(mod.tools.length, tools.length);
  });

  it("defaults tools to empty array", () => {
    const spec = defineModuleSpec({ id: "mod", command: "cmd" });
    const mod = defineModule({ spec });
    assert.deepEqual(mod.tools, []);
    assert.equal(mod.description, undefined);
  });
});

// ---------------------------------------------------------------------------
// Console route builders
// ---------------------------------------------------------------------------

describe("buildConsoleRoute()", () => {
  it("returns path without auth token", () => {
    assert.equal(buildConsoleRoute("/console/modules"), "/console/modules");
    assert.equal(buildConsoleRoute("/console/experience"), "/console/experience");
  });

  it("appends auth token as query parameter", () => {
    assert.equal(
      buildConsoleRoute("/console/modules", "tok-123"),
      "/console/modules?auth_token=tok-123",
    );
  });

  it("encodes special characters in auth token", () => {
    const result = buildConsoleRoute("/console/modules", "tok with spaces&special=chars");
    assert.equal(
      result,
      "/console/modules?auth_token=tok%20with%20spaces%26special%3Dchars",
    );
  });
});

describe("buildConsoleModulesRoute()", () => {
  it("returns /console/modules without token", () => {
    assert.equal(buildConsoleModulesRoute(), "/console/modules");
  });

  it("returns /console/modules with token", () => {
    assert.equal(
      buildConsoleModulesRoute("abc"),
      "/console/modules?auth_token=abc",
    );
  });
});

describe("buildConsoleExperienceRoute()", () => {
  it("returns /console/experience without token", () => {
    assert.equal(buildConsoleExperienceRoute(), "/console/experience");
  });

  it("returns /console/experience with token", () => {
    assert.equal(
      buildConsoleExperienceRoute("xyz"),
      "/console/experience?auth_token=xyz",
    );
  });
});

describe("buildConsoleRoutes()", () => {
  it("returns both routes without token", () => {
    const routes = buildConsoleRoutes();
    assert.equal(routes.modules, "/console/modules");
    assert.equal(routes.experience, "/console/experience");
  });

  it("returns both routes with token", () => {
    const routes = buildConsoleRoutes("my-token");
    assert.equal(routes.modules, "/console/modules?auth_token=my-token");
    assert.equal(routes.experience, "/console/experience?auth_token=my-token");
  });
});
