/**
 * Tests for MetaBuilder configuration chain.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Meta, MetaBuilder } from "../dist/index.js";

describe("Meta.builder()", () => {
  it("returns a MetaBuilder instance", () => {
    const builder = Meta.builder();
    assert.ok(builder instanceof MetaBuilder);
  });
});

describe("MetaBuilder chainable methods", () => {
  it("mob() sets mobConfigPath and returns this", () => {
    const builder = Meta.builder();
    const result = builder.mob("config/mob.toml");
    assert.equal(result, builder);
    assert.equal(builder._config.mobConfigPath, "config/mob.toml");
  });

  it("preparedApplication() carries the bytes opaquely", () => {
    const builder = Meta.builder();
    const bundle = new Uint8Array([0, 1, 2, 255]);
    const result = builder.preparedApplication(bundle, "sha256:abc");
    assert.equal(result, builder);
    assert.deepEqual(builder._config.preparedApplication, {
      bundle: new Uint8Array([0, 1, 2, 255]),
      digest: "sha256:abc",
    });
    assert.throws(() => Meta.builder().preparedApplication(bundle, ""));
  });

  it("gateway() sets gatewayBin and returns this", () => {
    const builder = Meta.builder();
    const result = builder.gateway("/usr/bin/gateway");
    assert.equal(result, builder);
    assert.equal(builder._config.gatewayBin, "/usr/bin/gateway");
  });

  it("sessionService() sets sessionBuilder", () => {
    const builder = Meta.builder();
    const mockBuilder = { buildAgent: async () => {} };
    const result = builder.sessionService(mockBuilder);
    assert.equal(result, builder);
    assert.equal(builder._config.sessionBuilder, mockBuilder);
  });

  it("onError() sets errorCallback and returns this", () => {
    const builder = Meta.builder();
    const cb = () => {};
    const result = builder.onError(cb);
    assert.equal(result, builder);
    assert.equal(builder._config.errorCallback, cb);
  });

  it("eventLog() sets eventLog config and returns this", () => {
    const builder = Meta.builder();
    const opts = { storage: "file", path: "/tmp/events.log" };
    const result = builder.eventLog(opts);
    assert.equal(result, builder);
    assert.deepEqual(builder._config.eventLog, {
      storage: "file",
      path: "/tmp/events.log",
    });
  });

  it("gating() sets gatingConfigPath and returns this", () => {
    const builder = Meta.builder();
    const result = builder.gating("config/gating.toml");
    assert.equal(result, builder);
    assert.equal(builder._config.gatingConfigPath, "config/gating.toml");
  });

  it("gating() rejects an explicit empty path", () => {
    assert.throws(() => Meta.builder().gating(""), /non-empty/);
  });

  it("gating() names how to keep gating off when given an empty path", () => {
    assert.throws(
      () => Meta.builder().gating(""),
      /an empty path no longer switches gating off\. To keep gating off, do not call gating\(\), and keep nothing at config\/gating\.toml/,
    );
  });

  it("consoleConfig() sets consoleConfigPath and returns this", () => {
    const builder = Meta.builder();
    const result = builder.consoleConfig("config/console.toml");
    assert.equal(result, builder);
    assert.equal(builder._config.consoleConfigPath, "config/console.toml");
  });

  it("accessControl() sets accessConfigPath and returns this", () => {
    const builder = Meta.builder();
    const result = builder.accessControl("config/access.toml");
    assert.equal(result, builder);
    assert.equal(builder._config.accessConfigPath, "config/access.toml");
  });

  it("accessControl() rejects an explicit empty path", () => {
    assert.throws(() => Meta.builder().accessControl(""), /non-empty/);
  });

  it("accessControl() names how to keep access control off when given an empty path", () => {
    assert.throws(
      () => Meta.builder().accessControl(""),
      /an empty path no longer switches access control off\. To keep access control off, do not call accessControl\(\), and keep nothing at config\/access\.toml/,
    );
  });

  it("meerkatConfig() sets meerkatConfigPath and returns this", () => {
    const builder = Meta.builder();
    const result = builder.meerkatConfig(".rkat/config.toml");
    assert.equal(result, builder);
    assert.equal(builder._config.meerkatConfigPath, ".rkat/config.toml");
  });

  it("meerkatConfig() rejects an empty path", () => {
    assert.throws(
      () => Meta.builder().meerkatConfig("  "),
      /meerkatConfig path must not be empty/,
    );
  });

  it("providerCredentialsRoot() sets providerCredentialsRoot and returns this", () => {
    const builder = Meta.builder();
    const result = builder.providerCredentialsRoot(
      "/srv/meta/users/a/credentials",
    );
    assert.equal(result, builder);
    assert.equal(
      builder._config.providerCredentialsRoot,
      "/srv/meta/users/a/credentials",
    );
  });

  it("providerCredentialsRoot() rejects an empty path", () => {
    assert.throws(
      () => Meta.builder().providerCredentialsRoot("  "),
      /providerCredentialsRoot path must not be empty/,
    );
  });

  it("httpListen() trims, sets httpListen and returns this", () => {
    const builder = Meta.builder();
    const result = builder.httpListen(" 0.0.0.0:8080 ");
    assert.equal(result, builder);
    assert.equal(builder._config.httpListen, "0.0.0.0:8080");
  });

  it("httpListen() rejects an empty address", () => {
    assert.throws(
      () => Meta.builder().httpListen("  "),
      /httpListen address must not be empty/,
    );
  });

  it("httpPublicBaseUrl() sets httpPublicBaseUrl and returns this", () => {
    const builder = Meta.builder();
    const result = builder.httpPublicBaseUrl("https://mob.example.com");
    assert.equal(result, builder);
    assert.equal(builder._config.httpPublicBaseUrl, "https://mob.example.com");
  });

  it("allowRemote() defaults to true and accepts an explicit false", () => {
    assert.equal(Meta.builder().allowRemote()._config.allowRemote, true);
    assert.equal(Meta.builder().allowRemote(false)._config.allowRemote, false);
  });

  it("consoleAuthRequired() sets consoleRequireAppAuth and returns this", () => {
    const builder = Meta.builder();
    const result = builder.consoleAuthRequired(false);
    assert.equal(result, builder);
    assert.equal(builder._config.consoleRequireAppAuth, false);
  });

  it("consoleReadOnly() sets consoleReadOnly and returns this", () => {
    const builder = Meta.builder();
    const result = builder.consoleReadOnly();
    assert.equal(result, builder);
    assert.equal(builder._config.consoleReadOnly, true);
  });

  it("consoleFetchTimeoutMs() sets consoleFetchTimeoutMs and returns this", () => {
    const builder = Meta.builder();
    const result = builder.consoleFetchTimeoutMs(120_000);
    assert.equal(result, builder);
    assert.equal(builder._config.consoleFetchTimeoutMs, 120_000);
  });

  it("consoleFetchTimeoutMs() rejects invalid timeouts", () => {
    assert.throws(
      () => Meta.builder().consoleFetchTimeoutMs(0),
      /consoleFetchTimeoutMs must be a positive integer/,
    );
  });

  it("demoLlm() enables deterministic gateway LLM mode and returns this", () => {
    const builder = Meta.builder();
    const result = builder.demoLlm();
    assert.equal(result, builder);
    assert.equal(builder._config.demoLlm, true);
  });

  it("demoLlm(false) disables deterministic gateway LLM mode", () => {
    const builder = Meta.builder().demoLlm();
    builder.demoLlm(false);
    assert.equal(builder._config.demoLlm, false);
  });

  it("memberCommsTcp() configures cross-process peer replies", () => {
    const builder = Meta.builder();
    const result = builder.memberCommsTcp();
    assert.equal(result, builder);
    assert.equal(builder._config.memberCommsAddress, "127.0.0.1:0");
    builder.memberCommsTcp("192.0.2.10:0");
    assert.equal(builder._config.memberCommsAddress, "192.0.2.10:0");
    assert.throws(() => builder.memberCommsTcp(""), /must not be empty/);
  });

  it("maxSessions() sets maxSessions and returns this", () => {
    const builder = Meta.builder();
    const result = builder.maxSessions(320);
    assert.equal(result, builder);
    assert.equal(builder._config.maxSessions, 320);
  });

  it("maxSessions() rejects invalid capacities", () => {
    assert.throws(() => Meta.builder().maxSessions(0), /positive integer/);
    assert.throws(() => Meta.builder().maxSessions(1.5), /positive integer/);
  });

  it("gatewayTimeoutMs() sets gatewayTimeoutMs and returns this", () => {
    const builder = Meta.builder();
    const result = builder.gatewayTimeoutMs(300_000);
    assert.equal(result, builder);
    assert.equal(builder._config.gatewayTimeoutMs, 300_000);
  });

  it("gatewayTimeoutMs() rejects invalid timeouts", () => {
    assert.throws(
      () => Meta.builder().gatewayTimeoutMs(0),
      /positive integer/,
    );
    assert.throws(
      () => Meta.builder().gatewayTimeoutMs(1.5),
      /positive integer/,
    );
  });

  it("routing() sets routingConfigPath and returns this", () => {
    const builder = Meta.builder();
    const result = builder.routing("deployment/routing.toml");
    assert.equal(result, builder);
    assert.equal(builder._config.routingConfigPath, "deployment/routing.toml");
  });

  it("workgraph() defaults to enabled=true and returns this", () => {
    const builder = Meta.builder();
    const result = builder.workgraph();
    assert.equal(result, builder);
    assert.equal(builder._config.workgraphEnabled, true);
  });

  it("workgraph(false) disables WorkGraph construction", () => {
    const builder = Meta.builder();
    builder.workgraph(false);
    assert.equal(builder._config.workgraphEnabled, false);
  });

  it("forkInstructionPolicy() records the policy and returns this", () => {
    const builder = Meta.builder();
    const result = builder.forkInstructionPolicy("reactivate_inherited");
    assert.equal(result, builder);
    assert.equal(builder._config.forkInstructionPolicy, "reactivate_inherited");
  });

  it("forkInstructionPolicy() rejects an unknown name", () => {
    const builder = Meta.builder();
    assert.throws(() => builder.forkInstructionPolicy("inherit" as never), RangeError);
  });

  it("memory() sets memoryConfig with config object and returns this", () => {
    const builder = Meta.builder();
    const cfg = { backend: "local_json" };
    const result = builder.memory(cfg);
    assert.equal(result, builder);
    assert.deepEqual(builder._config.memoryConfig, { backend: "local_json" });
  });

  it("memory() rejects options.stores without a Rust gateway config", () => {
    const builder = Meta.builder();
    assert.throws(
      () => builder.memory(undefined, { stores: ["main", "archive"] }),
      /not supported by the Rust gateway/,
    );
  });

  it("memory() defaults to null when called with no args", () => {
    const builder = Meta.builder();
    builder.memory();
    assert.equal(builder._config.memoryConfig, null);
  });

  it("auth() sets authConfig and returns this", () => {
    const builder = Meta.builder();
    const cfg = { provider: "google" };
    const result = builder.auth(cfg);
    assert.equal(result, builder);
    assert.deepEqual(builder._config.authConfig, { provider: "google" });
  });

  it("implicitDelegateIdleRetirement() sets seconds and returns this", () => {
    const builder = Meta.builder();
    const result = builder.implicitDelegateIdleRetirement(30);
    assert.equal(result, builder);
    assert.equal(builder._config.implicitDelegateIdleRetireSecs, 30);
  });

  it("implicitDelegateIdleRetirement() accepts null to disable", () => {
    const builder = Meta.builder();
    builder.implicitDelegateIdleRetirement(null);
    assert.equal(builder._config.implicitDelegateIdleRetireSecs, null);
  });

  it("implicitDelegateIdleRetirement() rejects negative seconds", () => {
    const builder = Meta.builder();
    assert.throws(() => builder.implicitDelegateIdleRetirement(-1));
  });

  it("modules() sets modules array and returns this", () => {
    const builder = Meta.builder();
    const specs = [{ id: "mod-a", command: "cmd" }];
    const result = builder.modules(specs);
    assert.equal(result, builder);
    assert.deepEqual(builder._config.modules, specs);
  });
});

describe("MetaBuilder default config", () => {
  it("has null/empty defaults for all fields", () => {
    const builder = Meta.builder();
    const cfg = builder._config;
    assert.equal(cfg.mobConfigPath, null);
    assert.equal(cfg.sessionBuilder, null);
    assert.equal(cfg.errorCallback, null);
    assert.equal(cfg.eventLog, null);
    assert.equal(cfg.consoleConfigPath, null);
    assert.equal(cfg.meerkatConfigPath, null);
    assert.equal(cfg.providerCredentialsRoot, null);
    assert.equal(cfg.httpListen, null);
    assert.equal(cfg.httpPublicBaseUrl, null);
    assert.equal(cfg.allowRemote, null);
    assert.equal(cfg.consoleRequireAppAuth, null);
    assert.equal(cfg.consoleReadOnly, null);
    assert.equal(cfg.consoleFetchTimeoutMs, null);
    assert.equal(cfg.demoLlm, false);
    assert.equal(cfg.memberCommsAddress, null);
    assert.equal(cfg.gatingConfigPath, null);
    assert.equal(cfg.routingConfigPath, null);
    assert.equal(cfg.workgraphEnabled, null);
    assert.equal(cfg.forkInstructionPolicy, null);
    assert.equal(cfg.memoryConfig, null);
    assert.equal(cfg.authConfig, null);
    assert.equal(cfg.implicitDelegateIdleRetireSecs, undefined);
    assert.equal(cfg.maxSessions, null);
    assert.equal(cfg.gatewayTimeoutMs, null);
    assert.equal(cfg.gatewayBin, null);
    assert.deepEqual(cfg.modules, []);
  });
});

describe("MetaBuilder convention defaults", () => {
  it("does not set gating if file does not exist", () => {
    // Convention defaults look for config/gating.toml — which won't
    // exist in the test environment, so gatingConfigPath stays null.
    const builder = Meta.builder();
    // Manually call the private method via build path check
    // We can test by checking the config before build would set it:
    assert.equal(builder._config.gatingConfigPath, null);
  });

  it("does not set routing if file does not exist", () => {
    const builder = Meta.builder();
    assert.equal(builder._config.routingConfigPath, null);
  });

  it("explicit accessControl overrides convention auto-discovery", () => {
    // Convention defaults look for config/access.toml; an explicit
    // accessControl() value must survive _applyConventionDefaults()
    // regardless of whether the conventional file exists.
    const builder = Meta.builder()
      .mob("config/mob.toml")
      .accessControl("custom/access.toml");
    (builder as any)._applyConventionDefaults();
    assert.equal(builder._config.accessConfigPath, "custom/access.toml");
  });

  it("forwards anything at config/access.toml so the gateway refuses it", () => {
    // Only a genuinely absent entry leaves access control off. A directory,
    // a dangling link or a lookup that fails other than by absence is
    // forwarded so the gateway's strict loader refuses it.
    const previous = process.cwd();
    const root = mkdtempSync(join(tmpdir(), "meta-access-convention-"));
    const discovered = (): string | null => {
      const builder = Meta.builder().mob("config/mob.toml");
      (builder as any)._applyConventionDefaults();
      return builder._config.accessConfigPath;
    };
    try {
      process.chdir(root);
      assert.equal(discovered(), null);

      mkdirSync("config/access.toml", { recursive: true });
      assert.equal(discovered(), "config/access.toml");

      if (process.platform !== "win32") {
        rmSync("config", { recursive: true });
        mkdirSync("config");
        symlinkSync(join(root, "missing.toml"), "config/access.toml");
        assert.equal(discovered(), "config/access.toml");

        // With config a file, the lookup reports "not a directory".
        rmSync("config", { recursive: true });
        writeFileSync("config", "");
        assert.equal(discovered(), "config/access.toml");
      }
    } finally {
      process.chdir(previous);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("forwards anything at config/gating.toml so the gateway refuses it", () => {
    // Like access.toml, only ENOENT leaves gating off. A directory, a
    // dangling link or another lookup error is forwarded so the gateway's
    // strict loader refuses it.
    const previous = process.cwd();
    const root = mkdtempSync(join(tmpdir(), "meta-gating-convention-"));
    const discovered = (): string | null => {
      const builder = Meta.builder().mob("config/mob.toml");
      (builder as any)._applyConventionDefaults();
      return builder._config.gatingConfigPath;
    };
    try {
      process.chdir(root);
      assert.equal(discovered(), null);

      mkdirSync("config/gating.toml", { recursive: true });
      assert.equal(discovered(), "config/gating.toml");

      if (process.platform !== "win32") {
        rmSync("config", { recursive: true });
        mkdirSync("config");
        symlinkSync(join(root, "missing.toml"), "config/gating.toml");
        assert.equal(discovered(), "config/gating.toml");

        // With config a file, the lookup reports "not a directory".
        rmSync("config", { recursive: true });
        writeFileSync("config", "");
        assert.equal(discovered(), "config/gating.toml");
      }
    } finally {
      process.chdir(previous);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("omits the opt-in files under a dangling config directory link", () => {
    // ENOENT also covers a dangling parent link, so access.toml and
    // gating.toml are omitted there exactly like a missing file.
    if (process.platform === "win32") return;
    const previous = process.cwd();
    const root = mkdtempSync(join(tmpdir(), "meta-dangling-parent-"));
    try {
      process.chdir(root);
      symlinkSync(join(root, "missing-dir"), "config");
      const builder = Meta.builder().mob("config/mob.toml");
      (builder as any)._applyConventionDefaults();
      assert.equal(builder._config.accessConfigPath, null);
      assert.equal(builder._config.gatingConfigPath, null);
    } finally {
      process.chdir(previous);
      rmSync(root, { recursive: true, force: true });
    }
  });
});

describe("MetaBuilder method chaining", () => {
  it("supports full chain", () => {
    const builder = Meta.builder()
      .mob("mob.toml")
      .gateway("/bin/gw")
      .consoleConfig("console.toml")
      .meerkatConfig("host/config.toml")
      .httpListen("0.0.0.0:8080")
      .httpPublicBaseUrl("https://mob.example.com")
      .allowRemote()
      .consoleAuthRequired(false)
      .consoleReadOnly(true)
      .consoleFetchTimeoutMs(120_000)
      .demoLlm()
      .memberCommsTcp()
      .gating("gating.toml")
      .routing("routing.toml")
      .workgraph(false)
      .auth({ provider: "jwt" })
      .maxSessions(320)
      .gatewayTimeoutMs(300_000)
      .modules([{ id: "a" }]);

    assert.equal(builder._config.mobConfigPath, "mob.toml");
    assert.equal(builder._config.gatewayBin, "/bin/gw");
    assert.equal(builder._config.consoleConfigPath, "console.toml");
    assert.equal(builder._config.meerkatConfigPath, "host/config.toml");
    assert.equal(builder._config.httpListen, "0.0.0.0:8080");
    assert.equal(builder._config.httpPublicBaseUrl, "https://mob.example.com");
    assert.equal(builder._config.allowRemote, true);
    assert.equal(builder._config.consoleRequireAppAuth, false);
    assert.equal(builder._config.consoleReadOnly, true);
    assert.equal(builder._config.consoleFetchTimeoutMs, 120_000);
    assert.equal(builder._config.demoLlm, true);
    assert.equal(builder._config.memberCommsAddress, "127.0.0.1:0");
    assert.equal(builder._config.gatingConfigPath, "gating.toml");
    assert.equal(builder._config.workgraphEnabled, false);
    assert.equal(builder._config.routingConfigPath, "routing.toml");
    assert.deepEqual(builder._config.authConfig, { provider: "jwt" });
    assert.equal(builder._config.maxSessions, 320);
    assert.equal(builder._config.gatewayTimeoutMs, 300_000);
    assert.deepEqual(builder._config.modules, [{ id: "a" }]);
  });
});

describe("MetaBuilder removed boot knobs", () => {
  // discovery() / preSpawn() were stored and never transmitted (dead since
  // v0.2). The method must be gone so a caller fails loudly at the call site
  // instead of being accepted and dropped, and the config has no slot for it.
  it("discovery() and preSpawn() are not builder methods", () => {
    const builder = Meta.builder() as unknown as Record<string, unknown>;
    assert.equal(typeof builder.discovery, "undefined");
    assert.equal(typeof builder.preSpawn, "undefined");
  });

  it("the config carries no dead callback slots", () => {
    const cfg = Meta.builder()._config as unknown as Record<string, unknown>;
    assert.equal("discoveryCallback" in cfg, false);
    assert.equal("preSpawnCallback" in cfg, false);
    // The live boot-membership knob is still there.
    assert.equal("rosterProvider" in cfg, true);
  });
});
