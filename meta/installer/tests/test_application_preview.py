import hashlib
from html.parser import HTMLParser

import pytest

from launchkit.application_preview import (
    ApplicationPreviewError, CSP, MAX_PREVIEW_DOCUMENT_BYTES,
    sanitize_application_preview, validate_application_preview,
)


class Elements(HTMLParser):
    def __init__(self, source):
        super().__init__()
        self.elements = []
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        self.elements.append((tag, dict(attrs)))


def test_real_view_content_and_css_survive_but_actions_are_inert():
    preview = sanitize_application_preview({
        "title": "Reading room",
        "html": '<style>.book{padding:1rem} p:after{content:"<&>"}</style>'
                '<main><h1>Your books</h1><article class="book"><h2>A Wizard of Earthsea</h2>'
                '<p>Wishlist &amp; notes</p><form action="/confirm"><input value="Sample title">'
                '<button type="submit" onclick="submit()">Confirm</button></form>'
                '<a href="/book/1">View book</a></article></main>',
    })
    document = preview["html"]
    assert "Your books" in document and "A Wizard of Earthsea" in document
    assert '.book{padding:1rem;}' in document and 'class="book"' in document
    assert "Wishlist &amp; notes" in document
    elements = Elements(document).elements
    assert not any(tag in {"form", "a", "script"} for tag, _ in elements)
    for tag, attrs in elements:
        assert not any(key.startswith("on") or key in {"href", "action", "src"} for key in attrs)
        if tag in {"input", "button"}:
            assert attrs["disabled"] == "disabled" and attrs["tabindex"] == "-1"
    assert validate_application_preview(preview) == preview
    assert preview["live_ready"] is False and len(document.encode()) <= MAX_PREVIEW_DOCUMENT_BYTES


@pytest.mark.parametrize("hidden", [
    '<input type="hidden" name="operation" value="OpaqueOperationToken" id="hidden-state">',
    '<INPUT VALUE="OpaqueOperationToken" TYPE="HiDdEn" CLASS="hidden-state">',
    "<input name='proposal' type='HIDDEN' value='OpaqueOperationToken'/>",
    '<input type="hidd&#101;n" value="OpaqueOperationToken">',
    '<input type="hidden" type="text" value="OpaqueOperationToken">',
])
def test_hidden_form_state_is_omitted_without_changing_visible_controls_or_authority(hidden):
    preview = sanitize_application_preview({"html":
        '<main><h1>Sample pending decision</h1><form action="/confirm">' + hidden +
        '<label>Title<input type="text" value="Visible sample"></label>'
        '<input type="checkbox" checked="checked"><textarea>Entered sample notes</textarea>'
        '<select><option selected="selected">Sample choice</option></select>'
        '<button onclick="confirm()">Confirm</button></form><p>Not saved.</p></main>',
    })
    document = preview["html"]
    assert "OpaqueOperationToken" not in document and "hidden-state" not in document
    assert '<h1>Sample pending decision</h1>' in document and '<p>Not saved.</p>' in document
    assert "Entered sample notes" in document and "Sample choice" in document
    elements = Elements(document).elements
    inputs = [attrs for tag, attrs in elements if tag == "input"]
    assert [attrs["type"] for attrs in inputs] == ["text", "checkbox"]
    assert inputs[0]["value"] == "Visible sample" and "checked" in inputs[1]
    assert all(attrs["disabled"] == "disabled" and attrs["tabindex"] == "-1"
               for tag, attrs in elements if tag in {"input", "textarea", "select", "option", "button"})
    assert not any(tag == "form" or "onclick" in attrs or "action" in attrs for tag, attrs in elements)
    assert preview["live_ready"] is False and preview["scope"] == "offline-application-preview"
    assert preview["document_sha256"] == hashlib.sha256(document.encode()).hexdigest()
    preview.update(trial_id="owned-trial", build_id="checked-build", source_digest="bound-source",
                   guide_version=4, observation_revision=3)
    assert validate_application_preview(preview) == preview
    with pytest.raises(ApplicationPreviewError):
        validate_application_preview({**preview, "live_ready": True})
    with pytest.raises(ApplicationPreviewError):
        validate_application_preview({**preview, "approved": True})


