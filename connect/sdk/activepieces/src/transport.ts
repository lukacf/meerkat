import { EffectRequestState, isWriteProfile, type EffectPlan } from './effect-profiles.js';
import {SdkRequestState,type SdkPlan} from './sdk-profiles.js';
import { ReadRequestState, type ReadPlan } from './read-profiles.js';
import { ActivepiecesError, type FailureKind } from "./errors.js";
import { record, type Selection } from "./profiles.js";
import type { DonorResponse } from "./donor-factory.js";
import { contentHeaders, contentBodyFailure } from "./content.js";

/** One call owns its signal, request and borrowed response until cleanup finishes. */
export class Invocation {
  entered = false;
  dispatched = false;
  fault: ActivepiecesError | undefined;
  private readonly controller = new AbortController();
  private readonly timer: ReturnType<typeof setTimeout>;
  private readonly deadline: number;
  private abortKind: "cancelled" | "deadline_exceeded" | undefined;
  private readonly pending = new Set<Promise<DonorResponse>>();
  private readonly readState: ReadRequestState | EffectRequestState | SdkRequestState | undefined;
  private receivedBytes = 0;
  private readonly externalAbort = () => this.abort("cancelled");

  constructor(private readonly selection: Selection, private readonly pathname: string, private readonly external?: AbortSignal, private readonly selectedQuery?: Readonly<Record<string, string>>, private readonly inlineQuery?: string, readPlan?: ReadPlan | EffectPlan | SdkPlan) {
    this.readState = !readPlan ? undefined : "kind" in readPlan ? readPlan.kind === "effect" ? new EffectRequestState(selection, pathname, readPlan) : new SdkRequestState(selection,pathname,selectedQuery,readPlan) : new ReadRequestState(selection,pathname,selectedQuery,inlineQuery,readPlan);
    if (external?.aborted) throw new ActivepiecesError("cancelled", false, false, undefined, isWriteProfile(selection.profile) ? "write" : "read");
    this.deadline = performance.now() + selection.timeoutMs;
    external?.addEventListener("abort", this.externalAbort, { once: true });
    this.timer = setTimeout(() => this.abort("deadline_exceeded"), selection.timeoutMs);
  }
  s3Page(token:string|null):void { if (!(this.readState instanceof SdkRequestState)) throw this.fail("invalid_response"); this.readState.s3Page(token); }
  projectResult(value:unknown):unknown { return this.readState instanceof SdkRequestState || this.readState instanceof EffectRequestState ? this.readState.projectResult(value) : value; }
  private abort(kind: "cancelled" | "deadline_exceeded"): void {
    if (this.abortKind) return;
    this.abortKind = kind;
    this.controller.abort();
  }
  fail(kind: FailureKind, status?: number): ActivepiecesError {
    this.fault ??= new ActivepiecesError(this.abortKind ?? kind, this.entered, this.dispatched, status, isWriteProfile(this.selection.profile) ? "write" : "read");
    return this.fault;
  }
  check(): void {
    // Synchronous donor construction or JSON parsing can outlast a timer tick.
    if (performance.now() >= this.deadline) this.abort("deadline_exceeded");
    if (this.controller.signal.aborted) throw this.fail(this.abortKind ?? "cancelled");
  }
  async finish(): Promise<void> {
    // An SDK timeout may reject its waiter before its fetch settles. The owned
    // request and body still retire before this invocation can be observed done.
    if (this.pending.size) this.controller.abort();
    await Promise.allSettled([...this.pending]);
    clearTimeout(this.timer);
    this.external?.removeEventListener("abort", this.externalAbort);
  }


