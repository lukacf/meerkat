// Find test assertions that pass a DOM node, or a list of nodes, to an
// equality assertion.
//
// node:assert formats both operands when an equality assertion fails. For a
// rendered element that formatting walks the attached React tree, and one
// failing row can grow to tens of gigabytes before it reports. Compare a
// boolean instead: assert.equal(element === null, true) or
// assert.equal(document.activeElement === editor, true).

const EQUALITY_CALL = new RegExp(String.raw`\bassert\s*\.\s*(equal|strictEqual|deepEqual|deepStrictEqual|`
  + String.raw`notEqual|notStrictEqual|notDeepEqual|notDeepStrictEqual)\s*\(`, "g");
const NODE_METHODS = new Set(["querySelector", "getElementById", "closest", "elementFromPoint"]);
const COLLECTION_METHODS = new Set(["querySelectorAll", "getElementsByClassName", "getElementsByTagName",
  "getElementsByName"]);
const NODE_PROPERTIES = new Set(["activeElement", "firstElementChild", "lastElementChild", "parentElement",
  "nextElementSibling", "previousElementSibling", "firstChild", "lastChild", "parentNode", "nextSibling",
  "previousSibling", "offsetParent", "ownerDocument", "contentDocument", "shadowRoot"]);
const COLLECTION_PROPERTIES = new Set(["children", "childNodes"]);
// A document's own nodes. Elsewhere, body is usually a request body.
const DOCUMENT_NODES = new Set(["body", "head", "documentElement"]);
const DOCUMENT = /(?:^|\.)\s*(?:document|doc|ownerDocument)$/;
// Node properties that hold text, numbers, flags or plain objects, never a node.
const VALUE_PROPERTIES = new Set(["textContent", "innerText", "innerHTML", "outerHTML", "nodeValue", "data",
  "value", "defaultValue", "checked", "defaultChecked", "indeterminate", "disabled", "hidden", "open", "selected",
  "selectedIndex", "readOnly", "required", "multiple", "tagName", "nodeName", "nodeType", "localName", "id",
  "className", "href", "src", "type", "name", "title", "placeholder", "alt", "rel", "lang", "dir", "role",
  "tabIndex", "isConnected", "length", "size", "dataset", "style", "classList", "attributes",
  "selectionStart", "selectionEnd", "scrollTop", "scrollLeft", "scrollHeight", "clientHeight", "offsetHeight",
  "offsetWidth", "validationMessage", "files"]);
// A literal operand. Masked strings keep their quotes, and template literals
// are blanked whole, so their contents never match an operator.
const LITERAL = /^(?:"[^"]*"|'[^']*'|`[^`]*`|-?(?:\d[\d_]*(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?n?|true|false|NaN|-?Infinity)$/;
const KEYWORDS = new Set(["null", "undefined", "this", "super", "arguments"]);
// Calls that return text, a number or a flag.
const PRIMITIVE_METHODS = new Set(["trim", "trimStart", "trimEnd", "toLowerCase", "toUpperCase", "toString",
  "toFixed", "join", "getAttribute", "hasAttribute", "contains", "matches", "includes", "startsWith", "endsWith",
  "indexOf", "some", "every", "test", "stringify"]);
const PRIMITIVE_FUNCTIONS = new Set(["String", "Number", "Boolean"]);
// Array methods that return one item of their receiver, or a subset of it.
const PICK_METHODS = new Set(["find", "findLast", "at", "item", "pop", "shift"]);
const SUBSET_METHODS = new Set(["filter", "slice", "reverse", "toReversed", "sort", "toSorted"]);
const NODE = "node";
const COLLECTION = "collection";
const IDENTIFIER = String.raw`[A-Za-z_$][\w$]*`;
// The member at the end of a reference, and a bare name.
const MEMBER = new RegExp(String.raw`(?:\?\.|\.)\s*(${IDENTIFIER})$`);
const NAME = new RegExp(String.raw`^(${IDENTIFIER})$`);
// A function expression holds a function, never a node.
const FUNCTION_VALUE = new RegExp(String.raw`^(?:async\s+)?(?:function\b|(?:\([^()]*\)|${IDENTIFIER})\s*=>)`);
const WORD = /[\w$]/;
const REGEX_AFTER_WORD = new Set(["return", "typeof", "case", "do", "else", "in", "of", "new", "delete",
  "void", "throw", "instanceof", "yield", "await"]);
