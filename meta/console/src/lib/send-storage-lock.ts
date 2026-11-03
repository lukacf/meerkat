/** Serialize synchronous queue storage changes across tabs, including plain HTTP. */
export async function withConsoleSendStorageLock<T>(key: string, update: () => T): Promise<T> {
  if (navigator.locks) return navigator.locks.request(key, update);
  if (!globalThis.indexedDB) throw new Error("This browser cannot coordinate saved queues. Your message remains in the composer.");
  // IndexedDB read/write transactions serialize across connections and tabs.
  // The callback stays synchronous, entirely within an active transaction.
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
  try {
    return await new Promise<T>((resolve, reject) => {
      const transaction = database.transaction("locks", "readwrite");
      let result: T;
      let failure: unknown;
      transaction.oncomplete = () => resolve(result);
      transaction.onabort = () => reject(failure ?? transaction.error ?? new Error("Queue coordination was interrupted."));
      transaction.onerror = () => { failure ??= transaction.error; };
      const request = transaction.objectStore("locks").get(key);
      request.onsuccess = () => {
        try { result = update(); }
        catch (error) { failure = error; transaction.abort(); }
      };
    });
  } finally { database.close(); }
}
