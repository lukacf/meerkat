"""Host-owned inert snapshots of an application's production-rendered trial view.

This is a viewing boundary, not an interactive browser sandbox. Authored script,
navigation, external assets and live authentication never enter the document.
"""
from __future__ import annotations

import hashlib
from html import escape
from html.parser import HTMLParser
import re

import tinycss2

MAX_PREVIEW_INPUT_BYTES = 64_000
MAX_PREVIEW_DOCUMENT_BYTES = 100_000
MAX_PREVIEW_TITLE = 120
SANITIZATION = "inert-html-v1"
SCOPE = "offline-application-preview"
CSP = ("default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; "
       "img-src 'none'; font-src 'none'; connect-src 'none'; media-src 'none'; "
       "object-src 'none'; frame-src 'none'; worker-src 'none'; "
       "base-uri 'none'; form-action 'none'")
_BASE_STYLE = ("html{color-scheme:light}body{margin:0;padding:20px;font:14px/1.5 "
               "system-ui,sans-serif;color:#252525;background:#fff;overflow-wrap:anywhere}"
               "*,*:before,*:after{box-sizing:border-box}img{display:none}")
_PREFIX = ('<!doctype html><html><head><meta charset="utf-8">'
           '<meta http-equiv="Content-Security-Policy" content="' + escape(CSP, quote=True) + '">'
           '<meta name="viewport" content="width=device-width, initial-scale=1">')
_ALLOWED = set("a abbr address article aside b bdi bdo blockquote br button caption code col "
               "colgroup dd del details dfn div dl dt em fieldset figcaption figure footer h1 "
               "h2 h3 h4 h5 h6 header hr i input kbd label legend li main mark meter nav ol "
               "optgroup option output p pre progress q s samp section select small span "
               "strong style sub summary sup table tbody td textarea tfoot th thead time "
               "tr u ul var wbr".split())
_VOID = {"br", "col", "hr", "input", "wbr"}
_DROP = set("script iframe frame frameset object embed svg math template noscript base "
            "meta link canvas video audio source track portal fencedframe title".split())
_DROP_VOID = {"base", "meta", "link", "embed", "source", "track", "frame"}
_ATTRS = set("class id title lang dir role style aria-label aria-labelledby aria-describedby "
             "aria-hidden colspan rowspan scope headers datetime open start reversed value "
             "min max low high optimum selected checked placeholder rows cols width height".split())
_CONTROLS = {"button", "input", "select", "textarea", "option", "optgroup", "fieldset"}
_CONTROL_CHARS = re.compile(r"[\x00-\x1f\x7f]")


def _resource_value(tokens):
    for token in tokens:
        if token.type == "url" or (token.type == "function" and token.lower_name in {"url", "image-set", "-webkit-image-set"}):
            return True
        children = getattr(token, "arguments", getattr(token, "content", None))
        if children is not None and _resource_value(children):
            return True
    return False


def _declarations(source):
    kept = []
    for item in tinycss2.parse_blocks_contents(source, skip_comments=True, skip_whitespace=True):
        if item.type != "declaration":
            continue
        name = item.lower_name
        plain = name.removeprefix("-webkit-").removeprefix("-moz-").removeprefix("-ms-")
        if (plain.startswith(("animation", "transition")) or name in {"behavior", "-moz-binding"}
                or _resource_value(item.value)):
            continue
        kept.append(item)
    return tinycss2.serialize(kept)


def _stylesheet(source, depth=0):
    if depth > 16:
        raise ApplicationPreviewError("The preview stylesheet exceeds its nesting bounds.")
    kept = []
    for item in tinycss2.parse_stylesheet(source, skip_comments=True, skip_whitespace=True):
        if item.type == "qualified-rule":
            content = _declarations(item.content)
            if content:
                item.content = tinycss2.parse_component_value_list(content)
                kept.append(item)
        elif (item.type == "at-rule" and item.lower_at_keyword in {"media", "supports", "container", "layer"}
                and item.content is not None):
            content = _stylesheet(tinycss2.serialize(item.content), depth + 1)
            if content:
                item.content = tinycss2.parse_component_value_list(content)
                kept.append(item)
    # CSS string serialization can decode an authored escape into '<'. Keep
    # its CSS meaning while preventing raw-text closing tags after parsing.
    return tinycss2.serialize(kept).replace("<", "\\3c ")