def test_visible_input_with_later_hidden_type_or_nonhidden_value_is_not_dropped():
    preview = sanitize_application_preview({"html":
        '<p>Visible sample fields</p><input type="text" type="hidden" value="First type wins">'
        '<input type="hidden-value" value="Ordinary fallback"><input type=" hidden " value="Invalid type fallback">',
    })
    inputs = [attrs for tag, attrs in Elements(preview["html"]).elements if tag == "input"]
    assert [attrs["value"] for attrs in inputs] == ["First type wins", "Ordinary fallback", "Invalid type fallback"]
    assert all(attrs["type"] == "text" and attrs["disabled"] == "disabled" for attrs in inputs)
    assert validate_application_preview(preview) == preview


def test_hidden_input_removal_keeps_existing_attribute_bounds_and_canonical_validation():
    with pytest.raises(ApplicationPreviewError):
        sanitize_application_preview({"html": '<p>Sample</p><input type="hidden" value="' + 'x' * 8001 + '">'})
    preview = sanitize_application_preview({"html": '<p>Sample outcome</p>'})
    preview["html"] = preview["html"].replace('<p>Sample outcome</p>',
        '<p>Sample outcome</p><input disabled="disabled" tabindex="-1" type="hidden" value="OpaqueToken">')
    preview["document_sha256"] = hashlib.sha256(preview["html"].encode()).hexdigest()
    with pytest.raises(ApplicationPreviewError):
        validate_application_preview(preview)


@pytest.mark.parametrize("attack", [
    '<script>parent.localStorage.clear();fetch("https://preview.invalid/")</script>',
    '<iframe src="https://preview.invalid/"></iframe>',
    '<meta http-equiv="refresh" content="0;url=https://preview.invalid/">',
    '<base href="https://preview.invalid/"><link rel="stylesheet" href="https://preview.invalid/x">',
    '<svg><foreignObject><p onclick="alert(1)">hostile</p></foreignObject></svg>',
    '<math><mtext><table><mglyph><style><!--</style><img src="x" onerror="alert(1)">',
    '<object data="file:///private/host"><embed src="https://preview.invalid/"></object>',
    '<template><script>alert(1)</script></template>',
    '<img src="https://preview.invalid/" srcset="https://preview.invalid/x 2x" onerror="alert(1)">',
    '<input formaction="https://preview.invalid/" autofocus name="host" onfocus="alert(1)">',
])
def test_active_browser_content_and_resource_navigation_are_never_retained(attack):
    preview = sanitize_application_preview({"html": '<p>Actual sample outcome</p>' + attack})
    elements = Elements(preview["html"]).elements
    forbidden = {"script", "iframe", "svg", "math", "object", "embed", "base", "link", "template", "img"}
    assert not any(tag in forbidden for tag, _ in elements)
    meta = [attrs for tag, attrs in elements if tag == "meta" and "http-equiv" in attrs]
    assert meta == [{"http-equiv": "Content-Security-Policy", "content": CSP}]
    for tag, attrs in elements:
        if tag == "meta":
            continue
        assert not any(key.startswith("on") or key in {
            "src", "srcset", "href", "action", "formaction", "autofocus", "name", "is", "xmlns",
        } for key in attrs)
    validate_application_preview(preview)


def test_css_cannot_relax_host_network_policy_or_end_a_style_into_script():
    preview = sanitize_application_preview({"html":
        '<style>@import "https://preview.invalid/a.css"; '
        'p{background:url(https://preview.invalid/image)}</style>'
        '<style>/*</style><script>fetch("https://preview.invalid/")</script>'
        '<p style="background:url(https://preview.invalid/private)">Sample result</p>',
    })
    # Arbitrary static CSS remains within the iframe; the trusted first CSP
    # denies every resource class except inline styling. Browser denial is a
    # separate real-browser check, not inferred from this parser assertion.
    assert "default-src &#x27;none&#x27;" in preview["html"]
    assert "script-src &#x27;none&#x27;" in preview["html"]
    assert "img-src &#x27;none&#x27;" in preview["html"]
    assert not any(tag == "script" for tag, _ in Elements(preview["html"]).elements)
    validate_application_preview(preview)