const REGEX_AFTER_PUNCTUATION = new Set([..."(,=:[!&|?{};+-*%<>~^"]);
const CONTINUATION = new Set([...".?:|&+-*/%=<>"]);
const MAX_HELPER_DEPTH = 4;

function blank(out, from, to) {
  for (let index = from; index < to; index += 1) {
    if (out[index] !== "\n") out[index] = " ";
  }
}

function skipString(source, start, quote) {
  let index = start + 1;
  while (index < source.length) {
    const character = source[index];
    if (character === "\\") index += 2;
    else if (character === quote) return index + 1;
    else if (character === "\n") return index;
    else index += 1;
  }
  return index;
}

function skipRegex(source, start) {
  let index = start + 1;
  let inClass = false;
  while (index < source.length) {
    const character = source[index];
    if (character === "\\") index += 2;
    else if (character === "\n") return index;
    else if (inClass) {
      if (character === "]") inClass = false;
      index += 1;
    } else if (character === "[") {
      inClass = true;
      index += 1;
    } else if (character === "/") {
      index += 1;
      while (index < source.length && /[a-z]/i.test(source[index])) index += 1;
      return index;
    } else index += 1;
  }
  return index;
}

function skipTemplate(source, out, start) {
  let index = start + 1;
  while (index < source.length) {
    const character = source[index];
    if (character === "\\") index += 2;
    else if (character === "`") return index + 1;
    else if (character === "$" && source[index + 1] === "{") index = scanCode(source, out, index + 2, true) + 1;
    else index += 1;
  }
  return index;
}

// Blank comments and literal contents in place. A template expression scan
// returns the index of its closing brace.
function scanCode(source, out, start, untilBrace) {
  let index = start;
  let depth = 0;
  let previous = "";
  let previousWord = false;
  while (index < source.length) {
    const character = source[index];
    const next = source[index + 1];
    if (character === "/" && next === "/") {
      const end = source.indexOf("\n", index);
      const stop = end < 0 ? source.length : end;
      blank(out, index, stop);
      index = stop;
    } else if (character === "/" && next === "*") {
      const end = source.indexOf("*/", index + 2);
      const stop = end < 0 ? source.length : end + 2;
      blank(out, index, stop);
      index = stop;
    } else if (character === "'" || character === "\"") {
      const stop = skipString(source, index, character);
      blank(out, index + 1, stop - 1);
      index = stop;
      previous = character;
      previousWord = false;
    } else if (character === "`") {
      const stop = skipTemplate(source, out, index);
      blank(out, index + 1, stop - 1);
      index = stop;
      previous = character;
      previousWord = false;
    } else if (character === "/" && (previous === ""
        || (previousWord ? REGEX_AFTER_WORD.has(previous) : REGEX_AFTER_PUNCTUATION.has(previous)))) {
      const stop = skipRegex(source, index);
      blank(out, index + 1, stop - 1);
      index = stop;
      previous = "/";
      previousWord = false;
    } else if (WORD.test(character)) {
      // A number or a name divides; a keyword such as return starts a regex.
      let stop = index;
      while (stop < source.length && WORD.test(source[stop])) stop += 1;
      previous = source.slice(index, stop);
      previousWord = true;
      index = stop;
    } else if (/\s/.test(character)) {
      index += 1;
    } else {
      if (character === "{") depth += 1;
      else if (character === "}") {
        if (untilBrace && depth === 0) return index;
        depth -= 1;
      }
      previous = character;
      previousWord = false;
      index += 1;
    }
  }
  return index;
}

