export const KEY = 'fixture-service-secret+/%?&';
export const serviceProfiles = [
  {
    profile: 'kit.get-subscriber', action: 'kit_get_subscriber', auth: key => ({ apiSecret: key }), config: {},
    args: { subscriber_id: '1234567890123456789' },
    url: key => `https://api.convertkit.com/v3/subscribers/1234567890123456789?${new URLSearchParams({ api_secret: key })}`,
    headers: () => ({ accept: 'application/json' }), envelope: 'subscriber',
    value: { id: 123, email_address: 'fixture@example.invalid', first_name: 'Fixture', state: 'active', fields: { visits: 0, confirmed: false }, optional: null },
  },
  {
    profile: 'mailer-lite.get-campaign', action: 'get_campaign', auth: key => ({ apiKey: key }), config: {},
    args: { campaign_id: 'campaign_123' }, url: () => 'https://connect.mailerlite.com/api/campaigns/campaign_123',
    headers: key => ({ accept: 'application/json', authorization: `Bearer ${key}` }), envelope: 'data',
    value: { id: 'campaign_123', name: 'Product update', status: 'sent', stats: { opens_count: 0 }, archived: false, optional: null },
  },
  {
    profile: 'calendly.get-scheduled-event', action: 'get_scheduled_event', auth: key => ({ personalToken: key }), config: {},
    args: { scheduledEvent: 'https://api.calendly.com/scheduled_events/event_123' }, url: () => 'https://api.calendly.com/scheduled_events/event_123',
    headers: key => ({ accept: 'application/json', authorization: `Bearer ${key}` }), envelope: 'resource',
    value: { uri: 'https://api.calendly.com/scheduled_events/event_123', name: 'Intro call', status: 'active', start_time: '2026-10-03T12:00:00Z', invitees_counter: { active: 0 }, canceled: false, optional: null },
  },
].map(definition => ({ ...definition, body: { [definition.envelope]: definition.value }, result: definition.value }));