def test_css_preserves_production_selectors_and_quotes_and_removes_motion_and_resources():
    preview = sanitize_application_preview({"html":
        '<style>.cards > article{font-family:"Some Font",sans-serif;'
        'padding:12px;content:"A & B";animation:spin 1s infinite;'
        'background:url(https://preview.invalid/x)}'
        '@media(max-width:600px){.cards{display:block;transition:all 1s}}'
        '@keyframes spin{to{transform:rotate(1turn)}}'
        '@import "https://preview.invalid/style.css";</style>'
        '<details><summary>Details</summary><p style="anima\\74ion:spin 1s;'
        'color:red;trans\\69tion:all 1s">Current sample outcome</p></details>',
    })
    document = preview["html"]
    assert '.cards > article' in document and 'font-family:"Some Font",sans-serif;' in document
    assert 'content:"A & B";' in document and '@media(max-width:600px)' in document
    assert 'display:block;' in document and 'color:red;' in document
    assert not any(value in document for value in ('@keyframes', '@import', 'spin 1s', 'transition:', 'preview.invalid'))
    assert {"details", "summary"} <= {tag for tag, _ in Elements(document).elements}
    assert '<body>' in document and 'inert=' not in document
    validate_application_preview(preview)


def test_css_escape_decoding_cannot_reconstruct_a_closing_style_tag():
    preview = sanitize_application_preview({"html":
        r'<style>p:after{content:"\3c /style\3e \3c img src=x onerror=alert(1)\3e "}</style>'
        '<p>Current sample outcome</p>',
    })
    assert not any(tag in {"img", "script"} for tag, _ in Elements(preview["html"]).elements)
    assert '\\3c /style>' in preview["html"]
    validate_application_preview(preview)


@pytest.mark.parametrize("value", [
    None, [], {"html": None}, {"html": ""}, {"html": "x" * 64_001},
    {"html": "<p>x</p>", "title": "x" * 121}, {"html": "<p>x</p>", "title": False},
    {"html": "<p>x</p>", "title": "bad\nheading"}, {"html": "\x00"},
    {"html": "\ud800"}, {"html": "<p>x</p>", "url": "http://localhost"},
    {"html": "<script>Only script</script>"}, {"html": "<style>p{color:red}</style>"},
    {"html": "<div>" * 82 + "deep"}, {"html": "<br>" * 4001 + "wide"},
    {"html": "&" * 30_000},
])
def test_malformed_active_only_or_unbounded_hook_output_is_rejected(value):
    with pytest.raises(ApplicationPreviewError):
        sanitize_application_preview(value)


@pytest.mark.parametrize("replacement", [
    '<p onclick="fetch(\'https://preview.invalid/\')">Sample result</p>',
    '<p>Sample result</p><script>alert(1)</script>',
    '<p>Sample result</p><iframe src="https://preview.invalid/"></iframe>',
    '<a href="https://preview.invalid/">Sample result</a>',
])
def test_rehashed_retained_active_content_is_not_accepted_as_host_document(replacement):
    preview = sanitize_application_preview({"html": "<p>Sample result</p>"})
    preview["html"] = preview["html"].replace("<p>Sample result</p>", replacement)
    preview["document_sha256"] = hashlib.sha256(preview["html"].encode()).hexdigest()
    with pytest.raises(ApplicationPreviewError):
        validate_application_preview(preview)


def test_changed_csp_digest_and_scope_fail_and_host_binding_remains_separate():
    preview = sanitize_application_preview({"html": "<p>Sample result</p>"})
    preview.update(trial_id="trial", build_id="build", source_digest="source", observation_revision=3)
    assert validate_application_preview(preview) == preview
    for changes in [{"html": preview["html"] + " "}, {"live_ready": True}, {"scope": "live"}]:
        with pytest.raises(ApplicationPreviewError):
            validate_application_preview({**preview, **changes})
    changed = {**preview, "html": preview["html"].replace("default-src &#x27;none&#x27;", "default-src *")}
    changed["document_sha256"] = hashlib.sha256(changed["html"].encode()).hexdigest()
    with pytest.raises(ApplicationPreviewError):
        validate_application_preview(changed)


def test_closed_evidence_disclosure_retains_native_structure_without_opening():
    # Parser assertions establish the authored structure, not browser visibility.
    original = "A long original evidence paragraph. " * 200
    preview = sanitize_application_preview({"html":
        '<main><h1>Next action: ask for a replacement strap</h1>'
        '<details class="evidence"><summary>Original evidence</summary>'
        '<p>' + original + '</p></details></main>'})
    nodes = Elements(preview["html"]).elements
    disclosures = [attrs for tag, attrs in nodes if tag == "details"]
    assert disclosures == [{"class": "evidence"}]
    assert sum(tag == "summary" for tag, _ in nodes) == 1
    assert original in preview["html"]
    assert "Next action: ask for a replacement strap" in preview["html"]
    assert preview["live_ready"] is False
    assert validate_application_preview(preview) == preview


