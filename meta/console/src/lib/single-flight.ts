/// At most one run per key. A request made while that key is running is not
/// started concurrently: it marks the run dirty, the latest task runs once
/// more after the current one settles, and every caller in between awaits
/// that trailing run, so it still observes state from after its request.
///
/// Refreshes are triggered by stream events; without this, a server slower
/// than the event rate stacked concurrent requests until the browser's
/// per-origin connection limit starved every other console request.
export type SingleFlight = (key: string, task: () => Promise<void>) => Promise<void>;

export function createSingleFlight(): SingleFlight {
  const flights = new Map<string, { again: boolean; task: () => Promise<void>; done: Promise<void> }>();
  return (key, task) => {
    const current = flights.get(key);
    if (current) {
      current.again = true;
      current.task = task;
      return current.done;
    }
    const flight = { again: false, task, done: Promise.resolve() };
    flight.done = (async () => {
      try {
        let failure: { error: unknown } | null;
        do {
          flight.again = false;
          failure = null;
          try {
            await flight.task();
          } catch (error) {
            failure = { error };
          }
        } while (flight.again);
        if (failure) throw failure.error;
      } finally {
        flights.delete(key);
      }
    })();
    flights.set(key, flight);
    return flight.done;
  };
}
