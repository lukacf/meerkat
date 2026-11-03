"""Freeze canonical declaration helpers into one standalone destination controller."""
from pathlib import Path


def controller_bytes(*, product_control=None):
    """Return exact controller and helper bytes under a single controller digest.

    Existing stored controllers are never rewritten. The injected literal runs
    before main() and is also available to the existing detached worker loader.
    """
    root = Path(__file__).parent
    source = (root / "remote_destination.py").read_bytes()
    parser = (root / "application_experiences.py").read_bytes()
    console = (root / "templates_v2/console_configuration.py").read_bytes()
    marker = b"\nif __name__ == '__main__':\n"
    if source.count(marker) != 1:
        raise ValueError("The standalone destination controller entry point changed.")
    payload = b"\nAPPLICATION_EXPERIENCES_SOURCE = " + repr(parser).encode("ascii") + b"\n"
    payload += b"CONSOLE_CONFIGURATION_SOURCE = " + repr(console).encode("ascii") + b"\n"
    if product_control is not None:
        from .remote_product_control import ProductControlBundle, _json
        if type(product_control) is not ProductControlBundle:
            raise ValueError("Select the original trusted product control bundle.")
        product_control.require_current()
        adapter = (root / "remote_product_control.py").read_bytes()
        payload += b"PRODUCT_CONTROL_MANIFEST = " + repr(_json(product_control.manifest)).encode("ascii") + b"\n"
        payload += b"PRODUCT_CONTROL_ADAPTER_SOURCE = " + repr(adapter).encode("ascii") + b"\n"
        product_control.require_current()
    return source.replace(marker, payload + marker)


def product_artifact_receiver_bytes():
    """Freeze a separate inert receiver and its exact trusted integrity readers.

    Transferred filenames cannot supply Python modules. The isolated receiver
    constructs its helper packages entirely from these frozen literals. This
    bundle never changes a retained ordinary destination controller.
    """
    import hashlib
    import os
    import stat
    root = Path(__file__).parent
    modules = (
        ("launchkit.sdk_wheel_integrity", "sdk_wheel_integrity.py"),
        ("launchkit.runtime_candidates", "runtime_candidates.py"),
        ("launchkit.templates.install_candidate", "templates/install_candidate.py"),
        ("launchkit.runtime_selection", "runtime_selection.py"),
        ("launchkit.remote_product_artifacts", "remote_product_artifacts.py"),
    )
    filenames = [name for _, name in modules] + ["remote_product_artifact_receiver.py"]
    frozen = {}
    def identity(info):
        return (info.st_dev, info.st_ino, info.st_mode, info.st_uid,
            info.st_nlink, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
    for name in filenames:
        path = root / name
        if any(item.is_symlink() for item in (path, *path.parents)):
            raise ValueError("The trusted artifact receiver source contains a link.")
        before = path.lstat()
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            info = os.fstat(stream.fileno())
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                    or info.st_nlink != 1 or not 0 < info.st_size <= 4 * 1024 * 1024
                    or identity(info) != identity(before)):
                raise ValueError("The trusted artifact receiver source is not an owned bounded file.")
            content = stream.read(4 * 1024 * 1024 + 1)
            if len(content) != info.st_size or identity(os.fstat(stream.fileno())) != identity(info):
                raise ValueError("The trusted artifact receiver source changed during freezing.")
        frozen[name] = content, identity(info), hashlib.sha256(content).hexdigest()
    for name, (_, observed, _) in frozen.items():
        if identity((root / name).lstat()) != observed:
            raise ValueError("The trusted artifact receiver source changed during freezing.")
    lines = [
        "import hashlib, sys, types",
        "for _package in ('launchkit', 'launchkit.templates'):",
        "    _module = types.ModuleType(_package)",
        "    _module.__path__ = []",
        "    _module.__package__ = _package",
        "    sys.modules[_package] = _module",
    ]
    for module_name, filename in modules:
        content, _, digest = frozen[filename]
        lines.extend((
            "_source = " + repr(content),
            "assert hashlib.sha256(_source).hexdigest() == " + repr(digest),
            "_module = types.ModuleType(" + repr(module_name) + ")",
            "_module.__file__ = " + repr("/__forge_frozen__/" + filename),
            "_module.__package__ = " + repr(module_name.rsplit(".", 1)[0]),
            "sys.modules[_module.__name__] = _module",
            "exec(compile(_source, _module.__file__, 'exec'), _module.__dict__)",
        ))
    source, _, digest = frozen["remote_product_artifact_receiver.py"]
    lines.extend((
        "_source = " + repr(source),
        "assert hashlib.sha256(_source).hexdigest() == " + repr(digest),
        "__package__ = 'launchkit'",
        "exec(compile(_source, '/__forge_frozen__/remote_product_artifact_receiver.py', 'exec'), globals())",
    ))
    return ("\n".join(lines) + "\n").encode("ascii")


