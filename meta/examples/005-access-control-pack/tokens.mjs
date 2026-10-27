// Shared HS256 dev-token minter for the access control pack.
//
// The example server (server.rs) trusts an HS256 issuer on a `.localhost`
// host for local development only. These tokens are NOT a production auth
// mechanism — they exist so the demo can present each persona's identity to
// the open console without standing up a real OIDC provider.
import crypto from "node:crypto";

export const ISSUER = "https://trusted.meta.localhost";
export const AUDIENCE = "meerkat-console";
const SECRET = "phase7-trusted-current-secret";
const KID = "kid-current";

function b64url(buf) {
  return Buffer.from(buf).toString("base64url");
}

/** Mint a 12-hour HS256 console token for `email`. */
export function mintToken(email) {
  const header = { alg: "HS256", typ: "JWT", kid: KID };
  const now = Math.floor(Date.now() / 1000);
  const claims = {
    iss: ISSUER,
    aud: AUDIENCE,
    iat: now,
    exp: now + 12 * 3600,
    email,
    provider: "google_oauth",
  };
  const signingInput = `${b64url(JSON.stringify(header))}.${b64url(JSON.stringify(claims))}`;
  const signature = crypto.createHmac("sha256", SECRET).update(signingInput).digest("base64url");
  return `${signingInput}.${signature}`;
}

/** The demo personas, in browse order. `email: null` is the anonymous tab. */
export const PERSONAS = [
  { port: 7301, label: "anonymous", email: null },
  { port: 7302, label: "alice (ops)", email: "alice@example.test" },
  { port: 7303, label: "bob (payments)", email: "bob@example.test" },
  { port: 7304, label: "root (admin)", email: "root@example.test" },
];
