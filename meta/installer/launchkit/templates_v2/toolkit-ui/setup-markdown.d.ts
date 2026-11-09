/** Render the pinned lexer's tokens with DOM construction only. Raw HTML is
 * visible inert text; code is verbatim; ordinary Markdown entities are decoded
 * as text, never parsed as HTML. No shared Marked extensions are installed.
 */
export declare function renderSetupMarkdown(container: HTMLElement, source: string): void;
