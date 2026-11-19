"""An agent schedules a second turn; Python never sends that turn."""
import asyncio
import os
import secrets
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

from meerkat import MeerkatClient


def text(content):
    if isinstance(content, str):
        return content
    return ''.join(b['text'] for b in content or [] if b.get('type') == 'text')


def assistant_text(row):
    return ''.join(b.text or '' for b in row.blocks if b.block_type == 'text')


async def run(root):
    client = MeerkatClient(rkat_path=os.environ['RKAT_RPC'])
    await client.connect(isolated=True, state_root=str(root / 'state'),
                         context_root=str(root), user_config_root=str(root / 'config'))
    try:
        left, right = secrets.token_hex(5), secrets.token_hex(5)
        expected = left + right
        follow = f'Join {left} and {right} without a separator. Reply only with the joined string.'
        due = (datetime.now(timezone.utc) + timedelta(seconds=45)).isoformat(timespec='seconds')
        session = await client.create_session(
            f'Call meerkat_schedule_create exactly once: schedule a one-time prompt at {due} '
            f'for your current_session. Use this exact action prompt: {follow}\n'
            'Use skip_if_running, skip on misfire, and mark_misfired if the target is missing. '
            'After creation reply only Scheduled. Do not do the scheduled task now.',
            model='claude-haiku-5-5', enable_schedule=True, enable_shell=False,
            enable_builtins=False, keep_alive=True)
        first = (await session.history()).messages
        schedules = (await client.list_schedules())['schedules']
        if len(schedules) != 1 or expected in session.text:
            raise ValueError(f'Expected one agent-created schedule and no early answer; schedules={len(schedules)}')
        schedule = schedules[0]
        sid = schedule['schedule_id']
        calls = {b.id for row in first for b in row.blocks if b.name == 'meerkat_schedule_create'}
        ok = [r for row in first for r in row.results if r.tool_use_id in calls and not r.is_error and sid in text(r.content)]
        if len(calls) != 1 or len(ok) != 1:
            raise ValueError('No unique successful agent create result matching the schedule ID')
        print('Agent create result:', sid, 'session:', session.id, flush=True)
        for _ in range(90):
            rows = (await client.list_schedule_occurrences(sid, include_terminal=True))['occurrences']
            if len(rows) == 1 and rows[0]['phase'] == 'completed':
                occ = rows[0]
                target = occ['target_snapshot']
                if (occ['schedule_id'] != sid or target.get('session_id') != session.id or
                        target.get('action', {}).get('prompt') != follow or
                        schedule['trigger']['type'] != 'once'):
                    raise ValueError('Completed occurrence does not match the expected one-shot target/prompt')
                page = await session.history()
                delivered = [i for i, m in enumerate(page.messages) if m.role == 'user' and text(m.content) == follow]
                if len(delivered) != 1 or page.has_more:
                    raise ValueError(f'Cannot correlate prompt: {len(delivered)} matches, has_more={page.has_more}, ' 
                                     f'roles={[m.role for m in page.messages]}')
                turn = page.messages[delivered[0]+1:]
                if any(m.role == 'user' for m in turn):
                    raise ValueError('Another user turn intervened')
                answers = [m for m in turn if assistant_text(m).strip() == expected]
                if len(answers) != 1 or not answers[0].assistant_message_id:
                    raise ValueError('No unique committed assistant answer for the scheduled turn')
                print('Occurrence:', occ['occurrence_id'], 'assistant message:', answers[0].assistant_message_id,
                      'verified output:', expected)
                return
            if any(r['phase'] in {'misfired','delivery_failed','skipped','superseded'} for r in rows):
                raise ValueError(f'No completed delivery: {rows[0]["phase"]}')
            await asyncio.sleep(1)
        raise TimeoutError('Timed out observing scheduled delivery; no retry')
    finally:
        await client.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='one-shot-') as directory:
        asyncio.run(asyncio.wait_for(run(Path(directory)), timeout=150))