@pytest.mark.parametrize("authored", [
    '<details open>', '<details open="">', '<details open="false">',
    '<details open="open">', '<details open open="false">',
])
def test_authored_open_presence_is_canonical_and_validation_is_idempotent(authored):
    preview = sanitize_application_preview({"html": authored +
        '<summary>Original evidence</summary><p>Visible sample evidence</p></details>'})
    disclosures = [attrs for tag, attrs in Elements(preview["html"]).elements if tag == "details"]
    assert disclosures == [{"open": "open"}]
    assert validate_application_preview(preview) == preview


def test_nested_disclosures_preserve_reading_but_not_application_actions_or_resources():
    preview = sanitize_application_preview({"html":
        '<details ontoggle="fetch(1)" onbeforetoggle="submit()" name="shared-group">'
        '<summary onclick="submit()" is="active-summary">Original evidence</summary>'
        '<p>Staff request</p><details><summary>Attribution</summary><p>Repair assessment</p></details>'
        '<form action="/approve"><input type="hidden" value="OpaquePrivateOperation">'
        '<input value="Sample note"><button onclick="submit()">Approve</button></form>'
        '<a href="https://preview.invalid/approve">Read sample source</a>'
        '<script>ForbiddenPrivateScript</script><iframe src="https://preview.invalid/"></iframe>'
        '<style>details{padding:8px;background:url(https://preview.invalid/x);transition:all 1s}</style>'
        '</details>'})
    nodes = Elements(preview["html"]).elements
    assert sum(tag == "details" for tag, _ in nodes) == 2
    assert sum(tag == "summary" for tag, _ in nodes) == 2
    assert all("open" not in attrs for tag, attrs in nodes if tag == "details")
    assert not any(tag in {"form", "a", "script", "iframe", "img"} for tag, _ in nodes)
    assert "OpaquePrivateOperation" not in preview["html"]
    assert "ForbiddenPrivateScript" not in preview["html"]
    assert "preview.invalid" not in preview["html"] and "transition:" not in preview["html"]
    for tag, attrs in nodes:
        if tag == "meta":
            continue
        assert not any(key.startswith("on") or key in {"href", "action", "src", "name", "is"} for key in attrs)
        if tag in {"input", "button"}:
            assert attrs["disabled"] == "disabled" and attrs["tabindex"] == "-1"
    meta = [attrs for tag, attrs in nodes if tag == "meta" and "http-equiv" in attrs]
    assert meta == [{"http-equiv": "Content-Security-Policy", "content": CSP}]
    assert validate_application_preview(preview) == preview


@pytest.mark.parametrize("event", ["ontoggle", "onbeforetoggle", "onclick"])
def test_rehashed_disclosure_event_handlers_do_not_become_trusted_host_html(event):
    preview = sanitize_application_preview({"html":
        '<details><summary>Original evidence</summary><p>Sample outcome</p></details>'})
    preview["html"] = preview["html"].replace('<details>', '<details ' + event + '="fetch(1)">')
    preview["document_sha256"] = hashlib.sha256(preview["html"].encode()).hexdigest()
    with pytest.raises(ApplicationPreviewError):
        validate_application_preview(preview)


def test_legacy_flattened_preview_stays_canonical_without_borrowing_new_evidence():
    # An earlier host can have emitted open on the div replacing details.
    # Preserve this already-inert legacy representation under inert-html-v1.
    preview = sanitize_application_preview({"html":
        '<div class="evidence" open="open"><p>Original evidence</p><p>Saved sample outcome</p></div>'})
    bound = {**preview, "trial_id": "owned-synthetic-trial", "build_id": "owned-synthetic-build",
        "source_digest": "a" * 64, "guide_version": 10, "observation_revision": 3}
    assert validate_application_preview(bound) == bound
    assert bound["document_sha256"] == hashlib.sha256(bound["html"].encode()).hexdigest()
    assert not any(tag in {"details", "summary"} for tag, _ in Elements(bound["html"]).elements)


def test_boolean_open_normalization_does_not_bypass_existing_attribute_bounds():
    with pytest.raises(ApplicationPreviewError):
        sanitize_application_preview({"html": '<details open="' + 'x' * 8001 +
            '"><summary>Original evidence</summary><p>Sample</p></details>'})
