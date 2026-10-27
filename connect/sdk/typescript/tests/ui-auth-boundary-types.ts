import type { UserActionView, ControlRequest } from '@rkat/toolkit/ui';
const action: UserActionView = {
  id: 'safe', integration: 'Example', kind: 'browser_authorization',
  state: 'action_required', instructions: 'Review the requested account.',
  authorizationOrigin: 'https://auth.example.test', requestedAccountLabel: 'Work', controls: [],
};
const legacy: UserActionView = { ...action,
  // @ts-expect-error native authorize URLs do not belong in this shared projection
  authorizationUrl: 'https://auth.example.test/authorize?state=synthetic',
};
const request: ControlRequest = {
  scope: 'person', viewRevision: 1,
  control: { offer: 'presentation', kind: 'open_authorization',
    target: { owner: 'host', reference: 'opaque', revision: 'current' } },
};
const echo: ControlRequest = { ...request,
  // @ts-expect-error dispatch carries only the admitted presentation reference
  displayedAuthorizationUrl: 'https://auth.example.test/authorize?state=synthetic',
};
void legacy; void echo;
