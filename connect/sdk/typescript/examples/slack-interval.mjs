/** Explicit host-selected creation-time retrieval. Importing performs no I/O. */
export async function readSelectedInterval(selectedSlack, {
  selectedChannel, start, end, permittedRootOldest, maxRequests, maxMessages, signal,
}) {
  // The existing host authority permits this channel AND wider discovery range.
  // No identity resolver, credential lookup, grant or send is performed here.
  const result = await selectedSlack.readInterval(selectedChannel, {
    start, end, rootOldest: permittedRootOldest, maxRequests, maxMessages,
    ...(signal === undefined ? {} : { signal }),
  });
  // Carry coverage_gaps and pending/stopped chains into any summary. Finished
  // means the queue ended, not that hidden roots or concurrent changes vanished.
  return result;
}