export function maskSource(source) {
  // UTF-16 units, so masked offsets match source.slice offsets.
  const out = source.split("");
  scanCode(source, out, 0, false);
  return out.join("");
}

const PAIRS = { "(": ")", "[": "]", "{": "}" };
const REVERSE = { ")": "(", "]": "[", "}": "{" };

function closing(text, open) {
  const stack = [PAIRS[text[open]]];
  for (let index = open + 1; index < text.length; index += 1) {
    const character = text[index];
    if (PAIRS[character]) stack.push(PAIRS[character]);
    else if (REVERSE[character]) {
      if (stack.pop() !== character) return -1;
      if (stack.length === 0) return index;
    }
  }
  return -1;
}

function opening(text, close) {
  const stack = [REVERSE[text[close]]];
  for (let index = close - 1; index >= 0; index -= 1) {
    const character = text[index];
    if (REVERSE[character]) stack.push(REVERSE[character]);
    else if (PAIRS[character]) {
      if (stack.pop() !== character) return -1;
      if (stack.length === 0) return index;
    }
  }
  return -1;
}

function trimmedSpan(text, start, end) {
  while (start < end && /\s/.test(text[start])) start += 1;
  while (end > start && /\s/.test(text[end - 1])) end -= 1;
  return [start, end];
}

function argumentSpans(text, open, close) {
  const spans = [];
  let depth = 0;
  let start = open + 1;
  for (let index = open + 1; index < close; index += 1) {
    const character = text[index];
    if (PAIRS[character]) depth += 1;
    else if (REVERSE[character]) depth -= 1;
    else if (character === "," && depth === 0) {
      spans.push(trimmedSpan(text, start, index));
      start = index + 1;
    }
  }
  const last = trimmedSpan(text, start, close);
  if (last[1] > last[0]) spans.push(last);
  return spans;
}

// An arrow body or assigned value ends at a top-level ";" or ",", at the
// enclosing bracket, or at a line break that does not continue it.
function expressionEnd(text, start) {
  let depth = 0;
  for (let index = start; index < text.length; index += 1) {
    const character = text[index];
    if (PAIRS[character]) depth += 1;
    else if (REVERSE[character]) {
      if (depth === 0) return index;
      depth -= 1;
    } else if (depth === 0 && (character === ";" || character === ",")) return index;
    else if (depth === 0 && character === "\n") {
      const rest = text.slice(index + 1).match(/^\s*(\S)/);
      if (!rest || !CONTINUATION.has(rest[1])) return index;
    }
  }
  return text.length;
}

function blockAt(text, brace) {
  const end = closing(text, brace);
  return end < 0 ? null : text.slice(brace, end + 1);
}

function bodyAfterArrow(text, arrowEnd) {
  let start = arrowEnd;
  while (start < text.length && /\s/.test(text[start])) start += 1;
  if (text[start] === "{") return blockAt(text, start);
  return text.slice(start, expressionEnd(text, start));
}

function functionBody(text, from) {
  const open = text.indexOf("(", from);
  if (open < 0) return null;
  const close = closing(text, open);
  if (close < 0) return null;
  const brace = text.indexOf("{", close);
  return brace < 0 ? null : blockAt(text, brace);
}

function escaped(name) {
  return name.replace(/\$/g, "\\$");
}

