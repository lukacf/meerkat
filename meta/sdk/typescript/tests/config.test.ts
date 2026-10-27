import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { auth, memory } from "../dist/index.js";

// ---------------------------------------------------------------------------
// auth.google()
// ---------------------------------------------------------------------------

describe("auth.google", () => {
  it("creates a GoogleAuthConfig with defaults", () => {
    const config = auth.google("my-client-id");
    assert.equal(config.clientId, "my-client-id");
    assert.equal(
      config.discoveryUrl,
      "https://accounts.google.com/.well-known/openid-configuration",
    );
    assert.equal(config.audience, null);
    assert.equal(config.leewaySeconds, 60);
  });

  it("allows custom discoveryUrl", () => {
    const config = auth.google("cid", {
      discoveryUrl: "https://custom/.well-known/openid-configuration",
    });
    assert.equal(
      config.discoveryUrl,
      "https://custom/.well-known/openid-configuration",
    );
  });

  it("allows custom audience", () => {
    const config = auth.google("cid", { audience: "my-audience" });
    assert.equal(config.audience, "my-audience");
  });

  it("allows custom leewaySeconds", () => {
    const config = auth.google("cid", { leewaySeconds: 120 });
    assert.equal(config.leewaySeconds, 120);
  });

  it("allows all options combined", () => {
    const config = auth.google("cid", {
      discoveryUrl: "https://example.com",
      audience: "aud",
      leewaySeconds: 30,
    });
    assert.equal(config.clientId, "cid");
    assert.equal(config.discoveryUrl, "https://example.com");
    assert.equal(config.audience, "aud");
    assert.equal(config.leewaySeconds, 30);
  });
});

// ---------------------------------------------------------------------------
// auth.googleAuthConfigToDict()
// ---------------------------------------------------------------------------

describe("auth.googleAuthConfigToDict", () => {
  it("produces the correct wire dict", () => {
    const config = auth.google("cid-1");
    const dict = auth.googleAuthConfigToDict(config);
    assert.equal(dict.provider, "google");
    assert.equal(dict.client_id, "cid-1");
    assert.equal(
      dict.discovery_url,
      "https://accounts.google.com/.well-known/openid-configuration",
    );
    // When audience is null, it falls back to clientId
    assert.equal(dict.audience, "cid-1");
    assert.equal(dict.leeway_seconds, 60);
  });

  it("uses audience when explicitly set", () => {
    const config = auth.google("cid-2", { audience: "explicit-aud" });
    const dict = auth.googleAuthConfigToDict(config);
    assert.equal(dict.audience, "explicit-aud");
  });
});

// ---------------------------------------------------------------------------
// auth.jwt()
// ---------------------------------------------------------------------------

describe("auth.jwt", () => {
  it("creates a JwtAuthConfig with defaults", () => {
    const config = auth.jwt("secret-key");
    assert.equal(config.sharedSecret, "secret-key");
    assert.equal(config.issuer, null);
    assert.equal(config.audience, null);
    assert.equal(config.leewaySeconds, 60);
  });

  it("allows custom issuer", () => {
    const config = auth.jwt("secret", { issuer: "my-issuer" });
    assert.equal(config.issuer, "my-issuer");
  });

  it("allows custom audience", () => {
    const config = auth.jwt("secret", { audience: "my-aud" });
    assert.equal(config.audience, "my-aud");
  });

  it("allows custom leewaySeconds", () => {
    const config = auth.jwt("secret", { leewaySeconds: 10 });
    assert.equal(config.leewaySeconds, 10);
  });
});

// ---------------------------------------------------------------------------
// auth.jwtAuthConfigToDict()
// ---------------------------------------------------------------------------

describe("auth.jwtAuthConfigToDict", () => {
  it("produces the correct wire dict", () => {
    const config = auth.jwt("s3cr3t");
    const dict = auth.jwtAuthConfigToDict(config);
    assert.equal(dict.provider, "jwt");
    assert.equal(dict.shared_secret, "s3cr3t");
    assert.equal(dict.issuer, null);
    assert.equal(dict.audience, null);
    assert.equal(dict.leeway_seconds, 60);
  });

  it("includes issuer and audience when set", () => {
    const config = auth.jwt("key", { issuer: "iss", audience: "aud" });
    const dict = auth.jwtAuthConfigToDict(config);
    assert.equal(dict.issuer, "iss");
    assert.equal(dict.audience, "aud");
  });
});

// ---------------------------------------------------------------------------
// memory.localJson()
// ---------------------------------------------------------------------------

describe("memory.localJson", () => {
  it("produces minimal dict without a health endpoint", () => {
    const config = memory.localJson();
    assert.equal(config.healthCheckEndpoint, null);
    assert.deepEqual(config.toDict(), { backend: "local_json" });
  });

  it("includes health_check_endpoint when provided", () => {
    const config = memory.localJson({
      healthCheckEndpoint: "http://localhost:3000",
    });
    assert.equal(config.healthCheckEndpoint, "http://localhost:3000");
    assert.deepEqual(config.toDict(), {
      backend: "local_json",
      health_check_endpoint: "http://localhost:3000",
    });
  });
});

describe("memory module", () => {
  it("exports localJson as the only backend helper", () => {
    // The predecessor's backend helpers are removed with no alias; the
    // gateway refuses that backend value at init.
    assert.deepEqual(Object.keys(memory).sort(), ["localJson"]);
  });
});
