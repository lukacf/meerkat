const itemId = (obs) => obs.provider_item_id ?? obs.item_id;
const hasIdentity = (obs) => Boolean(obs.response_id || itemId(obs));

export class Transcript {
  constructor() { this.reset(); }

  reset() {
    this.turns = [];
    this.interruptedSpokenScopes = [];
    this.completedResponses = new Set();
    this.anonymousGeneration = 0;
    this.anonymousSpokenBlocked = false;
    this.activeSpoken = undefined;
  }

  matches(row, obs) {
    if (!hasIdentity(obs)) return row.anonymousGeneration === this.anonymousGeneration;
    return (!obs.response_id || row.responseId === obs.response_id)
      && (!itemId(obs) || row.itemId === itemId(obs))
      && (obs.content_index == null || row.contentIndex === obs.content_index);
  }

  correlate(obs) {
    if (!itemId(obs)) return obs;
    const responses = new Set(this.turns.filter((row) => row.itemId === itemId(obs) && row.responseId)
      .map((row) => row.responseId));
    for (const scope of this.interruptedSpokenScopes) {
      if (itemId(scope) === itemId(obs) && scope.response_id) responses.add(scope.response_id);
    }
    if (obs.response_id) responses.add(obs.response_id);
    if (responses.size > 1) return undefined;
    if (responses.size === 1) {
      const responseId = [...responses][0];
      for (const scope of this.interruptedSpokenScopes) {
        if (itemId(scope) === itemId(obs)) scope.response_id ??= responseId;
      }
      return obs.response_id ? obs : { ...obs, response_id: responseId };
    }
    return obs;
  }

  blocked(obs, modality) {
    // Missing identity cannot distinguish late output from an interrupted scope.
    return this.completedResponses.has(obs.response_id) || (modality === "spoken" && (
      this.interruptedSpokenScopes.some((scope) =>
        (!scope.response_id || !obs.response_id || scope.response_id === obs.response_id)
        && (!itemId(scope) || !itemId(obs) || itemId(scope) === itemId(obs))
        && (scope.content_index == null || obs.content_index == null
          || scope.content_index === obs.content_index))
      || (!hasIdentity(obs) && this.anonymousSpokenBlocked)
    ));
  }

  add(obs, modality, time) {
    const row = {
      role: "assistant", text: "", draft: true, time, modality,
      responseId: obs.response_id, itemId: itemId(obs), contentIndex: obs.content_index,
      anonymousGeneration: hasIdentity(obs) ? undefined : this.anonymousGeneration,
    };
    this.turns.push(row);
    return row;
  }

  findRow(obs, modality) {
    const rows = this.turns.filter((row) => row.modality === modality
      && (this.matches(row, obs) || (
        ((obs.response_id && row.responseId === obs.response_id) || (itemId(obs) && row.itemId === itemId(obs)))
        && (!row.responseId || !obs.response_id || row.responseId === obs.response_id)
        && (!row.itemId || !itemId(obs) || row.itemId === itemId(obs))
        && (row.contentIndex == null || obs.content_index == null || row.contentIndex === obs.content_index)
      )));
    if (rows.length !== 1) return undefined;
    const row = rows[0];
    row.responseId ??= obs.response_id;
    row.itemId ??= itemId(obs);
    row.contentIndex ??= obs.content_index;
    return row;
  }

  delta(obs, modality, time) {
    obs = this.correlate(obs);
    if (!obs) return;
    if (this.blocked(obs, modality)) return;
    let row = this.findRow(obs, modality);
    if (row && !row.draft) return;
    row ??= this.add(obs, modality, time);
    row.text += obs.delta || "";
    if (modality === "spoken") this.activeSpoken = row;
  }

  final(obs, time) {
    obs = this.correlate(obs);
    if (!obs) return;
    const row = this.findRow(obs, "spoken") ?? this.findRow(obs, "written");
    const modality = row?.modality ?? "spoken";
    if (this.blocked(obs, modality) || (row && !row.draft)) return;
    const target = row ?? this.add(obs, modality, time);
    target.text = obs.text ?? target.text;
    target.draft = false;
  }

  interrupt(obs, time, truncated = false) {
    obs = this.correlate(obs);
    if (!obs) return;
    let target = hasIdentity(obs) ? obs : this.activeSpoken ? {
      response_id: this.activeSpoken.responseId,
      provider_item_id: this.activeSpoken.itemId,
      content_index: this.activeSpoken.contentIndex,
    } : {};
    if (!truncated && target.response_id) target = { response_id: target.response_id };
    // Item-scoped truncation must not discard another spoken item in the response.
    if (hasIdentity(target)) this.interruptedSpokenScopes.push(target);
    this.anonymousSpokenBlocked = true;
    const rows = !hasIdentity(target) && this.activeSpoken
      ? [this.activeSpoken]
      : this.turns.filter((row) => row.modality === "spoken" && this.matches(row, target));
    if (rows.length === 0 && itemId(target)) {
      const row = this.findRow(target, "spoken");
      if (row) rows.push(row);
    }
    if (truncated && rows.length === 0 && hasIdentity(target) && obs.text != null) {
      rows.push(this.add(target, "spoken", time));
    }
    for (const row of rows) {
      row.draft = false;
      row.interrupted = true;
      if (truncated && obs.text != null) row.text = obs.text;
    }
  }

  complete(obs) {
    if (obs.response_id) this.completedResponses.add(obs.response_id);
    for (const row of this.turns) {
      if (row.role === "assistant" && this.matches(row, obs)) row.draft = false;
    }
    if (!hasIdentity(obs)) this.anonymousGeneration += 1;
  }
}
