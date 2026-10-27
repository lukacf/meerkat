import { Lexer } from "./vendor/marked/marked.esm.js";
import { decodeHTMLStrict } from "./vendor/entities/decode.js";
/** Only an explicit absolute web link can leave this view. Rendering never
 * fetches images, resolves local paths, or grants access to a host resource.
 */
function webLink(href) {
    if (/[\u0000-\u0020\u007f\\]/u.test(href))
        return;
    try {
        const url = new URL(href);
        if ((url.protocol === "https:" || url.protocol === "http:") && !url.username && !url.password)
            return url.href;
    }
    catch { /* Unusable destinations remain ordinary text. */ }
}
/** Render the pinned lexer's tokens with DOM construction only. Raw HTML is
 * visible inert text; code is verbatim; ordinary Markdown entities are decoded
 * as text, never parsed as HTML. No shared Marked extensions are installed.
 */
export function renderSetupMarkdown(container, source) {
    const doc = container.ownerDocument;
    let remaining = 10_000;
    function render(tokens, parent, depth = 0) {
        if (depth > 64)
            throw new RangeError("Markdown nesting");
        for (const value of tokens) {
            if (--remaining < 0)
                throw new RangeError("Markdown nodes");
            // This fresh built-in Lexer has no custom token extensions.
            const token = value;
            const text = (content) => parent.append(doc.createTextNode(content));
            function element(tag, children) {
                const node = doc.createElement(tag);
                if (children)
                    render(children, node, depth + 1);
                parent.append(node);
                return node;
            }
            switch (token.type) {
                case "space":
                case "def": break;
                case "paragraph":
                    element("p", token.tokens);
                    break;
                case "heading":
                    element(`h${Math.max(3, token.depth)}`, token.tokens);
                    break;
                case "strong":
                    element("strong", token.tokens);
                    break;
                case "em":
                    element("em", token.tokens);
                    break;
                case "del":
                    element("del", token.tokens);
                    break;
                case "blockquote":
                    element("blockquote", token.tokens);
                    break;
                case "hr":
                    element("hr");
                    break;
                case "br":
                    element("br");
                    break;
                case "code": {
                    const code = doc.createElement("code");
                    code.textContent = token.text;
                    element("pre").append(code);
                    break;
                }
                case "codespan":
                    element("code").textContent = token.text;
                    break;
                case "escape":
                    text(token.text);
                    break;
                case "html":
                    text(token.raw);
                    break;
                case "text":
                    if (token.tokens)
                        render(token.tokens, parent, depth + 1);
                    else
                        text(token.escaped ? token.text : decodeHTMLStrict(token.text));
                    break;
                case "link": {
                    const href = webLink(token.autolink ? token.href : decodeHTMLStrict(token.href));
                    if (!href) {
                        render(token.tokens, parent, depth + 1);
                        break;
                    }
                    const a = element("a", token.tokens);
                    a.href = href;
                    a.target = "_blank";
                    a.rel = "noopener noreferrer";
                    a.referrerPolicy = "no-referrer";
                    if (token.title)
                        a.title = decodeHTMLStrict(token.title);
                    break;
                }
                case "image":
                    text(`[Image: ${decodeHTMLStrict(token.text)}]`);
                    break;
                case "list": {
                    const list = element(token.ordered ? "ol" : "ul");
                    if (token.ordered && typeof token.start === "number")
                        list.start = token.start;
                    for (const item of token.items) {
                        const li = doc.createElement("li");
                        if (item.task) {
                            const mark = doc.createElement("span");
                            mark.textContent = item.checked ? "[x] " : "[ ] ";
                            mark.setAttribute("aria-label", item.checked ? "Completed task: " : "Incomplete task: ");
                            li.append(mark);
                        }
                        render(item.tokens, li, depth + 1);
                        list.append(li);
                    }
                    break;
                }
                case "table": {
                    const table = element("table"), head = doc.createElement("thead"), body = doc.createElement("tbody");
                    function row(cells, heading) {
                        const tr = doc.createElement("tr");
                        for (const cell of cells) {
                            const td = doc.createElement(heading ? "th" : "td");
                            if (heading)
                                td.setAttribute("scope", "col");
                            render(cell.tokens, td, depth + 1);
                            tr.append(td);
                        }
                        return tr;
                    }
                    head.append(row(token.header, true));
                    for (const cells of token.rows)
                        body.append(row(cells, false));
                    table.append(head, body);
                    break;
                }
                default: text(token.raw);
            }
        }
    }
    const result = doc.createDocumentFragment();
    try {
        // Large transcripts remain complete and copyable without building an
        // unbounded rich DOM. This is a presentation fallback, not content removal.
        if (source.length > 100_000)
            throw new RangeError("Markdown size");
        render(new Lexer({ gfm: true, breaks: false }).lex(source), result);
        container.replaceChildren(result);
    }
    catch {
        const plain = doc.createElement("pre");
        plain.textContent = source;
        container.replaceChildren(plain);
    }
}