  private prepare(value: unknown): { url: string; headers: Headers; method?: "GET" | "POST"; body?: string } {
    if (this.readState) {
      try {
        const prepared = this.readState.prepare(value);
        // The wider mechanical HTTP ceiling serves only the new explicit
        // effect plans. Existing read/SDK plans retain their 32 KiB body cap.
        if (!(this.readState instanceof EffectRequestState) && prepared.body !== undefined &&
          Buffer.byteLength(prepared.body) > 32768) throw 0;
        return prepared;
      } catch { throw this.fail("upstream_error"); }
    }
    const invalid = () => { throw this.fail("upstream_error"); };
    if (!record(value) || Object.keys(value).some(key => !["method", "url", "authentication", "headers", "queryParams", "body"].includes(key)) ||
      value.method !== "GET" || typeof value.url !== "string" || value.body !== undefined) return invalid();
    const expected = `${this.selection.origin}${this.pathname}`;
    if (value.url !== expected + (this.inlineQuery ?? "")) return invalid();
    // Selected unchanged donors construct their query in the URL. Check exact
    // bytes, then serialize the selected values so literal +, & and # stay data.
    if (this.inlineQuery !== undefined && !["campaign-monitor.find-subscriber", "okta.find-user-by-email", "sendgrid.find-list-by-name"].includes(this.selection.profile)) return invalid();
    const url = new URL(expected);
    if (url.origin !== this.selection.origin || url.pathname !== this.pathname || url.username || url.password || url.search || url.hash) return invalid();
    const headers = new Headers({ Accept: "application/json" });
    const profile = this.selection.profile, secret = this.selection.secret;
    const expectedHeaders: Record<string, string> | undefined = contentHeaders(profile, secret) ?? (
      profile === "nocodb.get-record" ? { "xc-token": secret } :
      profile === "baserow.get-row" ? { Authorization: `Token ${secret}` } :
      profile === "bitly.get-bitlink-details" ? { Authorization: `Bearer ${secret}`, "Content-Type": "application/json" } :
      profile === "clockify.find-running-timer" ? { "X-Api-Key": secret } :
      profile === "pagerduty.get-incident" ? { Authorization: `Token token=${secret}`, Accept: "application/vnd.pagerduty+json;version=2", "Content-Type": "application/json" } :
      profile === "postmark.get-delivery-stats" ? { "X-Postmark-Server-Token": secret, Accept: "application/json", "Content-Type": "application/json" } : undefined);
    const selectedHeaders = profile === "okta.find-user-by-email" ? { Authorization: `SSWS ${secret}`, "Content-Type": "application/json", Accept: "application/json" } :
      profile === "toggl.find-project" ? { "Content-Type": "application/json", Authorization: `Basic ${Buffer.from(`${secret}:api_token`).toString("base64")}` } :
      profile === "notion.get-page" ? { authorization: `Bearer ${secret}`, "Notion-Version": "2022-02-22", "user-agent": "notionhq-client/2.2.14" } :
      profile === "intercom.get-conversation" ? { Authorization: `Bearer ${secret}`, "X-Fern-Language": "JavaScript", "X-Fern-SDK-Name": "intercom-client",
        "X-Fern-SDK-Version": "6.2.0", "User-Agent": "intercom-client/6.2.0", "Intercom-Version": "2.11", "X-Fern-Runtime": "node", "X-Fern-Runtime-Version": process.versions.node } :
      profile === "help-scout.find-conversation" ? { Authorization: `Bearer ${secret}`, "Content-Type": "application/json" } :
      profile === "brevo.find-contact" ? { "api-key": secret, accept: "application/json" } :
      profile === "jotform.get-submission" ? { APIKEY: secret } : expectedHeaders;
    if (selectedHeaders) {
      if (value.authentication !== undefined || !record(value.headers) || Object.keys(value.headers).length !== Object.keys(selectedHeaders).length) return invalid();
      for (const [key, item] of Object.entries(selectedHeaders)) {
        if (!Object.hasOwn(value.headers, key) || value.headers[key] !== item) return invalid();
        headers.set(key, item);
      }
    } else if (profile === "kit.get-subscriber") {
      // Kit V3 binds the selected secret in its query, never caller input.
      if (value.authentication !== undefined || value.headers !== undefined) return invalid();
    } else if (profile === "campaign-monitor.find-subscriber") {
      if (!record(value.headers) || Object.keys(value.headers).length !== 1 || value.headers["Content-Type"] !== "application/json" ||
        !record(value.authentication) || Object.keys(value.authentication).length !== 3 || value.authentication.type !== "BASIC" ||
        value.authentication.username !== secret || value.authentication.password !== "x") return invalid();
      headers.set("Content-Type", "application/json");
      headers.set("Authorization", `Basic ${Buffer.from(`${secret}:x`, "utf8").toString("base64")}`);
    } else if (profile === "confluence.get-page-content" || profile === "wordpress.get-page") {
      const props = record(this.selection.auth) ? this.selection.auth.props : undefined;
      if (!record(props) || typeof props.username !== "string" || value.headers !== undefined || !record(value.authentication) ||
        Object.keys(value.authentication).length !== 3 || value.authentication.type !== "BASIC" ||
        value.authentication.username !== props.username || value.authentication.password !== secret) return invalid();
      headers.set("Authorization", `Basic ${Buffer.from(`${props.username}:${secret}`, "utf8").toString("base64")}`);
    } else {
      const validHeaders = profile === "mailer-lite.get-campaign" ? record(value.headers) &&
        Object.keys(value.headers).length === 1 && Object.hasOwn(value.headers, "Accept") && value.headers.Accept === "application/json" : value.headers === undefined;
      if (!validHeaders || !record(value.authentication) || Object.keys(value.authentication).length !== 2 ||
        value.authentication.type !== "BEARER_TOKEN" || value.authentication.token !== this.selection.secret) return invalid();
      headers.set("Authorization", `Bearer ${this.selection.secret}`);
    }
    const expectedQuery: Readonly<Record<string, string>> | undefined = this.selectedQuery ?? (profile === "kit.get-subscriber" ? { api_secret: secret } : profile === "baserow.get-row" ? { user_field_names: "true" } :
      profile === "coda.get-row" ? { useColumnNames: "true", valueFormat: "simpleWithArrays" } : undefined);
    if (expectedQuery) {
      if (this.inlineQuery !== undefined ? value.queryParams !== undefined :
        !record(value.queryParams) || Object.keys(value.queryParams).length !== Object.keys(expectedQuery).length) return invalid();
      for (const [key, item] of Object.entries(expectedQuery)) {
        if (this.inlineQuery === undefined && (!record(value.queryParams) || !Object.hasOwn(value.queryParams, key) || value.queryParams[key] !== item)) return invalid();
        url.searchParams.set(key, item);
      }
    } else if (value.queryParams !== undefined) {
      if (!record(value.queryParams)) return invalid();
      for (const [key, item] of Object.entries(value.queryParams)) {
        if (this.selection.profile !== "asana.get-task" || key !== "opt_fields" || typeof item !== "string" || item.length > 8192) return invalid();
        url.searchParams.set(key, item);
      }
    }
    return { url: url.toString(), headers };
  }

