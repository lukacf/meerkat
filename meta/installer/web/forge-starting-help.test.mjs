import test from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

const webBase = new URL("./", import.meta.url);
const require = createRequire(new URL("package.json", webBase));
const React = require("react"), { act } = React;
const { build } = require("esbuild");
const { JSDOM } = createRequire(new URL("../../console/package.json", webBase))("jsdom");
const built = await build({
  entryPoints: [fileURLToPath(new URL("forge-starting-help.jsx", webBase))],
  bundle: true, write: false, platform: "node", format: "esm", logLevel: "silent",
  loader: { ".css": "empty" },
  plugins: [{ name: "shared-real-react", setup(bundler) {
    bundler.onResolve({ filter: /^react(?:\/.*)?$/ }, args => ({ path: pathToFileURL(require.resolve(args.path)).href, external: true }));
  } }],
});
const { ForgeStartingHelp, ForgeStartingContext } = await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].contents).toString("base64")}`);

async function mounted(initial, verify, { withComposer = false, withStartingContext = false } = {}) {
  const dom = new JSDOM('<form id="host"><div id="root"></div></form>', { url: "http://offline.test/" });
  const composer = withComposer ? dom.window.document.createElement("textarea") : null;
  if (composer) {
    composer.id = "existing-composer";
    composer.setAttribute("aria-label", "Describe your idea");
    dom.window.document.querySelector("form").append(composer);
  }
  const effects = [], choices = [], contexts = [], contextChoices = [], props = (Array.isArray(initial) ? initial : [initial]).map((item, index) => ({
    draft: "", disabled: false, onChoose: (text, context) => {
      choices.push({ index, text }); contexts.push(context);
      if (composer) {
        props[index] = { ...props[index], draft: text, ...(props[index].appliedContext !== undefined ? { appliedContext: context } : {}) };
        composer.value = text;
        renderComponents();
        composer.focus();
      }
    }, onChooseContext: context => {
      contextChoices.push({ index, context });
      props[index] = { ...props[index], appliedContext: context, editRequest: null };
      renderComponents();
    }, ...item,
  }));
  const replacements = { window: dom.window, document: dom.window.document, navigator: dom.window.navigator,
    HTMLElement: dom.window.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true,
    fetch: () => { effects.push("fetch"); throw new Error("Draft help cannot start network work."); },
    EventSource: class { constructor() { effects.push("EventSource"); throw new Error("Draft help cannot observe runtime work."); } },
    WebSocket: class { constructor() { effects.push("WebSocket"); throw new Error("Draft help cannot create runtime work."); } },
  };
  const names = [...Object.keys(replacements), "localStorage", "sessionStorage"];
  const before = new Map(names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]));
  for (const [name, value] of Object.entries(replacements)) Object.defineProperty(globalThis, name, { configurable: true, value });
  for (const name of ["localStorage", "sessionStorage"]) Object.defineProperty(globalThis, name, { configurable: true, get() { effects.push(name); throw new Error("Draft help is presentation only."); } });
  const { createRoot } = require("react-dom/client");
  const root = createRoot(dom.window.document.getElementById("root"));
  const renderComponents = () => root.render(React.createElement(React.Fragment, null,
    ...props.flatMap((item, key) => [React.createElement(ForgeStartingHelp, { ...item, key }),
      ...(withStartingContext ? [React.createElement(ForgeStartingContext, { key: `context-${key}-${item.contextTestKey || 0}`,
        context: item.appliedContext, composerId: item.composerId, disabled: item.disabled,
        onEdit: (step, trigger) => {
          props[key] = { ...props[key], editRequest: { step, trigger, sequence: (props[key].editRequest?.sequence || 0) + 1 } };
          renderComponents();
        },
      })] : [])])));
  let submissions = 0;
  dom.window.document.querySelector("form").addEventListener("submit", event => { event.preventDefault(); submissions += 1; });
  const sections = () => [...dom.window.document.querySelectorAll(".forge-starting-help")];
  const button = (label, index = 0) => {
    const found = [...sections()[index].querySelectorAll("button")].find(node => (node.getAttribute("aria-label") || node.querySelector("strong")?.textContent || node.textContent) === label && !node.closest("[hidden]"));
    assert.ok(found, `Missing action: ${label}`); return found;
  };
  const render = async (changes = {}, index = 0) => {
    props[index] = { ...props[index], ...changes };
    if (composer) composer.value = props[index].draft;
    await act(async () => renderComponents());
  };
  const click = async (label, index = 0) => act(async () => button(label, index).click());
  try {
    await render(); await verify({ document: dom.window.document, sections, button, click, render, choices, contexts, contextChoices, effects, composer, submissions: () => submissions });
    assert.deepEqual(effects, [], "Presentation actions must not use network, runtime or browser storage.");
    assert.equal(submissions, 0, "No helper button may submit its containing form.");
  } finally {
    await act(async () => root.unmount()); dom.window.close();
    for (const [name, descriptor] of before) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor); else delete globalThis[name];
    }
  }
}

test("optional help starts collapsed and shows one audience question with native disclosure and focus", async () => {
  await mounted({}, async ({ document, sections, button, click, choices }) => {
    const toggle = button("Help me get started");
    assert.equal(toggle.type, "button"); assert.equal(toggle.getAttribute("aria-expanded"), "false");
    const content = document.getElementById(toggle.getAttribute("aria-controls"));
    assert.ok(content); assert.equal(content.hidden, true);
    await click("Help me get started");
    assert.equal(toggle.getAttribute("aria-expanded"), "true"); assert.equal(content.hidden, false);
    assert.equal(content.querySelectorAll("h3").length, 1);
    const heading = content.querySelector("h3");
    assert.equal(heading.textContent, "Who would this help?"); assert.equal(document.activeElement === heading, true);
    assert.equal(heading.tabIndex, -1);
    assert.equal(content.querySelector(".forge-starting-help__progress").textContent, "Question 1 of 2");
    const group = content.querySelector('[role="group"]');
    assert.equal(document.getElementById(group.getAttribute("aria-labelledby")) === heading, true);
    assert.deepEqual([...group.querySelectorAll("button")].map(node => node.querySelector("strong").textContent), ["Just me", "My household", "A team", "Many people", "I'm still exploring"]);
    assert.equal(sections()[0].querySelector("textarea, input, [contenteditable]") === null, true);
    assert.deepEqual(choices, []);
    await click("Back to typing");
    assert.equal(content.hidden, true); assert.equal(document.activeElement === toggle, true);
  });
});

test("two answers make a short human context and change the parent only through explicit Use", async () => {
  const examples = [
    ["A team", "Keeping track of things", "For a team, I would like help organizing information and commitments."],
    ["Just me", "Figuring something out", "For myself, I would like help researching and understanding something."],
    ["My household", "Keeping track of things", "For my household, I would like help organizing information and commitments."],
    ["Many people", "Repetitive work", "For many people, I would like help with repetitive work."],
    ["Just me", "Something else", "For myself, I have something else in mind."],
    ["I'm still exploring", "Still exploring", "I am still exploring who this could help. I am still exploring what kind of help would be useful."],
  ];
  for (const [audience, kind, expected] of examples) {
    await mounted({}, async ({ document, sections, button, click, choices }) => {
      await click("Help me get started"); await click(audience);
      const content = sections()[0].querySelector(".forge-starting-help__content");
      assert.equal(content.querySelectorAll("h3").length, 1);
      assert.equal(content.querySelector("h3").textContent, "What kind of work would you like help with?");
      assert.equal(document.activeElement === content.querySelector("h3"), true);
      assert.equal(content.querySelector(".forge-starting-help__progress").textContent, "Question 2 of 2");
      assert.match(content.querySelector(".forge-starting-help__answer").textContent, new RegExp(audience));
      assert.equal(content.querySelector('[role="group"]').querySelectorAll("button").length, 5);
      assert.deepEqual(choices, [], "Audience selection must not fill or send the idea.");
      await click(kind);
      const preview = content.querySelector(".forge-starting-help__suggestion").textContent;
      assert.equal(preview, expected);
      assert.equal(content.querySelector('[role="group"]') === null, true, "Only the context review remains after both questions.");
      assert.equal(document.activeElement === content.querySelector("h3"), true);
      assert.doesNotMatch(preview, /please|ask me|agents?|provider|tools?|topology|api|deployment/i);
      assert.equal(sections()[0].querySelector("textarea, input, [contenteditable]") === null, true);
      assert.deepEqual(choices, [], "Neither answer may call the parent draft action.");
      await click("Use this starting point");
      assert.deepEqual(choices, [{ index: 0, text: expected }]);
      assert.equal(button("Help me get started").getAttribute("aria-expanded"), "false");
    });
  }
});

test("Back retains both answers and lets a changed audience and kind revise the final context", async () => {
  await mounted({}, async ({ document, sections, button, click, choices }) => {
    await click("Help me get started"); await click("A team"); await click("Figuring something out");
    const earlier = sections()[0].querySelector(".forge-starting-help__suggestion").textContent;
    await click("Back to questions");
    assert.equal(button("Figuring something out").getAttribute("aria-pressed"), "true");
    await click("Back");
    assert.equal(button("A team").getAttribute("aria-pressed"), "true");
    assert.equal(document.activeElement === sections()[0].querySelector("h3"), true);
    await click("Many people");
    assert.equal(button("Figuring something out").getAttribute("aria-pressed"), "true");
    await click("Keeping track of things");
    const preview = sections()[0].querySelector(".forge-starting-help__suggestion").textContent;
    assert.notEqual(preview, earlier);
    assert.equal(preview, "For many people, I would like help organizing information and commitments.");
    assert.deepEqual(choices, [], "Back and revised answers are local presentation only.");
    await click("Use this starting point");
    assert.deepEqual(choices, [{ index: 0, text: preview }]);
  });
});

test("Add preserves the exact current draft prefix and uses the revised selections", async () => {
  for (const draft of ["  My existing idea\r\n<script>must remain text</script>\n\t  ", "\n\t  "]) {
    await mounted({ draft: "Earlier idea" }, async ({ document, sections, button, click, render, choices }) => {
      await click("Help me get started"); await click("Just me"); await click("Repetitive work");
      const add = button("Add to my idea");
      await render({ draft });
      assert.equal(button("Add to my idea") === add, true, "Draft edits keep the current action node.");
      assert.deepEqual(choices, []); assert.equal(document.querySelector("script") === null, true);
      await click("Back to questions"); await click("Back"); await click("A team"); await click("Figuring something out");
      const preview = sections()[0].querySelector(".forge-starting-help__suggestion").textContent;
      await click("Add to my idea");
      assert.deepEqual(choices, [{ index: 0, text: draft + "\n\n" + preview }]);
      assert.equal(choices[0].text.slice(0, draft.length), draft);
      assert.equal(document.activeElement === button("Help me get started"), true);
    });
  }
});

test("a draft arriving or clearing during review changes Use versus Add without overwriting parent input", async () => {
  await mounted({}, async ({ sections, button, click, render, choices }) => {
    await click("Help me get started"); await click("A team"); await click("Something else");
    const action = button("Use this starting point");
    const preview = sections()[0].querySelector(".forge-starting-help__suggestion").textContent;
    await render({ draft: "Later deliberate writing  " });
    assert.equal(button("Add to my idea") === action, true);
    assert.match(sections()[0].textContent, /Your existing text is kept/);
    await click("Keep my idea"); assert.deepEqual(choices, []);
    await click("Help me get started"); await render({ draft: "" });
    assert.equal(button("Use this starting point") === action, true);
    await click("Use this starting point");
    assert.deepEqual(choices, [{ index: 0, text: preview }]);
  });
});

test("busy changes block disclosure, both question stages, navigation and final actions without losing answers", async () => {
  await mounted({ draft: "My idea  " }, async ({ sections, button, click, render, choices }) => {
    await render({ disabled: true }); await click("Help me get started");
    assert.equal(button("Help me get started").getAttribute("aria-expanded"), "false");
    await render({ disabled: false }); await click("Help me get started");
    await render({ disabled: true });
    assert.ok([...sections()[0].querySelectorAll("button")].every(node => node.disabled));
    await click("A team"); await click("Back to typing");
    assert.equal(sections()[0].querySelector("h3").textContent, "Who would this help?");
    await render({ disabled: false }); await click("A team"); await render({ disabled: true });
    await click("Figuring something out"); await click("Back"); await click("Back to typing"); await click("Help me get started");
    assert.equal(sections()[0].querySelector("h3").textContent, "What kind of work would you like help with?");
    assert.equal(button("Help me get started").getAttribute("aria-expanded"), "true");
    await render({ disabled: false }); await click("Figuring something out");
    const preview = sections()[0].querySelector(".forge-starting-help__suggestion").textContent;
    await render({ disabled: true });
    assert.ok([...sections()[0].querySelectorAll("button")].every(node => node.disabled));
    await click("Add to my idea"); await click("Keep my idea"); await click("Back to questions");
    assert.deepEqual(choices, []);
    assert.equal(sections()[0].querySelector(".forge-starting-help__suggestion").textContent, preview);
    await render({ disabled: false }); await click("Add to my idea");
    assert.deepEqual(choices, [{ index: 0, text: "My idea  \n\n" + preview }]);
  });
});

test("a missing parent action disables Use or Add while returning to typing remains available", async () => {
  for (const draft of ["", "Keep this idea exactly.  "]) {
    for (const onChoose of [undefined, null, "not a callback"]) {
      await mounted({ draft, onChoose }, async ({ document, button, click, choices }) => {
        await click("Help me get started"); await click("Just me"); await click("Still exploring");
        const label = draft ? "Add to my idea" : "Use this starting point";
        assert.equal(button(label).disabled, true); await click(label);
        assert.deepEqual(choices, []);
        await click(draft ? "Keep my idea" : "Back to typing");
        assert.equal(button("Help me get started").getAttribute("aria-expanded"), "false");
        assert.equal(document.activeElement === button("Help me get started"), true);
      });
    }
  }
});

test("the optional typing callback is navigation only and is blocked during busy", async () => {
  let returns = 0;
  await mounted({ draft: "My idea", onReturnToTyping: () => { returns += 1; } }, async ({ button, click, render, choices }) => {
    await click("Help me get started"); await click("Back to typing");
    assert.equal(returns, 1); assert.deepEqual(choices, []);
    await click("Help me get started"); await click("Just me"); await click("Something else");
    await render({ disabled: true }); await click("Keep my idea");
    assert.equal(returns, 1);
    await render({ disabled: false }); await click("Keep my idea");
    assert.equal(returns, 2); assert.deepEqual(choices, []);
    assert.equal(button("Help me get started").getAttribute("aria-expanded"), "false");
  });
});

test("independent instances isolate question state, draft actions and all disclosure label IDs", async () => {
  await mounted([{}, { draft: "Second idea" }], async ({ document, sections, button, click, choices }) => {
    const a = button("Help me get started", 0), b = button("Help me get started", 1);
    assert.notEqual(a.getAttribute("aria-controls"), b.getAttribute("aria-controls"));
    await click("Help me get started", 0); await click("Just me", 0);
    assert.equal(b.getAttribute("aria-expanded"), "false");
    await click("Help me get started", 1);
    const groupA = sections()[0].querySelector('[role="group"]'), groupB = sections()[1].querySelector('[role="group"]');
    assert.notEqual(groupA.getAttribute("aria-labelledby"), groupB.getAttribute("aria-labelledby"));
    assert.equal(document.getElementById(groupA.getAttribute("aria-labelledby")).textContent, "What kind of work would you like help with?");
    assert.equal(document.getElementById(groupB.getAttribute("aria-labelledby")).textContent, "Who would this help?");
    await click("Many people", 1); await click("Repetitive work", 1);
    await click("Add to my idea", 1);
    assert.deepEqual(choices, [{ index: 1, text: "Second idea\n\nFor many people, I would like help with repetitive work." }]);
    assert.equal(a.getAttribute("aria-expanded"), "true");
    assert.equal(sections()[0].querySelector("h3").textContent, "What kind of work would you like help with?");
    assert.ok([...document.querySelectorAll("button")].every(node => node.type === "button"));
  });
});

test("step navigation focuses and reveals the visible heading while draft and busy updates preserve position", async () => {
  await mounted({}, async ({ document, sections, button, click, render, choices }) => {
    const scrolls = [];
    document.defaultView.HTMLElement.prototype.scrollIntoView = function (options) {
      scrolls.push({ node: this, options, focus: document.activeElement });
    };
    await click("Help me get started"); await click("A team"); await click("Figuring something out");
    assert.equal(scrolls.length, 3);
    for (const item of scrolls) {
      assert.equal(item.node === item.focus, true); assert.equal(item.node.tagName, "H3");
      assert.deepEqual(item.options, { block: "nearest", behavior: "auto" });
      assert.equal(item.node.closest("[hidden]") === null, true);
    }
    const back = button("Back to questions"); back.focus();
    await render({ draft: "Later deliberate writing" }); await render({ disabled: true }); await render({ disabled: false });
    assert.equal(scrolls.length, 3); assert.equal(document.activeElement === back, true);
    await click("Back to questions"); await click("Back");
    assert.equal(scrolls.length, 5); assert.equal(document.activeElement === sections()[0].querySelector("h3"), true);
    await click("Back to typing");
    assert.equal(document.activeElement === button("Help me get started"), true); assert.deepEqual(choices, []);
  });
});


// HTML textareas normalize line endings; exact draft bytes are asserted on onChoose.
const editorText = text => text.replace(/\r\n?/g, "\n");

function visibleTaskPrompt(section, prompt) {
  return [...section.querySelectorAll("h3, p, label")].find(node =>
    node.textContent === prompt && !node.closest("[hidden]") && !node.closest('[aria-hidden="true"]')) || null;
}

test("one optional disclosure leaves the composer primary and preserves the written idea through help", async () => {
  const draft = "  My own rough idea.\nKeep every character.  ";
  let returns = 0;
  await mounted({ inlineStart: true, draft, onReturnToTyping: () => {
    returns += 1; document.querySelector("#existing-composer").focus();
  } }, async ({ document, sections, button, click, render, choices, contexts, composer }) => {
    const invitation = sections()[0].querySelector(".forge-starting-help__invitation");
    const help = button("Help me get started");
    assert.equal(invitation.dataset.entryOptions, "true");
    assert.equal(invitation.querySelectorAll("button").length, 1);
    assert.equal(help.type, "button");
    assert.equal(help.hasAttribute("aria-pressed"), false, "Optional help is a disclosure, not an entry mode.");
    assert.ok(document.getElementById(help.getAttribute("aria-controls")));
    assert.equal(document.querySelectorAll("textarea, input, [contenteditable]").length, 1);
    composer.focus();
    assert.equal(document.activeElement === composer, true);
    assert.equal(composer.value, draft); assert.equal(returns, 0);
    assert.equal(help.getAttribute("aria-expanded"), "false");
    assert.deepEqual(choices, []); assert.deepEqual(contexts, []);

    await render({ disabled: true }); await click("Help me get started");
    assert.equal(returns, 0); assert.equal(composer.value, draft);
    assert.equal(help.getAttribute("aria-expanded"), "false");
    await render({ disabled: false }); await click("Help me get started");
    assert.equal(help.getAttribute("aria-expanded"), "true");
    assert.equal(composer.value, draft);
    assert.equal(document.activeElement.textContent, "Who would this help?");
    assert.equal(invitation.querySelectorAll("button").length, 1, "The question has one way back to writing, with no competing entry tabs.");
    assert.equal(button("Write it myself") === help, true, "The original disclosure retains ownership and its accessible controls.");
    await click("A team");
    assert.equal(document.activeElement.textContent, "What kind of work would you like help with?");
    await click("Back");
    assert.equal(document.activeElement.textContent, "Who would this help?");
    assert.equal(button("A team").getAttribute("aria-pressed"), "true");
    assert.equal(composer.value, draft); assert.deepEqual(choices, []);
    await click("Write it myself");
    assert.equal(document.activeElement === composer, true); assert.equal(returns, 1);
    assert.equal(composer.value, draft);
    assert.equal(help.getAttribute("aria-expanded"), "false");
    assert.deepEqual(choices, []); assert.deepEqual(contexts, []);
  }, { withComposer: true });
});

test("the second inline choice hands the exact draft to the sole composer with a visible task prompt through editing and busy", async () => {
  const context = "For a team, I would like help organizing information and commitments.";
  const prompt = "What's one thing you need to keep track of?";
  const example = "Keep track of decisions and deadlines from our weekly meeting.";
  for (const prefix of ["", "  Keep my exact idea\r\n<script>remain literal text</script>\n\t  "]) {
    await mounted({ inlineStart: true, composerId: "existing-composer", draft: prefix }, async ({ document, sections, button, click, render, choices, composer }) => {
      const section = sections()[0], toggle = button("Help me get started");
      assert.equal(visibleTaskPrompt(section, prompt) === null, true);
      await click("Help me get started"); await click("A team");
      assert.deepEqual(choices, [], "The audience choice must not update or send the parent draft.");
      assert.equal(composer.value, editorText(prefix));
      await click("Keeping track of things");
      const inserted = (prefix ? prefix + "\n\n" : "") + context;
      assert.deepEqual(choices, [{ index: 0, text: inserted }], "The second choice adds only editable broad context, without a third confirmation step.");
      assert.equal(composer.value, editorText(inserted));
      assert.equal(document.activeElement === composer, true, "The existing onChoose action returns focus to the original editor.");
      assert.equal(button("Change choices") === toggle, true, "The original disclosure becomes the context revision action.");
      assert.equal(toggle.getAttribute("aria-expanded"), "false");
      const hint = visibleTaskPrompt(section, prompt);
      assert.ok(hint, "Closing the questions must keep the task prompt visible beside the draft.");
      assert.equal(hint.tagName, "LABEL");
      assert.equal(hint.htmlFor, composer.id, "The task prompt labels the sole existing composer.");
      assert.equal(section.querySelector(".forge-starting-help__task-marker") === null, true);
      assert.equal(section.querySelector(".forge-starting-help__summary").textContent, "A team - Keeping track of things");
      assert.equal(section.querySelector(".forge-starting-help__task-example").textContent, "For example: " + example + " Add one task, or send this draft and your guide will help you find one.");
      assert.match(section.textContent, /Add one task, or send this draft and your guide will help you find one\./);
      assert.ok(hint.compareDocumentPosition(composer) & 4, "The handoff prompt belongs above the existing composer.");
      assert.equal(document.querySelectorAll("textarea, input, [contenteditable]").length, 1);
      assert.equal(section.querySelector("textarea, input, [contenteditable]") === null, true);
      assert.equal(document.querySelector("script") === null, true);
      const edited = inserted + "\n\nAn actual task with its own wording.  \r\n";
      await render({ draft: edited }); await render({ disabled: true });
      assert.equal(composer.value, editorText(edited));
      assert.equal(document.activeElement === composer, true, "Editing or becoming busy must not move the user's typing position.");
      assert.ok(visibleTaskPrompt(section, prompt), "The applied prompt must survive draft and busy rerenders.");
      assert.equal(section.querySelector(".forge-starting-help__task-example").textContent, "For example: " + example + " Add one task, or send this draft and your guide will help you find one.");
      await click("Change choices");
      assert.equal(toggle.getAttribute("aria-expanded"), "false");
      assert.equal(choices.length, 1);
      await render({ disabled: false }); await render({ draft: "" });
      assert.equal(visibleTaskPrompt(section, prompt) === null, true, "An emptied draft clears the local handoff.");
      assert.equal(section.querySelector(".forge-starting-help__task-example") === null, true);
      assert.equal(composer.value, "");
      assert.equal(choices.length, 1, "Editing and clearing do not invoke the apply action.");
    }, { withComposer: true });
  }
});

test("Change choices is navigation and the second revised choice updates only untouched inserted context, preserving all suffix bytes", async () => {
  const prefix = "  Original user wording\r\n\t  ";
  const first = "For a team, I would like help organizing information and commitments.";
  const revised = "For many people, I would like help with repetitive work.";
  const suffix = "\n\nMy actual task: compare records <script>as text</script>.  \r\n\t";
  await mounted({ inlineStart: true, draft: prefix }, async ({ document, sections, button, click, render, choices, composer }) => {
    const section = sections()[0], toggle = button("Help me get started");
    await click("Help me get started"); await click("A team"); await click("Keeping track of things");
    assert.equal(visibleTaskPrompt(section, "What's one thing you need to keep track of?").tagName, "P", "Without a composerId, the prompt remains ordinary text.");
    assert.equal(section.querySelector(".forge-starting-help__task-example").textContent,
      "For example: Keep track of decisions and deadlines from our weekly meeting. Add one task, or send this draft and your guide will help you find one.");
    const written = prefix + "\n\n" + first + suffix;
    await render({ draft: written });
    await click("Change choices");
    assert.equal(section.querySelector("h3").textContent, "Who would this help?");
    assert.equal(toggle.getAttribute("aria-expanded"), "true");
    assert.equal(composer.value, editorText(written));
    assert.equal(section.querySelector(".forge-starting-help__task-example") === null, true, "Reopening choices does not expose a tentative example as applied context.");
    assert.equal(choices.length, 1, "Opening choice revision is navigation, not a draft action.");
    await click("Many people");
    assert.equal(choices.length, 1, "Changing audience alone must not rewrite the draft.");
    assert.equal(composer.value, editorText(written));
    await click("Repetitive work");
    const expected = prefix + "\n\n" + revised + suffix;
    assert.deepEqual(choices, [{ index: 0, text: prefix + "\n\n" + first }, { index: 0, text: expected }]);
    assert.equal(composer.value, editorText(expected));
    assert.equal(choices.at(-1).text.slice(0, prefix.length), prefix);
    assert.equal(choices.at(-1).text.slice(-suffix.length), suffix);
    assert.equal(document.activeElement === composer, true);
    assert.equal(toggle.getAttribute("aria-expanded"), "false");
    assert.ok(visibleTaskPrompt(section, "What's one task you keep doing by hand?"));
    assert.equal(section.querySelector(".forge-starting-help__task-example").textContent,
      "For example: Turn our weekly project notes into a clear update. Add one task, or send this draft and your guide will help you find one.");
    await click("Change choices"); await click("Many people"); await click("Repetitive work");
    assert.equal(composer.value, editorText(expected), "Choosing unchanged context must not append it a second time.");
    assert.equal(choices.at(-1).text, expected);
    assert.equal(section.querySelector(".forge-starting-help__task-example").textContent,
      "For example: Turn our weekly project notes into a clear update. Add one task, or send this draft and your guide will help you find one.");
    assert.equal(document.activeElement === composer, true);
    assert.equal(document.querySelectorAll("textarea, input, [contenteditable]").length, 1);
    assert.equal(document.querySelector("script") === null, true);
  }, { withComposer: true });
});

test("edited inserted wording invalidates the handoff, and a different guide mount inherits no prompt or choice state", async () => {
  const prefix = "User writing stays  \r\n";
  const context = "For a team, I would like help organizing information and commitments.";
  const prompt = "What's one thing you need to keep track of?";
  const suffix = "\n\nTask details added later.  ";
  await mounted({ inlineStart: true, draft: prefix }, async ({ document, sections, button, click, render, choices, composer }) => {
    const section = sections()[0];
    await click("Help me get started"); await click("A team"); await click("Keeping track of things");
    const changed = prefix + "\n\n" + context.replace("organizing", "sorting") + suffix;
    await render({ draft: changed });
    assert.equal(visibleTaskPrompt(section, prompt) === null, true, "The helper must not claim that edited context still matches its snapshot.");
    assert.equal(section.querySelector(".forge-starting-help__task-example") === null, true, "Editing inserted context clears its example too.");
    assert.equal(composer.value, editorText(changed));
    assert.equal(choices.length, 1, "Invalidation does not repair, replace or submit the user's writing.");
    await click("Help me get started");
    assert.equal(section.querySelector("h3").textContent, "Who would this help?");
    await click("A team");
    assert.equal(choices.length, 1);
    assert.equal(composer.value, editorText(changed));
    await click("Keeping track of things");
    assert.equal(choices.at(-1).text, changed + "\n\n" + context, "A later explicit second choice preserves all edited wording and appends new context.");
    assert.equal(composer.value, editorText(choices.at(-1).text));
    assert.equal(choices.at(-1).text.slice(0, changed.length), changed);
    assert.equal(section.querySelector(".forge-starting-help__task-example").textContent,
      "For example: Keep track of decisions and deadlines from our weekly meeting. Add one task, or send this draft and your guide will help you find one.");
    assert.equal(document.activeElement === composer, true);
  }, { withComposer: true });
  const differentGuideDraft = "An unrelated guide's saved draft.  \n";
  await mounted({ inlineStart: true, draft: differentGuideDraft }, async ({ sections, button, click, choices, composer }) => {
    const section = sections()[0];
    assert.equal(visibleTaskPrompt(section, prompt) === null, true, "A freshly mounted guide must not inherit the previous guide's handoff.");
    assert.equal(section.querySelector(".forge-starting-help__task-example") === null, true);
    assert.equal(button("Help me get started").getAttribute("aria-expanded"), "false");
    assert.equal(composer.value, editorText(differentGuideDraft));
    assert.deepEqual(choices, []);
    await click("Help me get started");
    assert.equal(section.querySelector("h3").textContent, "Who would this help?");
    assert.ok([...section.querySelectorAll(".forge-starting-help__choice")].every(node => node.getAttribute("aria-pressed") === "false"));
    assert.deepEqual(choices, [], "Opening optional help in another guide has no draft side effect.");
  }, { withComposer: true });
});


test("starting context restore: an explicit snapshot restores known choices and preserves exact CRLF writing during revision", async () => {
  const prefix = "  User writing\r\n\t  \n\n", suffix = "\n\nMy task stays exact.  \r\n\t";
  const context = "For a team, I would like help organizing information and commitments.";
  const appliedContext = { audience_id: "team", kind_id: "organizing", prefix, inserted: prefix + context };
  await mounted({ inlineStart: true, composerId: "existing-composer", draft: appliedContext.inserted + suffix, appliedContext }, async ({ sections, button, click, choices, contexts, composer }) => {
    const section = sections()[0];
    assert.equal(button("Change choices").getAttribute("aria-expanded"), "false");
    assert.equal(visibleTaskPrompt(section, "What's one thing you need to keep track of?").tagName, "LABEL");
    assert.deepEqual(choices, [], "Restoring a snapshot does not apply or submit it again.");
    await click("Change choices");
    assert.equal(button("A team").getAttribute("aria-pressed"), "true");
    await click("A team");
    assert.equal(button("Keeping track of things").getAttribute("aria-pressed"), "true");
    await click("Keeping track of things");
    assert.equal(choices.at(-1).text, appliedContext.inserted + suffix);
    await click("Change choices"); await click("Many people"); await click("Repetitive work");
    const inserted = prefix + "For many people, I would like help with repetitive work.";
    assert.equal(choices.at(-1).text, inserted + suffix);
    assert.deepEqual(contexts.at(-1), { audience_id: "many", kind_id: "repetitive", prefix, inserted });
    assert.equal(choices.at(-1).text.slice(0, prefix.length), prefix);
    assert.equal(choices.at(-1).text.slice(-suffix.length), suffix);
    assert.equal(composer.value, editorText(inserted + suffix));
  }, { withComposer: true });
  for (const invalid of [{ ...appliedContext, kind_id: "unknown" }, { ...appliedContext, inserted: prefix + "Unexpected prose" }]) {
    await mounted({ inlineStart: true, draft: appliedContext.inserted + suffix, appliedContext: invalid }, async ({ sections, button, choices }) => {
      assert.equal(sections()[0].querySelector(".forge-starting-help__handoff") === null, true, "Only exact explicit known context can restore applied choices.");
      assert.ok(button("Help me get started")); assert.deepEqual(choices, []);
    });
  }
});

const versionTwoContext = { version: 2, audience_id: "team", kind_id: "organizing" };
const contextEditButton = (document, field) => {
  const label = field === "audience" ? "Change who this helps" : "Change the kind of work";
  const result = [...document.querySelectorAll(".forge-starting-context button")]
    .find(node => node.getAttribute("aria-label")?.startsWith(label));
  assert.ok(result, `Missing starting context field: ${field}`);
  return result;
};

test("v2 answered Choose returns to committed review without restarting questions or changing writing", async () => {
  const draft = "  Keep these words\r\nexactly.  ";
  let returns = 0;
  await mounted({ separateContext: true, composerId: "existing-composer", draft, appliedContext: versionTwoContext, onReturnToTyping: () => {
    returns += 1; document.querySelector("#existing-composer").focus();
  } }, async ({ document, sections, button, click, render, composer, contextChoices, choices, effects, submissions }) => {
    const section = sections()[0], choose = button("Your two answers");
    const content = section.querySelector(".forge-starting-conversation__content"), question = content.querySelector("h3");
    const questionText = question.textContent;
    const summary = document.getElementById(choose.getAttribute("aria-controls"));
    assert.ok(summary); assert.equal(summary.tagName, "H2"); assert.equal(summary.tabIndex, -1);
    assert.equal(summary.closest("[hidden]") === null, true);
    assert.equal(summary.closest(".forge-starting-context") === section.nextElementSibling, true);
    const summaryText = summary.textContent, scrolls = [];
    assert.equal(summaryText, "For a team, help with keeping track of things.");
    summary.scrollIntoView = options => scrolls.push(options);
    assert.equal(document.activeElement !== summary, true, "Rendering committed context alone must not steal focus.");
    assert.equal(choose.getAttribute("aria-pressed"), "true");
    choose.focus(); await click("Your two answers");
    assert.equal(document.activeElement === summary, true, "The deliberate review action focuses the visible committed sentence.");
    assert.equal(choose.getAttribute("aria-expanded"), null);
    assert.equal(section.dataset.review, "true");
    await click("In my own words");
    assert.equal(button("In my own words").getAttribute("aria-pressed"), "true");
    assert.equal(document.activeElement === composer, true); assert.equal(returns, 1);
    choose.focus(); await click("Your two answers");
    assert.equal(document.activeElement === summary, true);
    assert.equal(choose.getAttribute("aria-pressed"), "true");
    assert.equal(choose.getAttribute("aria-expanded"), null);
    assert.equal(document.getElementById(choose.getAttribute("aria-controls")) === summary, true);
    assert.equal(section.dataset.open, "false"); assert.equal(content.hidden, true);
    assert.equal(content.querySelector("h3") === question, true); assert.equal(question.textContent, questionText);
    assert.equal(summary.textContent, summaryText);
    assert.deepEqual(scrolls, [{ block: "nearest", behavior: "auto" }, { block: "nearest", behavior: "auto" }]);
    await render({ disabled: true }); composer.focus(); await click("Your two answers");
    assert.equal(document.activeElement === composer, true, "Disabled review cannot steal the user's editor focus.");
    assert.equal(scrolls.length, 2); assert.equal(section.dataset.open, "false");
    assert.equal(composer.value, editorText(draft));
    assert.equal(summary.textContent, summaryText);
    assert.deepEqual(contextChoices, []); assert.deepEqual(choices, []);
    assert.deepEqual(effects, []); assert.equal(submissions(), 0);
  }, { withComposer: true, withStartingContext: true });
});

test("v2 single-field cancellation restores committed review and the owned field trigger", async () => {
  for (const field of ["audience", "help"]) for (const action of ["escape", "cancel"]) {
    const draft = "  User wording\r\n<script>still text</script>  ";
    let returns = 0;
    await mounted({ separateContext: true, draft, appliedContext: versionTwoContext,
      onReturnToTyping: () => { returns += 1; } }, async ({ document, sections, button, click, composer, contextChoices, choices }) => {
      const trigger = contextEditButton(document, field);
      trigger.focus(); await act(async () => trigger.click());
      const section = sections()[0], heading = section.querySelector("h3");
      assert.equal(document.activeElement === heading, true);
      assert.equal(heading.textContent, field === "audience" ? "Who is it for?" : "What would help most?");
      if (field === "help") {
        await click("Change");
        assert.equal(heading.textContent, "Who is it for?", "Navigation remains one staged edit until cancelled.");
      }
      if (action === "escape") {
        await act(async () => heading.dispatchEvent(new document.defaultView.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
      } else {
        const cancel = button("Cancel change"); cancel.focus(); await click("Cancel change");
      }
      assert.equal(section.dataset.open, "false"); assert.equal(section.dataset.route, "choose");
      assert.equal(button("Your two answers").getAttribute("aria-pressed"), "true");
      assert.equal(document.activeElement === trigger, true);
      assert.equal(composer.value, editorText(draft));
      assert.match(document.querySelector(".forge-starting-context").textContent, /For a team, help with keeping track of things\./);
      assert.equal(returns, 0); assert.deepEqual(contextChoices, []); assert.deepEqual(choices, []);
      assert.equal(document.querySelector("script") === null, true);
    }, { withComposer: true, withStartingContext: true });
  }
});

test("v2 explicit Write abandons only the staged edit and leaves committed context available", async () => {
  let returns = 0;
  const draft = "A preserved idea.  ";
  await mounted({ separateContext: true, draft, appliedContext: versionTwoContext, onReturnToTyping: () => {
    returns += 1; document.querySelector("#existing-composer").focus();
  } }, async ({ document, sections, click, composer, contextChoices, choices }) => {
    const trigger = contextEditButton(document, "help"); trigger.focus(); await act(async () => trigger.click());
    await click("In my own words");
    assert.equal(sections()[0].dataset.route, "write"); assert.equal(sections()[0].dataset.open, "false");
    assert.equal(document.activeElement === composer, true); assert.equal(returns, 1);
    assert.equal(composer.value, draft);
    assert.match(document.querySelector(".forge-starting-context").textContent, /For a team, help with keeping track of things\./);
    assert.deepEqual(contextChoices, []); assert.deepEqual(choices, []);
  }, { withComposer: true, withStartingContext: true });
});

test("v2 a pointer-clicked field restores its supplied trigger without requiring prior focus", async () => {
  for (const field of ["audience", "help"]) for (const action of ["escape", "cancel"]) {
    const draft = "My preserved wording.  ";
    await mounted({ separateContext: true, draft, appliedContext: versionTwoContext }, async ({ document, sections, button, click, composer, contextChoices, choices }) => {
      const trigger = contextEditButton(document, field);
      composer.focus();
      assert.equal(document.activeElement !== trigger, true);
      await act(async () => trigger.click());
      const heading = sections()[0].querySelector("h3");
      assert.equal(document.activeElement === heading, true);
      if (action === "escape") {
        await act(async () => heading.dispatchEvent(new document.defaultView.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
      } else {
        button("Cancel change").focus(); await click("Cancel change");
      }
      assert.equal(document.activeElement === trigger, true, "Cancel restores the actual clicked field, not the previously focused editor.");
      assert.equal(sections()[0].dataset.open, "false");
      assert.equal(composer.value, draft);
      assert.equal(document.querySelector(".forge-starting-context__sentence").textContent, "For a team, help with keeping track of things.");
      assert.deepEqual(contextChoices, []); assert.deepEqual(choices, []);
    }, { withComposer: true, withStartingContext: true });
  }
});

test("v2 generic question cancellation returns to writing without creating context", async () => {
  for (const action of ["escape", "cancel"]) {
    let returns = 0;
    const draft = "Existing draft  ";
    await mounted({ separateContext: true, draft, onReturnToTyping: () => {
      returns += 1; document.querySelector("#existing-composer").focus();
    } }, async ({ document, sections, button, click, composer, contextChoices }) => {
      await click("Answer two questions"); await click("A team");
      const heading = sections()[0].querySelector("h3");
      if (action === "escape") {
        await act(async () => heading.dispatchEvent(new document.defaultView.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
      } else await click("Back to my words");
      assert.equal(sections()[0].dataset.route, "write");
      assert.equal(button("Answer two questions").getAttribute("aria-expanded"), "false");
      assert.equal(document.activeElement === composer, true); assert.equal(returns, 1);
      assert.equal(composer.value, draft); assert.deepEqual(contextChoices, []);
    }, { withComposer: true });
  }
});

test("v2 a changed or removed parent context abandons its old inline edit without restoring stale focus", async () => {
  for (const context of [null, { version: 2, audience_id: "many", kind_id: "repetitive" }]) {
    await mounted({ separateContext: true, draft: "Exact draft.  ", appliedContext: versionTwoContext }, async ({ document, sections, render, composer, contextChoices }) => {
      const trigger = contextEditButton(document, "audience"); trigger.focus(); await act(async () => trigger.click());
      composer.focus();
      await render({ appliedContext: context });
      assert.equal(sections()[0].dataset.open, "false");
      assert.equal(sections()[0].dataset.route, context ? "choose" : "write");
      assert.equal(document.activeElement === composer, true);
      assert.equal(composer.value, "Exact draft.  "); assert.deepEqual(contextChoices, []);
    }, { withComposer: true, withStartingContext: true });
  }
});

test("v2 cancelling an edit does not focus a disconnected or moved field trigger", async () => {
  for (const change of ["disconnected", "moved"]) {
    await mounted({ separateContext: true, appliedContext: versionTwoContext }, async ({ document, sections, button, click, render, contextChoices }) => {
      const trigger = contextEditButton(document, "audience"); trigger.focus(); await act(async () => trigger.click());
      let external;
      if (change === "disconnected") await render({ contextTestKey: 1 });
      else {
        external = document.createElement("div");
        document.querySelector("form").append(external);
        external.append(trigger);
      }
      let staleFocus = 0;
      trigger.focus = () => { staleFocus += 1; };
      const cancel = button("Cancel change"); cancel.focus(); await click("Cancel change");
      assert.equal(sections()[0].dataset.route, "choose");
      assert.equal(staleFocus, 0); assert.deepEqual(contextChoices, []);
      if (external) document.querySelector(".forge-starting-context__sentence").prepend(trigger);
    }, { withStartingContext: true });
  }
});

test("v2 busy edit cancellation and a new single-field answer retain parent ownership", async () => {
  let returns = 0;
  await mounted({ separateContext: true, draft: "Keep me  ", appliedContext: versionTwoContext,
    onReturnToTyping: () => { returns += 1; } }, async ({ document, sections, button, click, render, contextChoices, choices }) => {
    const trigger = contextEditButton(document, "help"); trigger.focus(); await act(async () => trigger.click());
    await render({ disabled: true });
    await click("Cancel change");
    await act(async () => sections()[0].querySelector("h3").dispatchEvent(new document.defaultView.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })));
    assert.equal(sections()[0].dataset.open, "true"); assert.equal(returns, 0);
    await render({ disabled: false }); await click("Repetitive work");
    assert.deepEqual(contextChoices, [{ index: 0, context: { version: 2, audience_id: "team", kind_id: "repetitive" } }]);
    assert.deepEqual(choices, []); assert.equal(sections()[0].dataset.route, "choose");
    assert.equal(button("Your two answers").getAttribute("aria-expanded"), null);
    assert.equal(returns, 0, "Choosing an answer leaves final focus to the parent, not a typing callback.");
  }, { withStartingContext: true });
});