class ApplicationPreviewError(ValueError):
    pass


def _text_size(value):
    if not isinstance(value, str):
        raise ApplicationPreviewError("An application preview must contain ordinary text.")
    try:
        return len(value.encode("utf-8"))
    except UnicodeError:
        raise ApplicationPreviewError("An application preview must contain valid Unicode text.") from None


class _InertHTML(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.output = []
        self.stack = []
        self.blocked = []
        self.nodes = 0
        self.visible = False

    def handle_starttag(self, tag, attrs):
        self.nodes += 1
        if self.nodes > 4000 or len(self.stack) + len(self.blocked) > 80:
            raise ApplicationPreviewError("The application preview exceeds its document bounds.")
        if self.blocked:
            if tag not in _VOID and tag not in _DROP_VOID:
                self.blocked.append(tag)
            return
        if tag in _DROP:
            if tag not in _DROP_VOID:
                self.blocked.append(tag)
            return
        if tag == "img":
            alt = next((value for key, value in attrs if key == "alt" and value), "")
            if alt:
                self.output.append('<span role="img">' + escape(alt) + '</span>')
                self.visible = True
            return
        # Navigation and submission tags become inert containers. Native disclosure
        # remains local viewing; unknown tags and customized built-ins are removed.
        original = tag
        tag = {"a": "span", "form": "div"}.get(tag, tag)
        if tag not in _ALLOWED:
            return
        selected = []
        seen = set()
        for key, value in attrs:
            if key not in _ATTRS or key in seen or value is None and not (tag == "details" and key == "open"):
                continue
            if value is not None and (_text_size(value) > 8000 or "\x00" in value):
                raise ApplicationPreviewError("The application preview has an oversized attribute.")
            seen.add(key)
            if tag == "details" and key == "open":
                # HTML boolean presence is true even when its value says false.
                value = "open"
            elif key == "style":
                value = _declarations(value)
            selected.append(' ' + key + '="' + escape(value, quote=True) + '"')
        # Hidden form state is not a visible production control. Dropping its
        # type would expose opaque tokens as text fields in the static preview.
        if tag == "input":
            input_type = next((value for key, value in attrs if key == "type"), "text")
            if isinstance(input_type, str) and input_type.lower() == "hidden":
                return
        if tag in _CONTROLS:
            selected.append(' disabled="disabled" tabindex="-1"')
        if tag == "input":
            if input_type not in {"text", "password", "number", "date", "checkbox", "radio", "email", "search", "tel", "url", "range", "color"}:
                input_type = "text"
            selected.append(' type="' + input_type + '"')
        self.output.append('<' + tag + ''.join(selected) + '>')
        if tag not in _VOID:
            self.stack.append((original, tag))

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)
        if tag not in _VOID and tag not in _DROP_VOID:
            self.handle_endtag(tag)

    def handle_endtag(self, tag):
        if self.blocked:
            if tag in self.blocked:
                self.blocked = self.blocked[:self.blocked.index(tag)]
            return
        for position in range(len(self.stack) - 1, -1, -1):
            if self.stack[position][0] == tag:
                for _, rendered in reversed(self.stack[position:]):
                    self.output.append('</' + rendered + '>')
                del self.stack[position:]
                return

    def handle_data(self, value):
        if self.blocked:
            return
        # Style is raw text in the browser. Escaping quotes or child selectors
        # would change the production CSS. The parser separates its closing
        # tag; additionally reject any retained closing-tag sequence.
        if self.stack and self.stack[-1][1] == "style":
            if re.search(r"</\s*style", value, re.IGNORECASE):
                raise ApplicationPreviewError("The preview stylesheet cannot contain a closing tag.")
            self.output.append(_stylesheet(value))
        else:
            self.output.append(escape(value))
        if value.strip() and (not self.stack or self.stack[-1][1] != "style"):
            self.visible = True

    def document_body(self):
        for _, tag in reversed(self.stack):
            self.output.append('</' + tag + '>')
        return ''.join(self.output)


