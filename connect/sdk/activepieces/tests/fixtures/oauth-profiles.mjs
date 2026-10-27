const auth = accessToken => ({ accessToken });
const headers = token => ({ authorization: `Bearer ${token}` });
export const oauthProfiles = [
  { profile: 'google-contacts.search-contact', auth, headers,
    args: { query: 'Anna + K', readMask: ['names', 'emailAddresses'], pageSize: 7 },
    url: 'https://people.googleapis.com/v1/people:searchContacts?query=Anna+%2B+K&readMask=names%2CemailAddresses&pageSize=7',
    body: { results: [{ person: { resourceName: 'people/example', names: [{ displayName: 'Anna' }] } }], unknown: null },
    wrap: body => body },
  { profile: 'figma.get-comments', auth, headers, args: { file_key: 'File_123' },
    url: 'https://api.figma.com/v1/files/File_123/comments',
    body: { comments: [{ id: '1', message: 'Keep + iterate', resolved_at: null }], unknown: false },
    wrap: body => ({ success: true, response_body: body }) },
  { profile: 'acuity.find-appointments', auth, headers,
    args: { firstName: 'Anna', phone: '+46701234567', minDate: '2026-10-03', maxDate: '2026-10-10', calendarID: 12, appointmentTypeID: 34, status: 'all', maxResults: 9, direction: 'ASC' },
    url: 'https://acuityscheduling.com/api/v1/appointments?firstName=Anna&phone=%2B46701234567&minDate=2026-10-03&maxDate=2026-10-10&calendarID=12&appointmentTypeID=34&showall=true&max=9&direction=ASC',
    body: [{ id: 100, firstName: 'Anna', paid: false, unknown: null }],
    wrap: body => ({ found: body.length > 0, data: body }) },
].map(d => ({ ...d, result: d.wrap(d.body), config: {} }));
