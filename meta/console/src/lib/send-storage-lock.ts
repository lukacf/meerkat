/**
 * Serialize synchronous queue storage changes across tabs, including plain HTTP.
 * `key` names both the lock and the localStorage item that `update` changes.
 */
export async function withConsoleSendStorageLock<T>(key: string, update: () => T): Promise<T> {
  if (navigator.locks) return navigator.locks.request(key, update);
  if (!globalThis.indexedDB) throw new Error("This browser cannot coordinate saved queues. Your message remains in the composer.");
  // IndexedDB read/write transactions serialize across connections and tabs.
  // The callback stays synchronous, entirely within an active transaction.
  // localStorage is not ordered by that transaction: another tab's write can
  // reach this tab after its transaction ended. The lock record keeps the
  // value the last holder left, and this holder keeps the transaction alive
  // until its own localStorage shows that value (bounded, so a writer that
  // does not record it, such as an older console tab, cannot stall the queue).
  const database = await new Promise<IDBDatabase>((resolve, reject) => {
    let abandoned = false;
    const request = indexedDB.open("meta-console-queue-locks", 1);
    request.onupgradeneeded = () => { request.result.createObjectStore("locks"); };
    request.onsuccess = () => {
      if (abandoned) request.result.close();
      else resolve(request.result);
    };
    request.onerror = () => reject(request.error ?? new Error("Queue coordination is unavailable."));
    request.onblocked = () => {
      abandoned = true;
      reject(new Error("Queue coordination is blocked by another tab."));
    };
  });
  const stored = (): string | null | undefined => {
    try { return localStorage.getItem(key); } catch { return undefined; }
  };
  try {
    return await new Promise<T>((resolve, reject) => {
      const transaction = database.transaction("locks", "readwrite");
      const store = transaction.objectStore("locks");
      let result: T;
      let failure: unknown;
      transaction.oncomplete = () => resolve(result);
      transaction.onabort = () => reject(failure ?? transaction.error ?? new Error("Queue coordination was interrupted."));
      transaction.onerror = () => { failure ??= transaction.error; };
      const request = store.get(key);
      request.onsuccess = () => {
        const record: unknown = request.result;
        const last = record && typeof record === "object" && "after" in record ? (record as { after: string | null }).after : undefined;
        const deadline = Date.now() + 250;
        const visible = () => {
          if (last === undefined || Date.now() >= deadline) return true;
          const seen = stored();
          return seen === undefined || seen === last;
        };
        const proceed = () => {
          if (!visible()) {
            // Each request round trip yields to the event loop, which applies
            // pending localStorage changes from other tabs, without releasing the lock.
            store.get(key).onsuccess = proceed;
            return;
          }
          try {
            result = update();
            const after = stored();
            if (after !== undefined) store.put({ after }, key);
          } catch (error) { failure = error; transaction.abort(); }
        };
        proceed();
      };
    });
  } finally { database.close(); }
}