def sanitize_application_preview(value):
    """Normalize authored {html, title?}; never return raw authored markup."""
    if not isinstance(value, dict) or set(value) - {"html", "title"}:
        raise ApplicationPreviewError("The preview hook must return only HTML and an optional title.")
    source = value.get("html")
    if not _text_size(source) or _text_size(source) > MAX_PREVIEW_INPUT_BYTES or "\x00" in source:
        raise ApplicationPreviewError("The application preview must fit within 64 KB.")
    title = value.get("title", "Application preview")
    if _text_size(title) > MAX_PREVIEW_TITLE * 4 or len(title) > MAX_PREVIEW_TITLE or _CONTROL_CHARS.search(title):
        raise ApplicationPreviewError("The application preview title is invalid or too long.")
    if not title.strip():
        title = "Application preview"
    parser = _InertHTML()
    try:
        parser.feed(source)
        parser.close()
        body = parser.document_body()
    except ApplicationPreviewError:
        raise
    except (ValueError, RecursionError):
        raise ApplicationPreviewError("The application preview could not be made inert.") from None
    if not parser.visible:
        raise ApplicationPreviewError("The application preview has no viewable application content.")
    document = (_PREFIX + '<title>' + escape(title) + '</title><style>' + _BASE_STYLE
                + '</style></head><body>' + body + '</body></html>')
    if _text_size(document) > MAX_PREVIEW_DOCUMENT_BYTES:
        raise ApplicationPreviewError("The inert application preview exceeds its document bounds.")
    return {"status": "available", "scope": SCOPE, "sanitization": SANITIZATION,
            "title": title, "html": document, "detail": None, "live_ready": False,
            "document_sha256": hashlib.sha256(document.encode()).hexdigest()}


def validate_application_preview(value):
    """Validate a retained host projection, preserving its separate scope binding.

    The exact host envelope and a fresh parser pass defend persisted evidence:
    a changed hash alone cannot legitimize script, URL or event-bearing content.
    Trial/build/revision authorization remains with the rehearsal owner.
    """
    allowed = {"status", "scope", "sanitization", "live_ready", "title", "html", "detail",
               "document_sha256", "trial_id", "build_id", "source_digest", "guide_version",
               "observation_revision"}
    if (not isinstance(value, dict) or set(value) - allowed or value.get("scope") != SCOPE
            or value.get("sanitization") != SANITIZATION or value.get("live_ready") is not False
            or value.get("status") not in {"available", "unsupported", "failed"}):
        raise ApplicationPreviewError("The retained application preview is invalid.")
    if value["status"] != "available":
        detail = value.get("detail")
        if (value.get("html") is not None or value.get("document_sha256") is not None
                or value.get("title") is not None or not isinstance(detail, str)
                or _text_size(detail) > 2400 or len(detail) > 600 or _CONTROL_CHARS.search(detail)):
            raise ApplicationPreviewError("An unavailable application preview cannot contain a document.")
        return dict(value)
    document, title = value.get("html"), value.get("title")
    if (_text_size(document) > MAX_PREVIEW_DOCUMENT_BYTES or not isinstance(title, str)
            or not title.strip() or len(title) > MAX_PREVIEW_TITLE or _CONTROL_CHARS.search(title)):
        raise ApplicationPreviewError("The retained application preview exceeds its bounds.")
    prefix = (_PREFIX + '<title>' + escape(title) + '</title><style>' + _BASE_STYLE
              + '</style></head><body>')
    suffix = '</body></html>'
    if not document.startswith(prefix) or not document.endswith(suffix):
        raise ApplicationPreviewError("The retained application preview lacks its host envelope.")
    if hashlib.sha256(document.encode()).hexdigest() != value.get("document_sha256"):
        raise ApplicationPreviewError("The retained application preview document changed.")
    body = document[len(prefix):-len(suffix)]
    parser = _InertHTML()
    parser.feed(body)
    parser.close()
    if parser.document_body() != body or not parser.visible:
        raise ApplicationPreviewError("The retained application preview is not canonical inert HTML.")
    return dict(value)
