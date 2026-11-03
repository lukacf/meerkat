/* Presentation helpers. The installed check-config command owns semantics. */
export class FormError extends Error {
  constructor(code, field) { super(code); this.code = code; this.field = field; }
}

const record = value => value !== null && typeof value === "object" && !Array.isArray(value);
const text = value => typeof value === "string" && value.length > 0 && value.length <= 8192;
const name = value => text(value) && /^[A-Za-z_][A-Za-z0-9_]*$/.test(value);
const effects = new Set(["read", "write", "unspecified"]);
const kinds = new Set(["string", "integer", "boolean", "string_list"]);

export const stringListValue = value => Array.isArray(value) && value.length <= 256 &&
  value.every(item => typeof item === "string" && [...item].length <= 2048 && !/[\x00-\x1f\x7f]/.test(item));

export const fieldText = value => Array.isArray(value) ? JSON.stringify(value) : String(value);

export function normalizeHelp(value, family, profile) {
  if (!record(value) || value.family !== family || value.profile !== profile ||
      !Array.isArray(value.operations) || value.operations.length === 0 ||
      !Array.isArray(value.fields) || !Array.isArray(value.prerequisites) ||
      !Array.isArray(value.unresolved_details) || !text(value.scope)) throw new FormError("invalid_help");
  const selectable = Object.hasOwn(value, "selectable_operations") ? value.selectable_operations : true;
  if (typeof selectable !== "boolean") throw new FormError("invalid_help");
  const ids = new Set();
  const operations = value.operations.map(item => {
    if (!record(item) || !text(item.id) || ids.has(item.id) || !effects.has(item.effect)) throw new FormError("invalid_help");
    ids.add(item.id);
    return { id: item.id, label: text(item.label) ? item.label : item.id, effect: item.effect };
  });
  const names = new Set();
  const fields = value.fields.map(item => {
    if (!record(item) || !name(item.name) || names.has(item.name) ||
        ["profile", "operations", "config", "__proto__", "prototype", "constructor"].includes(item.name) ||
        !kinds.has(item.kind) || typeof item.required !== "boolean" ||
        typeof item.credential_reference !== "boolean" ||
        (item.credential_reference && item.kind !== "string")) throw new FormError("invalid_help");
    names.add(item.name);
    const field = { name: item.name, kind: item.kind, required: item.required, credential_reference: item.credential_reference };
    for (const bound of ["minimum", "maximum"]) {
      if (item[bound] !== undefined) {
        if (item.kind !== "integer" || !Number.isSafeInteger(item[bound])) throw new FormError("invalid_help");
        field[bound] = item[bound];
      }
    }
    if (field.minimum !== undefined && field.maximum !== undefined && field.minimum > field.maximum) throw new FormError("invalid_help");
    if (item.only_when_operation_selected !== undefined) {
      if (!ids.has(item.only_when_operation_selected)) throw new FormError("invalid_help");
      field.only_when_operation_selected = item.only_when_operation_selected;
    }
    return field;
  });
  const prerequisites = value.prerequisites.map(item => {
    if (!record(item) || !text(item.kind) || !text(item.instruction)) throw new FormError("invalid_help");
    return { kind: item.kind, instruction: item.instruction };
  });
  if (!value.unresolved_details.every(text) ||
      (value.guide_text !== undefined && (typeof value.guide_text !== "string" || value.guide_text.length > 1_000_000))) throw new FormError("invalid_help");
  return { family, profile, label: text(value.label) ? value.label : profile, operations, selectable_operations: selectable, fields, prerequisites,
    unresolved_details: [...value.unresolved_details], scope: value.scope, guide_text: value.guide_text ?? "" };
}

export function applicable(field, operations) {
  return field.only_when_operation_selected === undefined || operations.includes(field.only_when_operation_selected);
}

export function draftFromFields(help, operations, entered) {
  const allowed = new Set(help.operations.map(item => item.id));
  if (!Array.isArray(operations) || operations.length === 0 || new Set(operations).size !== operations.length ||
      operations.some(item => !allowed.has(item))) throw new FormError("operations_required");
  const values = Object.create(null);
  for (const field of help.fields) {
    if (!applicable(field, operations)) continue;
    const value = entered[field.name] ?? "";
    if (value === "") {
      if (field.required) throw new FormError("field_required", field.name);
      continue;
    }
    if (typeof value !== "string") throw new FormError("invalid_field", field.name);
    if (field.kind === "integer") {
      if (!/^-?(0|[1-9][0-9]*)$/.test(value) || !Number.isSafeInteger(Number(value))) throw new FormError("integer_required", field.name);
      values[field.name] = Number(value);
      if ((field.minimum !== undefined && values[field.name] < field.minimum) ||
          (field.maximum !== undefined && values[field.name] > field.maximum)) throw new FormError("field_out_of_bounds", field.name);
    } else if (field.kind === "boolean") {
      if (value !== "true" && value !== "false") throw new FormError("boolean_required", field.name);
      values[field.name] = value === "true";
    } else if (field.kind === "string_list") {
      let items;
      try { items = JSON.parse(value); } catch { throw new FormError("string_list_required", field.name); }
      if (!stringListValue(items)) throw new FormError("string_list_required", field.name);
      values[field.name] = items;
    } else {
      if (field.credential_reference && !/^[A-Za-z_][A-Za-z0-9_]{0,127}$/.test(value)) throw new FormError("reference_name_required", field.name);
      values[field.name] = value;
    }
  }
  return { family: help.family, profile: help.profile, operations: [...operations], values };
}

export function takeSession(location, history) {
  const fragment = location.hash;
  history.replaceState(null, "", location.pathname + location.search);
  const params = new URLSearchParams(fragment.startsWith("#") ? fragment.slice(1) : fragment);
  const keys = [...params.keys()];
  const token = params.get("session");
  return keys.length === 1 && keys[0] === "session" && token && token.length <= 1024 ? token : null;
}
