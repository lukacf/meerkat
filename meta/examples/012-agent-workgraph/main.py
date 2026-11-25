import asyncio
import os
import secrets
import shutil
import sys
import tempfile
from pathlib import Path
from meerkat_meta import Meta

MEMBER = 'writer'
CONFIG = Path(__file__).with_name('mob.toml').resolve()


def verify(item, events, output, marker):
    history = sorted((e for e in events if e.item_id == item.id), key=lambda e: e.seq)
    kinds = [e.kind for e in history]
    expected = ['created', 'claimed', 'evidence_added', 'closed']
    if [k for k in kinds if k in expected] != expected:
        raise ValueError(f'Unexpected lifecycle: {kinds}')
    claimed = next(e.payload['item'] for e in history if e.kind == 'claimed')
    if claimed['claim']['owner']['key'] != {'kind': 'agent', 'id': MEMBER}:
        raise ValueError('Claim was not owned by the writer')
    if history[-1].payload['item']['status'] != 'completed' or item.status != 'completed':
        raise ValueError('No terminal completion')
    if not any(ref.get('kind') == 'file' and ref.get('id') == str(output) for ref in item.evidence_refs):
        raise ValueError('Missing reference to the expected file')
    if output.is_symlink() or not output.is_file() or output.stat().st_size > 1024:
        raise ValueError('Expected a small regular file')
    if output.read_text().strip() != marker:
        raise ValueError('File does not contain the marker for this run')
    return kinds


async def run(gateway, root):
    marker = 'handoff-' + secrets.token_hex(6)
    output = root / 'handoff.txt'
    rt = await (Meta.builder().gateway(gateway).mob(str(CONFIG))
                .persistent_state(str(root / 'state')).workgraph(True).init_deadline(60).build())
    try:
        mob = rt.mob_handle()
        await mob.ensure_member(MEMBER, 'writer')
        await mob.send(MEMBER, f'Prepare a handoff note. Create one native WorkGraph item titled {marker}. '
                       'Use workgraph_ready to find it, then claim it as owner key kind agent, id writer. '
                       'Use the current revision for each mutation. After claiming it, use shell to '
                       f'write exactly {marker} on one line in {output}, and read it back. Attach a '
                       f'workgraph_add_evidence reference with kind file and id {output}. Only then close '
                       'the item with status completed. Report failures rather than claiming success.')
        for _ in range(120):
            items = (await mob.workgraph_list(include_terminal=True)).items
            matches = [item for item in items if item.title == marker]
            if len(matches) > 1:
                raise ValueError('More than one item for the assignment')
            if matches and matches[0].terminal_at:
                item = matches[0]
                kinds = verify(item, await mob.workgraph_events(), output, marker)
                status = await mob.member_status(MEMBER)
                if status.progress and status.progress.run_state == 'idle' and not status.progress.in_flight_work:
                    print(item.id, '->'.join(kinds), 'verified', output.name, marker)
                    return
            await asyncio.sleep(1)
        raise TimeoutError('No verified result; Python did not mutate the graph')
    finally:
        await rt.shutdown()


def main():
    gateway = os.environ.get('META_GATEWAY_BIN')
    if not gateway:
        print('Set META_GATEWAY_BIN to the matching rpc_gateway.', file=sys.stderr)
        return 2
    root = Path(tempfile.mkdtemp(prefix='work-note-')).resolve()
    try:
        asyncio.run(asyncio.wait_for(run(gateway, root), timeout=180))
    except Exception:
        print(f'Failed run retained for local inspection: {root}', file=sys.stderr)
        raise
    shutil.rmtree(root)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
