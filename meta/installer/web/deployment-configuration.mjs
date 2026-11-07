/** Missing private setup fields, projected as package metadata only. */
function nonblank(value) {
  // Match Python str.strip() at the destination; never normalize private input.
  return typeof value === "string" && !/^[\u0009-\u000d\u001c-\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]*$/.test(value);
}

export function deploymentRequiredFields(options, values = {}, useHost = false) {
  const inherited = useHost ? new Set(options?.host_provider_keys || []) : new Set();
  return (options?.fields || []).filter((field) => {
    if (field.automatic || !field.required || field.configured ||
        (!field.secret && field.default) || inherited.has(field.key)) return false;
    return !nonblank(values[field.key]);
  }).map((field) => ({ key: field.key, label: field.label }));
}

export function deploymentFieldId(guideId, fieldKey) {
  return `deployment-setting-${guideId}-${fieldKey}`;
}

/** Public field constraints are hints. The destination validates actual values. */
export function deploymentFieldConstraintsValid(field) {
  const length = (value) => Number.isInteger(value) && value >= 0 && value <= 32768;
  if (Object.hasOwn(field, "min_length") && !length(field.min_length) ||
      Object.hasOwn(field, "max_length") && !length(field.max_length) ||
      field.min_length != null && field.max_length != null && field.min_length > field.max_length ||
      field.required && field.max_length === 0 ||
      Object.hasOwn(field, "nonblank") && typeof field.nonblank !== "boolean") return false;
  if (!Object.hasOwn(field, "type")) return !Object.hasOwn(field, "minimum") && !Object.hasOwn(field, "maximum");
  if (field.type !== "integer" || !Number.isSafeInteger(field.minimum) ||
      !Number.isSafeInteger(field.maximum) || field.minimum > field.maximum) return false;
  const closest = field.minimum <= 0 && field.maximum >= 0 ? 0 : field.minimum > 0 ? field.minimum : field.maximum;
  return field.max_length == null || String(closest).length <= field.max_length;
}

// The destination's own email rule for an OIDC Console (console_configuration.py).
const CONSOLE_EMAIL = /^[A-Za-z0-9_.+-]{1,64}@[A-Za-z0-9-]{1,63}(?:\.[A-Za-z0-9-]{1,63})+$/;

function allowedEmails(value) {
  const emails = value.trim().split(/[\s,]+/).filter(Boolean);
  return value.length <= 32768 && emails.length <= 100 && emails.every((email) => CONSOLE_EMAIL.test(email));
}

/** Describe invalid supplied input without returning or normalizing any value. */
export function deploymentFieldIssues(options, values = {}, removed = []) {
  const removal = new Set(removed), issues = [];
  for (const field of options?.fields || []) {
    if (field.automatic || removal.has(field.key)) continue;
    const value = values[field.key];
    // Blank replacement means retain the saved setting or use its source default.
    if (typeof value !== "string" || value === "") continue;
    let detail = null;
    const length = Array.from(value).length;
    if (value.includes("\0") || length > 32768) detail = "Enter a value of at most 32768 characters without null characters.";
    else if ((field.required || field.nonblank) && !nonblank(value)) detail = "Enter a value that is not only whitespace.";
    else if (Number.isInteger(field.min_length) && length < field.min_length) detail = `Use at least ${field.min_length} characters.`;
    else if (Number.isInteger(field.max_length) && length > field.max_length) detail = `Use at most ${field.max_length} characters.`;
    else if (field.type === "integer") {
      const integer = /^[+-]?[0-9]+$/.test(value) && !/[^+0-9-]/.test(value) ? Number(value) : NaN;
      if (!Number.isSafeInteger(integer) || integer < field.minimum || integer > field.maximum)
        detail = `Enter a whole number between ${field.minimum} and ${field.maximum}, without spaces.`;
    }
    // Only a declared authenticated Console binds these runtime credentials.
    // Matching names in an unrelated application do not establish that contract.
    // With named accounts they are the installation's own account.
    const authentication = options?.console_scope?.authentication;
    if (!detail && ["local-password", "local-accounts", "oidc"].includes(authentication)) {
      if (field.key === "CONSOLE_USER_ID" && (length < 1 || length > 200 || /[^A-Za-z0-9_.@+-]/.test(value)))
        detail = "Use 1-200 letters, numbers, dots, underscores, @, + or hyphens.";
      else if (field.key === "CONSOLE_USER_ID" && authentication === "oidc" && !CONSOLE_EMAIL.test(value))
        detail = "Use the operator's email address, as the identity provider reports it.";
      else if (field.key === "CONSOLE_ALLOWED_EMAILS" && authentication === "oidc" && !allowedEmails(value))
        detail = "List up to 100 email addresses, separated by commas or spaces.";
      else if (field.key === "CONSOLE_LOGIN_PASSWORD" && (length < 12 || length > 4096))
        detail = "Use 12-4096 characters for the Console password.";
      else if (field.key === "CONSOLE_LOGIN_PASSWORD" && /[\uD800-\uDFFF]/u.test(value))
        detail = "Use valid UTF-8 text for the Console login password.";
    }
    if (detail) issues.push({ key: field.key, label: field.label, detail });
  }
  return issues;
}

/** Only optional, explicitly saved package fields can be removed. */
export function deploymentRemovableFields(options) {
  return (options?.fields || []).filter((field) => field.required === false &&
    field.automatic !== true && field.configured === true && field.removable === true &&
    /^[A-Z][A-Z0-9_]*$/.test(field.key));
}

export function deploymentRemovalKeys(options, selected = []) {
  if (!Array.isArray(selected) || selected.length > 100 || new Set(selected).size !== selected.length) return null;
  const allowed = new Set(deploymentRemovableFields(options).map((field) => field.key));
  return selected.every((key) => typeof key === "string" && allowed.has(key)) ? [...selected] : null;
}
