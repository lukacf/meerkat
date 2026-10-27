"""An isolated MobKit owner for fresh visual critic helpers."""
from __future__ import annotations

import asyncio
from datetime import datetime, timezone
import os
from pathlib import Path
import re
import uuid

from .templates.storage_setup import write_private
from .visualization_timeline import read_record

from .planner import gateway_path, planner_capability
from .native_models import model_registry_lines
from .visualization_review import REVIEW_PROFILE


def visual_review_mob_toml(provider="openai", model="gpt-6.1-sol", *,
                           model_configuration=None, guide_runtime=None):
    from .guide_runtime import model_profile_lines
    lines = ['[mob]', 'id = "launch-visual-review"',
        *model_registry_lines([(provider, model)], guide_runtime=guide_runtime),
        f'[profiles.{REVIEW_PROFILE}]', *model_profile_lines(provider, model, model_configuration)]
    if provider == "openai":
        override = ', supports_reasoning_override = true' if model == "gpt-6.1-sol" else ''
        lines.append('provider_params = { provider_tag = { provider = "open_ai", reasoning_effort = "low"' + override + ' } }')
    lines.extend(['external_addressable = false', 'runtime_mode = "turn_driven"',
        # This host-spawned helper has no roster, orchestrator, role wiring,
        # or link to the guide mob. Native members require comms.
        f'[profiles.{REVIEW_PROFILE}.tools]', 'builtins = false', 'comms = true', 'mob = false', 'memory = false'])
    return '\n'.join(lines)


class VisualReviewRuntime:
    def __init__(self, root: Path):
        self.root = Path(root)
        self.owner_id = uuid.uuid4().hex
        self.reviews = None
        self.runtime = None
        self.native_process = None
        self.termination = None
        self.lock = asyncio.Lock()
        self.lock_context = None
        self.startup_cleanup = []
        self.state = 'stopped'

    def capability(self):
        configured = planner_capability()
        available = bool(configured['available'] and self.state in {'stopped', 'starting', 'ready'})
        return {'available': available, 'status': self.state,
                'detail': None if available else 'The configured visual critic runtime is unavailable.'}

    async def get_runtime(self):
        async with self.lock:
            if self.state in {'failed', 'closing', 'closed'}:
                raise ValueError('The visual critic runtime is unavailable.')
            if self.runtime is not None:
                return self.runtime
            if self.state == 'failed' or not self.capability()['available'] or self.reviews is None:
                raise ValueError('The visual critic runtime is unavailable.')
            from meerkat_meta import Meta
            from .guide_runtime import selected_runtime, configure_builder, build_with_cleanup
            from .templates.storage_setup import project_lock
            selected = selected_runtime()
            model_configuration = selected.model_configuration
            provider = planner_capability()["provider"]
            from .architect import SETUP_MODELS
            model = (model_configuration["model"] if model_configuration is not None
                     else SETUP_MODELS[provider])

            self.state = 'starting'
            self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
            owner = self

            class Roster:
                async def roster(self, context):
                    return []

            class Sessions:
                async def build_agent(self, draft):
                    if draft.profile_name != REVIEW_PROFILE:
                        raise ValueError('This runtime owns only independent visual critic helpers.')
                    owner.reviews.register_tools(draft)

            mob = visual_review_mob_toml(provider, model,
                model_configuration=model_configuration, guide_runtime=selected)
            acquired = False
            try:
                self.lock_context = project_lock(self.root)
                self.lock_context.__enter__()
                acquired = True
                builder = (Meta.builder().mob_inline(mob)
                    .persistent_state(str(self.root / 'generations' / self.owner_id / '.mobkit/state')).roster(Roster()).session_service(Sessions())
                    .console_auth_required(False).http_listen('127.0.0.1:0'))
                self.runtime = await build_with_cleanup(configure_builder(builder, selected), self.startup_cleanup)
                self.native_process = self.runtime._transport._process
                if self.native_process is None or self.native_process.poll() is not None:
                    raise RuntimeError('The visual critic has no positively observed native owner.')
                self.state = 'ready'
                return self.runtime
            except BaseException:
                self.state = 'failed'
                if acquired and self.runtime is None and not self.startup_cleanup:
                    self.lock_context.__exit__(None, None, None)
                    self.lock_context = None
                elif not acquired:
                    self.lock_context = None
                raise

    def runtime_owner_id(self, runtime):
        if self.state != 'ready' or runtime is not self.runtime:
            raise ValueError('The review runtime does not match its owned native generation.')
        return self.owner_id

    @staticmethod
    def _checked_termination(value, name):
        keys = {'version', 'owner_id', 'pid', 'exit_code', 'stopped_at', 'generation'}
        if (not isinstance(value, dict) or set(value) != keys
                or value['version'] != 'visual-review-runtime-termination-v1'
                or not isinstance(value['owner_id'], str) or not re.fullmatch('[0-9a-f]{32}', value['owner_id'])
                or name != 'terminated-' + value['owner_id'] + '.json'
                or type(value['pid']) is not int or value['pid'] <= 0
                or type(value['exit_code']) is not int
                or value['generation'] != 'generations/' + value['owner_id'] + '/.mobkit/state'
                or not isinstance(value['stopped_at'], str)):
            raise ValueError('The critic termination proof does not match its exact generation.')
        datetime.fromisoformat(value['stopped_at'])
        return value

    async def reconcile_terminated(self):
        # These are private host observations, not HTTP/model-supplied proofs.
        try:
            paths = sorted(self.root.glob('terminated-*.json'))
            if len(paths) > 10_000:
                raise ValueError('The critic owner history exceeds its bound.')
            for path in paths:
                proof = self._checked_termination(read_record(path, max_bytes=2000), path.name)
                await self.reviews.confirm_runtime_terminated(proof['owner_id'])
        except (OSError, ValueError, KeyError, TypeError):
            self.state = 'failed'
            return False
        return True

    async def shutdown(self):
        async with self.lock:
            if self.runtime is not None:
                self.state = 'closing'
                try:
                    await self.runtime.shutdown()
                    if self.native_process is None or self.native_process.poll() is None:
                        raise RuntimeError('The visual critic native owner has not positively terminated.')
                    if self.termination is None:
                        self.termination = {'version': 'visual-review-runtime-termination-v1',
                            'owner_id': self.owner_id, 'pid': self.native_process.pid,
                            'exit_code': self.native_process.returncode,
                            'stopped_at': datetime.now(timezone.utc).isoformat(),
                            'generation': 'generations/' + self.owner_id + '/.mobkit/state'}
                    path = self.root / ('terminated-' + self.owner_id + '.json')
                    self._checked_termination(self.termination, path.name)
                    write_private(path, self.termination)
                    descriptor = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY)
                    try:
                        os.fsync(descriptor)
                    finally:
                        os.close(descriptor)
                    await self.reviews.confirm_runtime_terminated(self.owner_id)
                except BaseException:
                    self.state = 'failed'
                    raise
                self.runtime = None
            for cleanup in tuple(self.startup_cleanup):
                await cleanup.shutdown()
                self.startup_cleanup.remove(cleanup)
            if self.lock_context is not None:
                self.lock_context.__exit__(None, None, None)
                self.lock_context = None
            self.state = 'closed'
