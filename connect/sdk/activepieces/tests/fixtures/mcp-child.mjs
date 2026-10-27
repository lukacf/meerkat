import { serveMcpStdio } from '../../dist/mcp.js';
const serving = serveMcpStdio({profile:'tally.get-form',token_env:'FIXTURE_TOKEN'}, {
  environment:{FIXTURE_TOKEN:'fixture-token'},
  httpFactory:()=>({
    async send(url,init) {
      if (new Headers(init.headers).get('authorization')!=='Bearer fixture-token') throw new Error('Fixture auth mismatch');
      if (url==='https://api.tally.so/forms/ready') return new Response('{"fixture":true}');
      if (url!=='https://api.tally.so/forms/held') throw new Error('Unexpected fixture URL');
      process.stderr.write('fixture request entered\n');
      return new Response(new ReadableStream({async cancel(){await new Promise(resolve=>setTimeout(resolve,25));process.stderr.write('fixture body closed\n');}}));
    },
    async close(){process.stderr.write('fixture owner closed\n');},
  }),
});
await serving.closed;
