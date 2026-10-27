"""Compose presentation with exact application-selected local Console assets.

This function owns no acquisition, authentication, bootstrap or native lifetime.
The existing authenticated ConsoleFront decides whether to call it.
"""

import json
import re


def _asset_url(value):
    if (not isinstance(value, str)
            or not re.fullmatch(r"/console/forge/assets/[A-Za-z0-9._/-]+", value)
            or any(part in {"", ".", ".."} for part in value.split("/")[1:])):
        raise ValueError("Console shell requires an exact trusted local asset URL.")
    return value


def render_console_shell(document, *, shell_js, shell_css, setup_js=None, setup_css=None,
                         toolkit_ui_js=None, toolkit_ui_css=None, review_js=None, review_css=None):
    """Keep the native document intact and append a sibling-shell bootstrap.

    URLs are selected by the application-owned immutable resource map, never by
    browser input. Setup's complete optional URL closure must be selected by
    its qualified owner; the base shell does not import it.
    """
    selected = {
        "shell_js": shell_js, "shell_css": shell_css,
        "setup_js": setup_js, "setup_css": setup_css,
        "toolkit_ui_js": toolkit_ui_js, "toolkit_ui_css": toolkit_ui_css,
        "review_js": review_js, "review_css": review_css,
    }
    setup_names = ("setup_js", "setup_css", "toolkit_ui_js", "toolkit_ui_css")
    setup = any(selected[name] is not None for name in setup_names)
    if setup and any(selected[name] is None for name in setup_names):
        raise ValueError("Console setup requires its complete selected asset closure.")
    review = review_js is not None or review_css is not None
    if review and (review_js is None or review_css is None):
        raise ValueError("Controller review requires its complete selected asset closure.")
    urls = {name: _asset_url(value) for name, value in selected.items() if value is not None}
    urls["shell_js"] = _asset_url(shell_js)
    urls["shell_css"] = _asset_url(shell_css)
    if not isinstance(document, bytes):
        raise ValueError("Console shell requires the native HTML bytes.")
    text = document.decode("utf-8")
    if (text.count('</head>') != 1 or text.count('</body>') != 1
            or len(re.findall(r'<(?:div|main)\s[^>]*\bid=["\']root["\']', text)) != 1
            or 'data-forge-console-shell' in text):
        raise ValueError("Console shell requires one unmodified native document.")
    # All native tags and their ordering survive; the last stylesheet isolates
    # shell geometry from the native Console's global root/button reset rules.
    style_names = (("toolkit_ui_css", "setup_css") if setup else ())
    style_names += ("review_css",) if review else ()
    style_names += ("shell_css",)
    styles = "\n".join(f'<link rel="stylesheet" href="{urls[name]}" />' for name in style_names)
    boot = f'import {{ mountConsoleShell }} from {json.dumps(urls["shell_js"])};\n'
    if setup or review:
        # Mount base controls before optional module loading. Even a failed
        # setup import cannot remove the host's ordinary sign-out control.
        boot += 'mountConsoleShell({ '
        if setup:
            boot += ('loadSetup: async () => {\n'
                 '  const [{ mountConsoleSetup }, { mountSetupConversation }] = await Promise.all([\n'
                 f'    import({json.dumps(urls["setup_js"])}),\n'
                 f'    import({json.dumps(urls["toolkit_ui_js"])})\n'
                 '  ]);\n'
                 '  return { mountConsoleSetup, mountSetupConversation };\n'
                 '}')
        if review:
            boot += ', ' if setup else ''
            boot += ('loadControllerReview: async () => {\n'
                     '  const { mountControllerReview } = await '
                     f'import({json.dumps(urls["review_js"])});\n'
                     '  return { mountControllerReview };\n'
                     '}')
        boot += ' });\n'
    else:
        boot += 'mountConsoleShell();\n'
    def insert_before_closing(value, tag, addition):
        match = re.search(r"[ \t]*" + re.escape(tag), value)
        return value[:match.start()] + addition + "\n" + value[match.start():]

    text = insert_before_closing(text, '</head>', styles)
    return insert_before_closing(text, '</body>',
        f'<script type="module" data-forge-console-shell>{boot}</script>').encode('utf-8')