// Each definition of a same-file helper, with the index where it is written.
function definitions(masked, name, member) {
  const bodies = [];
  const add = (at, body) => {
    if (body) bodies.push({ at, body });
  };
  const id = escaped(name);
  for (const match of masked.matchAll(new RegExp(String.raw`(?<![\w$.])function\s*\*?\s*${id}\s*\(`, "g"))) {
    add(match.index, functionBody(masked, match.index));
  }
  const arrow = String.raw`(?:async\s+)?(?:\([^()]*\)|${IDENTIFIER})\s*=>`;
  const binding = member ? String.raw`(?<![\w$.])${id}\s*:\s*`
    : String.raw`(?<![\w$.])(?:const|let|var)\s+${id}\s*=\s*`;
  for (const match of masked.matchAll(new RegExp(binding + arrow, "g"))) {
    add(match.index, bodyAfterArrow(masked, match.index + match[0].length));
  }
  for (const match of masked.matchAll(new RegExp(binding + String.raw`(?:async\s+)?function\b`, "g"))) {
    add(match.index, functionBody(masked, match.index + match[0].length));
  }
  if (member) {
    const shorthand = new RegExp(String.raw`(?<=[{,;\n]\s*)(?:async\s+)?(?:get\s+)?${id}\s*\([^()]*\)\s*\{`, "g");
    for (const match of masked.matchAll(shorthand)) {
      add(match.index, blockAt(masked, match.index + match[0].length - 1));
    }
  }
  return bodies;
}

function returnedValues(body) {
  if (!body.startsWith("{")) return [body];
  return [...body.matchAll(/\breturn\b/g)].map((match) => {
    const start = match.index + match[0].length;
    return body.slice(start, expressionEnd(body, start));
  });
}

// Every parameter list in a file, with the function body it binds. A plain
// parameter, as in (target, options) => ..., binds every use inside its
// function to the value a caller passes, or to its default. A destructured
// parameter usually names a fixture value bound elsewhere, so it binds
// nothing here.
function parameterLists(masked) {
  const lists = [];
  const parameters = new RegExp(String.raw`(?<![\w$.])(?:function\s*\*?\s*(?:${IDENTIFIER})?\s*\(([^()]*)\)|\(([^()]*)\)\s*=>|(${IDENTIFIER})\s*=>)`, "g");
  for (const match of masked.matchAll(parameters)) {
    const end = match.index + match[0].length;
    let scope = null;
    if (match[1] !== undefined) {
      const brace = masked.indexOf("{", end);
      const close = brace < 0 ? -1 : closing(masked, brace);
      if (close >= 0) scope = [brace, close];
    } else {
      let start = end;
      while (start < masked.length && /\s/.test(masked[start])) start += 1;
      const close = masked[start] === "{" ? closing(masked, start) : expressionEnd(masked, start);
      if (close >= 0) scope = [start, close];
    }
    lists.push({ at: match.index, end, entries: (match[1] ?? match[2] ?? match[3]).split(","), scope });
  }
  return lists;
}

function assignedValues(masked, name, lists) {
  const id = escaped(name);
  const values = [];
  for (const { at, entries, scope } of lists) {
    const parameter = entries.find((entry) => entry.split("=")[0].trim() === name);
    if (parameter === undefined || !scope) continue;
    const fallback = parameter.indexOf("=");
    values.push({ at, value: fallback < 0 ? "" : parameter.slice(fallback + 1), item: false, scope });
  }
  for (const match of masked.matchAll(new RegExp(String.raw`(?<![\w$.])(?:(?:const|let|var)\s+)?${id}\s*=(?![=>])\s*`, "g"))) {
    // A default in a parameter list belongs to its parameter above.
    if (lists.some(({ at, end }) => at <= match.index && match.index < end)) continue;
    const start = match.index + match[0].length;
    values.push({ at: match.index, value: masked.slice(start, expressionEnd(masked, start)), item: false });
  }
  // for (const button of buttons) binds one item of the iterated collection.
  for (const match of masked.matchAll(new RegExp(String.raw`\bfor\s*\(\s*(?:const|let|var)\s+${id}\s+of\s+`, "g"))) {
    const open = masked.lastIndexOf("(", match.index + match[0].length);
    const close = closing(masked, open);
    if (close >= 0) values.push({ at: match.index, value: masked.slice(match.index + match[0].length, close), item: true });
  }
  return values;
}

// The bracket pairs written at the top level of a file: one per test call,
// top-level function body or object literal.
function topLevelBlocks(masked) {
  const blocks = [];
  let depth = 0;
  let open = -1;
  for (let index = 0; index < masked.length; index += 1) {
    const character = masked[index];
    if (PAIRS[character]) {
      if (depth === 0) open = index;
      depth += 1;
    } else if (REVERSE[character] && depth > 0) {
      depth -= 1;
      if (depth === 0) blocks.push([open, index]);
    }
  }
  return blocks;
}

