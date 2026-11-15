// The selector of `recallInput` and `inputState` names the input by its
// input id or by its idempotency key, never by both and never by neither.
import type { DeferredSession, MeerkatClient, Session } from "../src/index.js";

declare const client: MeerkatClient;
declare const session: Session;
declare const deferred: DeferredSession;

const INPUT_ID = "01936f8b-0000-7000-8000-0000000000a1";
const KEY = "inbox:msg-1";

void session.recallInput({ inputId: INPUT_ID });
void session.recallInput({ idempotencyKey: KEY });
void deferred.recallInput({ inputId: INPUT_ID });
void deferred.recallInput({ idempotencyKey: KEY });
void client.inputState("s-1", { inputId: INPUT_ID });
void client.inputState("s-1", { idempotencyKey: KEY });
void client._recallInput("s-1", { inputId: INPUT_ID });
void client._recallInput("s-1", { idempotencyKey: KEY });

// @ts-expect-error A selector names one of the two, not both.
void session.recallInput({ inputId: INPUT_ID, idempotencyKey: KEY });
// @ts-expect-error A selector names one of the two, not both.
void deferred.recallInput({ inputId: INPUT_ID, idempotencyKey: KEY });
// @ts-expect-error A selector names one of the two, not both.
void client.inputState("s-1", { inputId: INPUT_ID, idempotencyKey: KEY });
// @ts-expect-error A selector names one of the two, not both.
void client._recallInput("s-1", { inputId: INPUT_ID, idempotencyKey: KEY });

// @ts-expect-error A selector names one of the two.
void session.recallInput({});
// @ts-expect-error A selector names one of the two.
void deferred.recallInput({});
// @ts-expect-error A selector names one of the two.
void client.inputState("s-1", {});
// @ts-expect-error A selector names one of the two.
void client._recallInput("s-1", {});

// A selector built with both keys in one variable is refused as well: the
// excess-property check does not apply to it, the `never` members do.
const both = { inputId: INPUT_ID, idempotencyKey: KEY };
// @ts-expect-error A selector names one of the two, not both.
void session.recallInput(both);
// @ts-expect-error A selector names one of the two, not both.
void client.inputState("s-1", both);
