// Persona auth proxies for the access control pack.
//
// Each port forwards to the example console server, injecting a minted bearer
// token for one persona so you can open one browser tab per identity:
//
//   7301 -> anonymous            7303 -> bob@example.test
//   7302 -> alice@example.test   7304 -> root@example.test
//
// Usage:
//   node persona-proxy.mjs                       # target http://127.0.0.1:7300
//   ACCESS_CONTROL_TARGET=127.0.0.1:7300 node persona-proxy.mjs
import http from "node:http";
import { mintToken, PERSONAS } from "./tokens.mjs";

const target = process.env.ACCESS_CONTROL_TARGET || "127.0.0.1:7300";
const [targetHost, targetPort] = target.split(":");

for (const { port, label, email } of PERSONAS) {
  const server = http.createServer((req, res) => {
    const headers = { ...req.headers, host: `${targetHost}:${targetPort}` };
    if (email) {
      headers.authorization = `Bearer ${mintToken(email)}`;
    } else {
      delete headers.authorization;
    }
    const upstream = http.request(
      { host: targetHost, port: Number(targetPort), method: req.method, path: req.url, headers },
      (upstreamRes) => {
        res.writeHead(upstreamRes.statusCode, upstreamRes.headers);
        upstreamRes.pipe(res);
      },
    );
    upstream.on("error", (err) => {
      res.writeHead(502, { "content-type": "text/plain" });
      res.end(`proxy error: ${err.message}`);
    });
    req.pipe(upstream);
  });
  server.listen(port, "127.0.0.1", () => {
    console.log(`persona ${label} -> http://127.0.0.1:${port}/console`);
  });
}