// Split a value at top-level ||, ?? and && and at the branches of a
// conditional, whose condition is not a result.
function alternatives(text) {
  const parts = [];
  let depth = 0;
  let start = 0;
  let split = false;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    const next = text[index + 1];
    if (PAIRS[character]) depth += 1;
    else if (REVERSE[character]) depth -= 1;
    else if (depth !== 0) continue;
    else if ((character === "|" && next === "|") || (character === "?" && next === "?")
        || (character === "&" && next === "&")) {
      parts.push(text.slice(start, index));
      start = index + 2;
      index += 1;
      split = true;
    } else if (character === "?" && next !== ".") {
      start = index + 1;
      split = true;
    } else if (character === ":") {
      parts.push(text.slice(start, index));
      start = index + 1;
      split = true;
    }
  }
  if (!split) return null;
  parts.push(text.slice(start));
  return parts;
}

function unwrap(text) {
  let value = text.trim();
  for (;;) {
    const prefix = value.match(/^(?:await|void|typeof)\s+/);
    if (prefix && prefix[0].startsWith("await")) value = value.slice(prefix[0].length).trim();
    else if (value.startsWith("(") && closing(value, 0) === value.length - 1) value = value.slice(1, -1).trim();
    else return value;
  }
}

// A unary operator, or a comparison, arithmetic, in or instanceof outside
// brackets, yields a boolean or a number, never a node.
function plainValue(text) {
  if (/^[!~+-]/.test(text) || /^(?:typeof|void|delete|new)\b/.test(text)) return true;
  let depth = 0;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    const next = text[index + 1];
    if (PAIRS[character]) depth += 1;
    else if (REVERSE[character]) depth -= 1;
    else if (depth !== 0) continue;
    else if (character === "=" && next === ">") index += 1;
    else if ((character === "=" || character === "!") && next === "=") return true;
    else if (character === "<" || character === ">" || "+-*/%".includes(character)) return true;
    else if (/\s/.test(character) && /^\s+(?:in|instanceof)\s/.test(text.slice(index))) return true;
  }
  return false;
}

// Text, a number or a flag: a literal, an operator result, a value property
// such as .value or .textContent, or a call such as .trim() or String(x).
function primitive(text) {
  const value = unwrap(text);
  if (LITERAL.test(value) || plainValue(value)) return true;
  if (value.endsWith(")")) {
    const open = opening(value, value.length - 1);
    if (open <= 0) return false;
    const callee = value.slice(0, open).replace(/\?\.\s*$/, "").trimEnd();
    const member = callee.match(MEMBER);
    return member ? PRIMITIVE_METHODS.has(member[1]) : PRIMITIVE_FUNCTIONS.has(callee);
  }
  const property = value.match(MEMBER);
  return !!property && VALUE_PROPERTIES.has(property[1]);
}

class Analysis {
  constructor(masked) {
    this.masked = masked;
    this.cache = new Map();
    this.blocks = topLevelBlocks(masked);
  }

  // The top-level block that holds an index, or -1 at the top level itself.
  block(at) {
    let low = 0;
    let high = this.blocks.length - 1;
    while (low <= high) {
      const middle = (low + high) >> 1;
      const [open, close] = this.blocks[middle];
      if (at < open) high = middle - 1;
      else if (at > close) low = middle + 1;
      else return middle;
    }
    return -1;
  }

  // Tests reuse names for different values. A use reads the bindings written
  // in its own top-level block, such as its own test; a name bound only
  // elsewhere, such as a fixture parameter, reads every binding in the file.
  near(rows, at) {
    const block = this.block(at);
    if (block < 0) return rows;
    const inside = rows.filter((row) => this.block(row.at) === block);
    return inside.length ? inside : rows;
  }