def product_destination_owner_bytes():
    """Freeze separate stopped-source intake; keep ordinary controllers intact.

    The existing artifact receiver loads only its canonical frozen readers.
    Source staging adds the existing project lease and destination location
    lock, never Python imports from a received application or artifact tree.
    """
    import hashlib
    import os
    import stat
    root = Path(__file__).parent
    base = product_artifact_receiver_bytes()
    names = ("templates/storage_setup.py", "remote_destination.py", "remote_product_destination_owner.py")
    frozen = {}
    def identity(info):
        return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_nlink,
            info.st_size, info.st_mtime_ns, info.st_ctime_ns)
    for name in names:
        path = root / name
        if any(item.is_symlink() for item in (path, *path.parents)):
            raise ValueError("The trusted destination source owner contains a link.")
        before = path.lstat()
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as handle:
            info = os.fstat(handle.fileno())
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1
                    or not 0 < info.st_size <= 4 * 1024 * 1024 or identity(before) != identity(info)):
                raise ValueError("The trusted destination source owner is not an owned bounded file.")
            content = handle.read(4 * 1024 * 1024 + 1)
            if len(content) != info.st_size or identity(info) != identity(os.fstat(handle.fileno())):
                raise ValueError("The trusted destination source owner changed during freezing.")
        frozen[name] = content, identity(info)
    for name, (_, observed) in frozen.items():
        if identity((root / name).lstat()) != observed:
            raise ValueError("The trusted destination source owner changed during freezing.")
    lines = [
        "import hashlib, sys, types",
        "_artifact = types.ModuleType('launchkit.remote_product_artifact_receiver')",
        "_artifact.__package__ = 'launchkit'",
        "_artifact_source = " + repr(base),
        "assert hashlib.sha256(_artifact_source).hexdigest() == " + repr(hashlib.sha256(base).hexdigest()),
        "exec(compile(_artifact_source, '/__forge_frozen__/artifact-receiver-bundle.py', 'exec'), _artifact.__dict__)",
        "sys.modules[_artifact.__name__] = _artifact",
    ]
    for module_name, filename in (("launchkit.templates.storage_setup", names[0]),
            ("launchkit.remote_destination", names[1])):
        content, _ = frozen[filename]
        lines.extend(("_source = " + repr(content),
            "assert hashlib.sha256(_source).hexdigest() == " + repr(hashlib.sha256(content).hexdigest()),
            "_module = types.ModuleType(" + repr(module_name) + ")",
            "_module.__file__ = " + repr("/__forge_frozen__/" + filename),
            "_module.__package__ = " + repr(module_name.rsplit(".", 1)[0]),
            "sys.modules[_module.__name__] = _module",
            "exec(compile(_source, _module.__file__, 'exec'), _module.__dict__)",
        ))
    content, _ = frozen[names[2]]
    lines.extend(("_source = " + repr(content),
        "assert hashlib.sha256(_source).hexdigest() == " + repr(hashlib.sha256(content).hexdigest()),
        "__package__ = 'launchkit'",
        "exec(compile(_source, '/__forge_frozen__/remote_product_destination_owner.py', 'exec'), globals())"))
    return ("\n".join(lines) + "\n").encode("ascii")