  readonly request = (value: unknown): Promise<DonorResponse> => {
    if (this.pending.size) return Promise.reject(this.fail("upstream_error"));
    const pending = this.perform(value);
    this.pending.add(pending);
    void pending.then(() => this.pending.delete(pending), () => this.pending.delete(pending));
    return pending;
  };
  private readonly perform = async (value: unknown): Promise<DonorResponse> => {
    this.check();
    if (this.dispatched && !this.readState) throw this.fail("upstream_error");
    const { url, headers, method = "GET", body: requestBody } = this.prepare(value);
    this.dispatched = true;
    let response: Response;
    try {
      // Await actual host completion even after abort. A host that ignores the
      // signal cannot be safely abandoned with a later response left unowned.
      response = await this.selection.transport(url, {
        method, headers, ...(requestBody === undefined ? {} : { body: requestBody }), signal: this.controller.signal, redirect: "error",
      });
    } catch { throw this.fail("transport_error"); }
    if (!(response instanceof Response) || response.body?.locked) throw this.fail("invalid_response");
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let finished = false;
    let cancellation: Promise<void> | undefined;
    let cleanupFailed = false;
    const cancel = (): Promise<void> => {
      cancellation ??= (reader ? reader.cancel() : Promise.resolve()).catch(() => { cleanupFailed = true; });
      return cancellation;
    };
    const onAbort = () => { void cancel(); };
    try {
      reader = response.body?.getReader();
      this.controller.signal.addEventListener("abort", onAbort, { once: true });
      this.check();
      if (response.redirected || (response.url && response.url !== url) || (response.status >= 300 && response.status < 400)) {
        throw this.fail("redirect_refused");
      }
      if (response.status < 200 || response.status >= 300) {
        // This exact donor defines 404 as a missing contact. Let its real
        // HttpError handler produce found:false; all other errors stay errors.
        if (!(["brevo.find-contact", "campaign-monitor.find-subscriber"].includes(this.selection.profile) && response.status === 404)) this.fail("http_error", response.status);
        // The seam throws the real donor HttpError. Retain only safe metadata
        // here, so donor formatting cannot reflect a response or credential.
        const retryAfter = response.headers.get("retry-after");
        return { status: response.status, headers: retryAfter && /^[0-9]{1,12}$/.test(retryAfter) ? { "retry-after": retryAfter } : {}, body: undefined };
      }
      const declaredLength = response.headers.get("content-length");
      // A borrowed Response may contain decoded bytes while retaining compressed
      // wire metadata. Only identity content has a comparable declared length.
      // Reject ambiguous/malformed framing instead of certifying an acknowledgement.
      if (declaredLength !== null && (!/^[0-9]+$/.test(declaredLength) || response.headers.has("transfer-encoding"))) {
        throw this.fail("invalid_response");
      }
      const contentEncoding = response.headers.get("content-encoding");
      const codings = contentEncoding?.split(",").map(value => value.trim().toLowerCase());
      if (codings?.some(value => !/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(value))) throw this.fail("invalid_response");
      let expectedBytes: number | undefined;
      if (declaredLength !== null && (!codings || codings.every(value => value === "identity"))) {
        // Compare decimal strings before Number conversion to avoid overflow or
        // precision loss, even for a large numeral or many leading zeroes.
        const decimal = declaredLength.replace(/^0+/, "") || "0";
        const limit = String(this.selection.maxResponseBytes);
        if (decimal.length > limit.length || (decimal.length === limit.length && decimal > limit)) {
          throw this.fail("response_too_large");
        }
        expectedBytes = Number(decimal);
      }
      const chunks: Uint8Array[] = [];
      let size = 0;
      while (reader) {
        this.check();
        const part = await reader.read();
        this.check();
        if (part.done) { finished = true; break; }
        if (!(part.value instanceof Uint8Array)) throw this.fail("invalid_response");
        size += part.value.byteLength;
        if (expectedBytes !== undefined && size > expectedBytes) throw this.fail("invalid_response");
        this.receivedBytes += part.value.byteLength;
        if (this.receivedBytes > this.selection.maxResponseBytes) throw this.fail("response_too_large");
        if (size > this.selection.maxResponseBytes) throw this.fail("response_too_large");
        chunks.push(part.value.slice());
      }
      if (expectedBytes !== undefined && size !== expectedBytes) throw this.fail("invalid_response");
      const bytes = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
      let body: unknown;
      try { const text = new TextDecoder("utf-8", {fatal:true}).decode(bytes); body = this.readState instanceof SdkRequestState && this.readState.responseType === "xml" ? text : JSON.parse(text); }
      catch { throw this.fail("invalid_response"); }
      const contentFailure = this.readState?.bodyFailure(body) ?? contentBodyFailure(this.selection, this.pathname, this.selectedQuery, body);
      if (contentFailure) throw this.fail(contentFailure);
      if (this.selection.profile === "jotform.get-submission" &&
        (!record(body) || !Number.isInteger(body.responseCode) || (body.responseCode as number) < 200 || (body.responseCode as number) > 599)) {
        throw this.fail("invalid_response");
      }
      const envelopeField = this.selection.profile === "kit.get-subscriber" ? "subscriber" :
        this.selection.profile === "mailer-lite.get-campaign" ? "data" : this.selection.profile === "calendly.get-scheduled-event" ? "resource" : undefined;
      if (envelopeField && (!record(body) || !Object.hasOwn(body, envelopeField) || !record(body[envelopeField]))) {
        throw this.fail("invalid_response");
      }
      if (this.selection.profile === "google-tasks.get-task" &&
        (!record(body) || body.id !== this.pathname.slice(this.pathname.lastIndexOf("/") + 1))) throw this.fail("invalid_response");
      if (this.selection.profile === "campaign-monitor.find-subscriber") {
        const email = this.selectedQuery?.email;
        if (!record(body) || typeof body.EmailAddress !== "string" || /[^\x00-\x7f]/.test(body.EmailAddress) || body.EmailAddress.toLowerCase() !== email?.toLowerCase() ||
          !Array.isArray(body.CustomFields) || body.CustomFields.length > 100 || !body.CustomFields.every(field => record(field) &&
            typeof field.Key === "string" && typeof field.Value === "string" && !Object.hasOwn(Object.prototype, field.Key))) {
          // Its unchanged plain-object transform cannot safely represent inherited
          // property names. Reject before its catch-all can report a false miss.
          throw this.fail("invalid_response");
        }
      }
      if (this.selection.profile === "okta.find-user-by-email") {
        const email = this.selectedQuery?.search?.slice('profile.email eq "'.length, -1);
        if (!Array.isArray(body) || body.length > 200 || !body.every(user => record(user) && record(user.profile) &&
          typeof user.profile.email === "string" && !/[^\x00-\x7f]/.test(user.profile.email) && user.profile.email.toLowerCase() === email?.toLowerCase())) {
          throw this.fail("invalid_response");
        }
      }
      if (this.selection.profile === "help-scout.find-conversation" &&
        (!record(body) || !record(body._embedded) || !Array.isArray(body._embedded.conversations) ||
          body._embedded.conversations.length > 200 || !body._embedded.conversations.every(record))) {
        throw this.fail("invalid_response");
      }
      const lastId = decodeURIComponent(this.pathname.slice(this.pathname.lastIndexOf("/") + 1));
      if (["airtable.get-record", "stripe.get-invoice", "google-calendar.get-event", "notion.get-page", "intercom.get-conversation"].includes(this.selection.profile) &&
        (!record(body) || body.id !== lastId)) throw this.fail("invalid_response");
      if (this.selection.profile === "wordpress.get-page" && (!record(body) || body.id !== Number(lastId))) throw this.fail("invalid_response");
      if (this.selection.profile === "pipedrive.get-activity" &&
        (!record(body) || body.success !== true || !record(body.data) || body.data.id !== Number(lastId))) throw this.fail("invalid_response");
      if (this.selection.profile === "toggl.find-project" &&
        (!Array.isArray(body) || body.length > Number(this.selectedQuery?.per_page) || !body.every(item => record(item) && Number.isSafeInteger(item.id) && (item.id as number) > 0 &&
          item.workspace_id === Number(this.pathname.split("/")[4])))) throw this.fail("invalid_response");
      this.check();
      return { status: response.status, headers: {}, body };
    } catch (error) {
      if (error === this.fault) throw error;
      throw this.fail("invalid_response");
    } finally {
      this.controller.signal.removeEventListener("abort", onAbort);
      if (!finished) await cancel();
      if (cancellation) await cancellation;
      try { reader?.releaseLock(); } catch { cleanupFailed = true; }
      if (cleanupFailed) throw this.fail("transport_error");
    }
  };
}
