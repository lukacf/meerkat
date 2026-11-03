"""Read the installed, application-selected Console presentation snapshot.

The base authenticated shell selects only its own JS/CSS. The qualified setup
owner may additionally select the fixed public Toolkit JS/CSS closure.
No discovery, acquisition, Connect construction or native lifetime lives here.
"""

import os
import stat
from types import MappingProxyType

if __package__:
    from .console_shell import render_console_shell
else:
    from console_shell import render_console_shell


MAX_ASSET_BYTES = 16 * 1024 * 1024
PREFIX = "/console/forge/assets/"
BASE_FILES = ("console_shell.mjs", "console_shell.css")
SETUP_FILES = ("console_setup.mjs", "console_setup.css")
REVIEW_FILES = ("controller_review.mjs", "controller_review.css")
SHELL_FILES = BASE_FILES + SETUP_FILES
# The acquired index also imports the separate public view/projection modules.
# Retain their exact relative layout even when only setup is currently mounted.
TOOLKIT_FILES = ("index.js", "model.js", "projection.js", "setup-conversation.js",
                 "setup-markdown.js", "setup-model.js", "view.js", "style.css",
                 "vendor/entities/decode-codepoint.js", "vendor/entities/decode.js",
                 "vendor/entities/generated/decode-data-html.js",
                 "vendor/entities/generated/decode-data-xml.js",
                 "vendor/entities/internal/bin-trie-flags.js",
                 "vendor/entities/internal/decode-shared.js",
                 "vendor/marked/marked.esm.js")
URLS = MappingProxyType({
    "shell_js": PREFIX + "console_shell.mjs",
    "shell_css": PREFIX + "console_shell.css",
    "setup_js": PREFIX + "console_setup.mjs",
    "setup_css": PREFIX + "console_setup.css",
    "toolkit_ui_js": PREFIX + "toolkit-ui/index.js",
    "toolkit_ui_css": PREFIX + "toolkit-ui/style.css",
    "review_js": PREFIX + "controller_review.mjs",
    "review_css": PREFIX + "controller_review.css",
})


def _stamp(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_nlink,
            info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def _read_regular(directory, name):
    descriptor, ancestors, directories = None, [], []
    try:
        components = name.split("/")
        parent = directory
        for component in components[:-1]:
            child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=parent)
            directories.append(child)
            ancestors.append((parent, component, child, os.fstat(child)))
            parent = child
        descriptor = os.open(components[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                             dir_fd=parent)
        before = os.fstat(descriptor)
        if (not stat.S_ISREG(before.st_mode) or before.st_nlink != 1
                or before.st_size > MAX_ASSET_BYTES):
            raise ValueError("Installed Console presentation assets are unavailable.")
        parts, size = [], 0
        while True:
            chunk = os.read(descriptor, min(64 * 1024, MAX_ASSET_BYTES + 1 - size))
            if not chunk:
                break
            parts.append(chunk); size += len(chunk)
            if size > MAX_ASSET_BYTES:
                raise ValueError("Installed Console presentation assets are unavailable.")
        after = os.fstat(descriptor)
        selected = os.stat(components[-1], dir_fd=parent, follow_symlinks=False)
        if (_stamp(before) != _stamp(after) or _stamp(after) != _stamp(selected)
                or size != before.st_size):
            raise ValueError("Installed Console presentation assets changed while being read.")
        for parent, component, child, before in ancestors:
            after = os.fstat(child)
            selected = os.stat(component, dir_fd=parent, follow_symlinks=False)
            if _stamp(before) != _stamp(after) or _stamp(after) != _stamp(selected):
                raise ValueError("Installed Console presentation assets changed while being read.")
        return b"".join(parts)
    finally:
        if descriptor is not None:
            os.close(descriptor)
        for child in reversed(directories):
            os.close(child)


def load_console_presentation(installed_root, *, setup=False, controller_review=False):
    """Return (immutable public byte/MIME map, pure HTML transform).

    ROOT is already selected by the trusted application owner. No source path,
    file name or module URL is selected by the browser. Optional setup is read
    only when its existing qualified owner selects it. Missing or unsafe input
    refuses the selected snapshot; partial maps are never returned.
    """
    if type(setup) is not bool or type(controller_review) is not bool:
        raise ValueError("Console presentation requires explicit application view selections.")
    directory = toolkit = None
    try:
        directory = os.open(installed_root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        assets = {}
        selected_files = (SHELL_FILES if setup else BASE_FILES) + (REVIEW_FILES if controller_review else ())
        for name in selected_files:
            assets[PREFIX + name] = (_read_regular(directory, name),
                                     "text/css" if name.endswith(".css") else "application/javascript")
        if setup:
            toolkit = os.open("toolkit-ui", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                              dir_fd=directory)
            toolkit_before = os.fstat(toolkit)
            for name in TOOLKIT_FILES:
                assets[PREFIX + "toolkit-ui/" + name] = (_read_regular(toolkit, name),
                    "text/css" if name.endswith(".css") else "application/javascript")
            toolkit_after = os.fstat(toolkit)
            toolkit_selected = os.stat("toolkit-ui", dir_fd=directory, follow_symlinks=False)
            if (_stamp(toolkit_before) != _stamp(toolkit_after)
                    or _stamp(toolkit_after) != _stamp(toolkit_selected)):
                raise ValueError("Installed Console presentation assets changed while being read.")
    except (OSError, TypeError) as error:
        raise ValueError("Installed Console presentation assets are unavailable.") from None
    finally:
        if toolkit is not None:
            os.close(toolkit)
        if directory is not None:
            os.close(directory)
    # Bytes and tuple values are immutable; neither later file edits nor caller
    # map mutations can change the selected response payloads.
    public_assets = MappingProxyType(assets)

    def document_transform(document):
        names = ("shell_js", "shell_css")
        if setup:
            names += ("setup_js", "setup_css", "toolkit_ui_js", "toolkit_ui_css")
        if controller_review:
            names += ("review_js", "review_css")
        selected = {name: URLS[name] for name in names}
        return render_console_shell(document, **selected)

    return public_assets, document_transform
