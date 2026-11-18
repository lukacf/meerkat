"""Exchange a signed peer message between agents hosted by two gateways."""
import asyncio
import os
import secrets
from contextlib import AsyncExitStack
from pathlib import Path

from meerkat_meta import Meta

HERE = Path(__file__).resolve().parent


async def exchange(stack: AsyncExitStack) -> None:
    async def start(config):
        rt = await (Meta.builder().gateway(os.environ['META_GATEWAY_BIN'])
                    .mob(str(HERE / config)).member_comms_tcp('127.0.0.1:0')
                    .init_deadline(60).build())
        stack.push_async_callback(rt.shutdown)
        return rt.mob_handle()

    desk, records = await start('newsroom.toml'), await start('archive.toml')
    ticket = 'ticket-' + secrets.token_hex(6)
    shelf = 'shelf-' + secrets.token_hex(6)
    await desk.ensure_member('editor', 'worker', additional_instructions=[
        'When asked for an archive lookup, call peers and send the ticket to the archivist '
        'with send_message. Do not guess a shelf code. When the peer reply arrives, '
        'report the ticket and shelf code in your answer; do not message again.'
    ])
    await records.ensure_member('archivist', 'worker', additional_instructions=[
        f'Your shelf code is {shelf}. When a peer asks for a lookup, use reply_to_peer '
        'to return its exact ticket and your shelf code. In your final answer include both.'
    ])
    for handle, member in ((desk, 'editor'), (records, 'archivist')):
        info = await handle.peer_info(member)
        if not info['dialable_address'].startswith('tcp://127.0.0.1:') or not info['pubkey_b64']:
            raise RuntimeError(f'{member} has no signed loopback endpoint')
    await desk.wire_cross_mob('editor', 'archivist', records)
    # The host gives work to just one member. It never delivers the ticket to
    # the archivist or supplies the archivist’s shelf code to the editor.
    await desk.send('editor', f'Ask the archivist for the shelf code for {ticket}.')
    quiet = 0
    for _ in range(120):
        editor, archivist = await asyncio.gather(
            desk.member_status('editor'), records.member_status('archivist'))
        confirmed = all(
            ticket in (m.output_preview or '') and shelf in (m.output_preview or '')
            and m.progress is not None and m.progress.run_state == 'idle'
            and m.progress.in_flight_work == 0
            for m in (editor, archivist)
        )
        quiet = quiet + 1 if confirmed else 0
        if quiet == 2:
            print(f'Both recipients reported {ticket} and {shelf}.')
            return
        await asyncio.sleep(0.5)
    raise RuntimeError('The round trip was not confirmed by both recipients.')


async def main():
    async with AsyncExitStack() as stack:
        await asyncio.wait_for(exchange(stack), timeout=180)


if __name__ == '__main__':
    asyncio.run(main())