  cached(key, at, depth, compute) {
    const scoped = `${key}@${this.block(at)}`;
    if (this.cache.has(scoped)) return this.cache.get(scoped);
    if (depth >= MAX_HELPER_DEPTH) return null;
    this.cache.set(scoped, null);
    let kind = null;
    for (const found of compute()) {
      if (found === NODE) kind = NODE;
      else if (found === COLLECTION && kind === null) kind = COLLECTION;
    }
    this.cache.set(scoped, kind);
    return kind;
  }

  // Reads of the whole file, once per name.
  scan(key, read) {
    if (!this.cache.has(key)) this.cache.set(key, read());
    return this.cache.get(key);
  }

  helper(name, member, depth, at) {
    const bodies = this.scan((member ? "definitions:member:" : "definitions:call:") + name, () => {
      const found = definitions(this.masked, name, member);
      // A fixture often hands its helpers over by shorthand, as { button },
      // so ui.button(label) also reads a same-file function button.
      if (member && this.keys(name).some((row) => row.value === null)) found.push(...definitions(this.masked, name, false));
      return found;
    });
    return this.cached((member ? "member:" : "call:") + name, at, depth, () =>
      this.near(bodies, at).flatMap(({ at: written, body }) =>
        returnedValues(body).map((value) => this.kind(value, depth + 1, written))));
  }

  variable(name, depth, at) {
    const lists = this.scan("parameters", () => parameterLists(this.masked));
    const rows = this.scan("assignments:" + name, () => assignedValues(this.masked, name, lists));
    // The innermost parameter of that name binds a use inside its function.
    const own = rows.filter((row) => row.scope && row.scope[0] <= at && at <= row.scope[1])
      .sort((left, right) => (left.scope[1] - left.scope[0]) - (right.scope[1] - right.scope[0]))[0];
    if (own) return depth >= MAX_HELPER_DEPTH ? null : this.kind(own.value, depth + 1, own.at);
    return this.cached("value:" + name, at, depth, () =>
      this.near(rows.filter((row) => !row.scope), at).map(({ at: written, value, item }) => {
        const found = this.kind(value, depth + 1, written);
        return item ? (found === COLLECTION ? NODE : null) : found;
      }));
  }

  // The object literal keys of a name in this file: a shorthand { open } has
  // value null; { open: value } keeps its value text. A function value is not
  // a fixture value.
  keys(name) {
    return this.scan("keys:" + name, () => {
      const id = escaped(name);
      const found = [];
      for (const match of this.masked.matchAll(new RegExp(String.raw`(?<=[{,]\s*)${id}(?=\s*[,}])`, "g"))) {
        found.push({ at: match.index, value: null });
      }
      const value = new RegExp(String.raw`(?<=[{,]\s*)${id}\s*:\s*(?!(?:async\s+)?(?:\([^()]*\)|${IDENTIFIER})\s*=>|(?:async\s+)?function\b)`, "g");
      for (const match of this.masked.matchAll(value)) {
        const start = match.index + match[0].length;
        found.push({ at: match.index, value: this.masked.slice(start, expressionEnd(this.masked, start)) });
      }
      return found;
    });
  }

  // A fixture property: an object literal in this file sets it, as in
  // { open } or { open: doc.querySelector(selector) }.
  property(name, depth, at) {
    return this.cached("property:" + name, at, depth, () => this.near(this.keys(name), at).map((row) =>
      row.value === null ? this.variable(name, depth + 1, row.at) : this.kind(row.value, depth + 1, row.at)));
  }

