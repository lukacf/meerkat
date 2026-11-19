"""Observe a native shell job; submission and terminal output are separate facts."""
import asyncio
import base64
import json
import os
import shutil
import tempfile
from pathlib import Path

from meerkat import MeerkatClient
from meerkat.generated.types import JobsListParams, JobsResultParams

COMMAND = 'expr 17 + 25'


def receipt_id(messages):
    calls = [b for m in messages for b in m.blocks if b.block_type == 'tool_use' and b.name == 'shell']
    if len(calls) != 1 or calls[0].args.get('command') != COMMAND or calls[0].args.get('background') is not True:
        raise ValueError('Expected one background shell call for the fixed calculation')
    results = [r for m in messages for r in m.results if r.tool_use_id == calls[0].id]
    if len(results) != 1 or results[0].is_error:
        raise ValueError('Missing successful shell submission receipt')
    content = results[0].content
    body = content if isinstance(content, str) else ''.join(b['text'] for b in content if b.get('type') == 'text')
    jid = json.loads(body)['job_id']
    if not isinstance(jid, str) or not jid:
        raise ValueError('Missing job ID')
    return jid


async def run(root, config):
    user = root / 'config'
    user.mkdir()
    shutil.copyfile(config, user / 'config.toml')
    client = MeerkatClient(os.environ['RKAT_RPC'])
    await client.connect(realm_id='global', context_root=str(root), state_root=str(root / 'state'),
                         user_config_root=str(user))
    try:
        session = await client.create_session(
            f'Use the shell tool once with command {COMMAND!r}, background true and timeout_secs 10. '
            'Do not run any other command, inspect the environment, poll or retry. After the receipt, answer Submitted.',
            model='job-demo', provider='self_hosted', enable_shell=True, enable_builtins=True)
        history = await session.history()
        if history.has_more:
            raise ValueError('Incomplete origin session history')
        jid = receipt_id(history.messages)
        jobs = (await client.jobs_list(JobsListParams(session_id=session.id))).jobs
        if not any(j.job_id == jid for j in jobs):
            raise ValueError('Receipt ID was not listed for the submitting session')
        print('Submission:', jid, 'origin:', session.id, flush=True)
        for _ in range(25):
            record = await client.jobs_result(JobsResultParams(job_id=jid))
            if record.job_id != jid:
                raise ValueError('Terminal lookup returned another job')
            if record.result is not None:
                if record.phase != 'succeeded' or not isinstance(record.result, dict) or record.result.get('kind') != 'succeeded':
                    raise ValueError(f'Job did not succeed: {record.phase}')
                ref = record.result.get('result_ref')
                if not isinstance(ref, str) or not ref:
                    raise ValueError('Success without a result reference')
                blob = await client.get_blob(ref)
                if blob.blob_id != ref:
                    raise ValueError('Referenced output did not match the fetched blob')
                output = json.loads(base64.b64decode(blob.data_base64, validate=True))
                if type(output.get('exit_code')) is not int or output['exit_code'] != 0 or output.get('stdout') != '42\n':
                    raise ValueError('Shell output did not verify 17 + 25 = 42')
                print('Verified:', record.phase, ref, 'exit=0 stdout=42')
                return
            await asyncio.sleep(0.2)
        raise TimeoutError('No terminal result observed; no resubmission')
    finally:
        await client.close()


if __name__ == '__main__':
    import sys
    config = Path(sys.argv[1]) if len(sys.argv) == 2 else Path(__file__).with_name('config.toml')
    with tempfile.TemporaryDirectory(prefix='job-proof-') as tmp:
        asyncio.run(asyncio.wait_for(run(Path(tmp), config), timeout=60))