  // NODE when a value can be a DOM node, COLLECTION when it is a list of them.
  // at is where the value is written; it selects the bindings its names read.
  kind(text, depth = 0, at = -1) {
    const value = unwrap(text);
    if (!value || LITERAL.test(value) || KEYWORDS.has(value) || FUNCTION_VALUE.test(value)) return null;
    const parts = alternatives(value);
    if (parts) {
      const kinds = parts.map((part) => this.kind(part, depth, at));
      return kinds.includes(NODE) ? NODE : kinds.includes(COLLECTION) ? COLLECTION : null;
    }
    if (plainValue(value)) return null;
    if (value.endsWith("]")) {
      const open = opening(value, value.length - 1);
      if (open === 0) {
        const spread = value.slice(1, -1).trim().match(/^\.\.\.([^]*)$/);
        return spread && this.kind(spread[1], depth, at) === COLLECTION ? COLLECTION : null;
      }
      return open > 0 && this.kind(value.slice(0, open), depth, at) === COLLECTION ? NODE : null;
    }
    if (value.endsWith(")")) {
      const open = opening(value, value.length - 1);
      if (open <= 0) return null;
      const callee = value.slice(0, open).replace(/\?\.\s*$/, "").trimEnd();
      const member = callee.match(MEMBER);
      if (member) {
        const name = member[1];
        const receiver = callee.slice(0, member.index);
        if (NODE_METHODS.has(name)) return NODE;
        if (COLLECTION_METHODS.has(name)) return COLLECTION;
        if (name === "from" && receiver.trim() === "Array") {
          const first = argumentSpans(value, open, value.length - 1)[0];
          return first && this.kind(value.slice(...first), depth, at) === COLLECTION ? COLLECTION : null;
        }
        if (PICK_METHODS.has(name)) return this.kind(receiver, depth, at) === COLLECTION ? NODE : null;
        if (SUBSET_METHODS.has(name)) return this.kind(receiver, depth, at) === COLLECTION ? COLLECTION : null;
        return this.helper(name, true, depth, at);
      }
      const bare = callee.match(NAME);
      return bare ? this.helper(bare[1], false, depth, at) : null;
    }
    const property = value.match(MEMBER);
    if (property) {
      const name = property[1];
      const receiver = value.slice(0, property.index);
      if (DOCUMENT_NODES.has(name)) return DOCUMENT.test(receiver.trim()) ? NODE : null;
      if (NODE_PROPERTIES.has(name)) return NODE;
      if (COLLECTION_PROPERTIES.has(name)) return COLLECTION;
      // A node's own .value or .textContent is text. On another receiver the
      // same name can be a fixture's node, as f.open is.
      if (VALUE_PROPERTIES.has(name) && this.kind(receiver, depth, at) !== null) return null;
      return this.property(name, depth, at);
    }
    const identifier = value.match(NAME);
    return identifier ? this.variable(identifier[1], depth, at) : null;
  }
}

// Every equality-family assertion in this source, with the kind of each of
// its two compared operands: "node", "collection" or null. A node or list is
// read directly, or through a same-file helper, variable or fixture property.
export function equalityAssertions(source) {
  const masked = maskSource(source);
  const analysis = new Analysis(masked);
  const findings = [];
  for (const match of masked.matchAll(EQUALITY_CALL)) {
    const open = match.index + match[0].length - 1;
    const close = closing(masked, open);
    if (close < 0) continue;
    const spans = argumentSpans(masked, open, close);
    if (spans.length < 2) continue;
    const operands = [masked.slice(...spans[0]), masked.slice(...spans[1])];
    const kinds = operands.map((operand, index) => analysis.kind(operand, 0, spans[index][0]));
    // A fixture can hold a node under a name a node also uses for text or a
    // flag, such as f.open, so an operand read as a node is never plain.
    const plain = operands.map((operand, index) => kinds[index] === null && primitive(operand));
    findings.push({
      line: source.slice(0, match.index).split("\n").length,
      method: match[1],
      actual: spans[0],
      expected: spans[1],
      kinds,
      // A node or list meets null, undefined, another node or any value that
      // is not provably text, a number or a flag. A node compared with text, a
      // number or a flag is a misread operand, and that row keeps its message.
      dom: kinds.some((kind, index) => kind !== null && !plain[1 - index]),
      text: source.slice(match.index, close + 1),
    });
  }
  return findings;
}

export function domAssertions(source) {
  return equalityAssertions(source).filter((finding) => finding.dom);
}
