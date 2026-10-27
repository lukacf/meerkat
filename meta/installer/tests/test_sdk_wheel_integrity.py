"""Inert SDK byte counterexamples; no selected SDK or native code is loaded."""
import base64
import csv
from dataclasses import replace
import hashlib
import io
import stat
import warnings
import zipfile

import pytest

from launchkit import runtime_candidates
from launchkit import sdk_wheel_integrity as integrity


def digest(value):
    return hashlib.sha256(value).hexdigest()


def archive_bytes(values, prefix, *, record=None, duplicate=None, link=None):
    rows = [[name, "sha256=" + base64.urlsafe_b64encode(hashlib.sha256(value).digest()).decode().rstrip("="),
             str(len(value))] for name, value in values.items()]
    rows.append([prefix + "RECORD", "", ""])
    output = io.StringIO()
    csv.writer(output, lineterminator="\n").writerows(rows)
    contents = {**values, prefix + "RECORD": output.getvalue().encode() if record is None else record}
    result = io.BytesIO()
    with warnings.catch_warnings(), zipfile.ZipFile(result, "w", zipfile.ZIP_DEFLATED) as archive:
        warnings.simplefilter("ignore", UserWarning)
        for name, value in contents.items():
            entry = zipfile.ZipInfo(name)
            entry.external_attr = (stat.S_IFLNK | 0o777 if name == link else stat.S_IFREG | 0o644) << 16
            archive.writestr(entry, value)
        if duplicate:
            archive.writestr(duplicate, contents[duplicate])
    return result.getvalue()


class InertWheel:
    def __init__(self, distribution, *, version="0.9.0", meta_version="0.9.0"):
        self.distribution = distribution
        self.version = version
        self.package = distribution.replace("-", "_")
        self.prefix = f"{self.package}-{self.version}.dist-info/"
        modules = ("builder.py", "runtime.py", "_transport.py") if distribution == "meerkat-meta" else (
            "application_runtime.py", "host_control.py")
        self.payload = {self.package + "/" + name: b"raise RuntimeError('must never import or execute')\n"
                        for name in ("__init__.py", *modules)}
        self.payload[self.package + "/py.typed"] = b""
        metadata = (f"Metadata-Version: 2.4\nName: {distribution}\nVersion: {self.version}\n"
                    "License-File: LICENSE-MIT\n")
        if distribution == "meerkat-toolkit":
            metadata += f'Provides-Extra: native-host\nRequires-Dist: meerkat-meta=={meta_version}; extra == "native-host"\n'
        self.metadata = {"METADATA": (metadata + "\n").encode(),
                         "WHEEL": b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n\n",
                         "top_level.txt": (self.package + "\n").encode(),
                         "licenses/LICENSE-MIT": b"inert license text\n"}

    def selected(self, *, values=None, source=None, metadata=None, **kwargs):
        source = self.payload if source is None else source
        metadata = self.metadata if metadata is None else metadata
        declared = {**self.payload, **{self.prefix + name: value for name, value in self.metadata.items()}}
        content = archive_bytes(declared if values is None else values, self.prefix, **kwargs)
        expectation = integrity.WheelByteExpectation(self.distribution, self.version, digest(content), len(content),
            tuple((name, digest(value), len(value)) for name, value in source.items()),
            tuple((name, digest(value), len(value)) for name, value in metadata.items()))
        return content, expectation


@pytest.fixture
def pair():
    return InertWheel("meerkat-meta"), InertWheel("meerkat-toolkit")


def check_pair(pair, *, meta=None, toolkit=None):
    meta_content, meta_pin = pair[0].selected() if meta is None else meta
    toolkit_content, toolkit_pin = pair[1].selected() if toolkit is None else toolkit
    return integrity.check_meta_toolkit_wheels(meta_content, toolkit_content, meta=meta_pin, toolkit=toolkit_pin)


def test_exact_selected_pair_preserves_source_and_license_facts_without_import(pair):
    facts = check_pair(pair)
    assert [item.distribution for item in facts] == ["meerkat-meta", "meerkat-toolkit"]
    for wheel, observed in zip(pair, facts):
        _, pin = wheel.selected()
        assert observed.source_payload == tuple(sorted(pin.source_payload))
        assert observed.metadata == tuple(sorted(pin.metadata))
        assert observed.sha256 == pin.sha256 and observed.size == pin.size
        assert not hasattr(observed, "admitted") and not hasattr(observed, "gateway")


@pytest.mark.parametrize(("meta_version", "toolkit_version"), [
    ("0.9.0", "0.9.0"), ("0.9.1", "0.9.0"), ("0.9.4", "0.9.0"), ("0.9.4", "0.9.4")])
def test_selected_meta_version_is_bound_to_exact_toolkit_native_host_metadata(meta_version, toolkit_version):
    pair = (InertWheel("meerkat-meta", version=meta_version),
            InertWheel("meerkat-toolkit", version=toolkit_version, meta_version=meta_version))
    facts = check_pair(pair)
    assert (facts[0].version, facts[1].version) == (meta_version, toolkit_version)
    assert all(observed.sha256 == wheel.selected()[1].sha256 for wheel, observed in zip(pair, facts))


@pytest.mark.parametrize(("meta_version", "required_version"), [("0.9.0", "0.9.1"), ("0.9.1", "0.9.0"),
    ("0.9.1", "0.9.4"), ("0.9.4", "0.9.1")])
def test_selected_meta_cross_pair_mismatch_refuses(meta_version, required_version):
    pair = (InertWheel("meerkat-meta", version=meta_version),
            InertWheel("meerkat-toolkit", meta_version=required_version))
    with pytest.raises(integrity.WheelIntegrityError, match="exactly the selected Meta"):
        check_pair(pair)


@pytest.mark.parametrize("version", ["0.8.48", "0.9.2", "0.9.3", "1.0.0"])
def test_other_meta_versions_refuse_even_when_wheel_and_dependency_are_exact(version):
    pair = (InertWheel("meerkat-meta", version=version),
            InertWheel("meerkat-toolkit", meta_version=version))
    with pytest.raises(integrity.WheelIntegrityError, match="not supported"):
        check_pair(pair)


def test_meta_091_cannot_relabel_selected_090_bytes():
    meta = InertWheel("meerkat-meta")
    toolkit = InertWheel("meerkat-toolkit", meta_version="0.9.1")
    content, expectation = meta.selected()
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair((meta, toolkit), meta=(content, replace(expectation, version="0.9.1")))


@pytest.mark.parametrize("defect", ["selected-version", "metadata-version", "native-host-version", "record"])
def test_toolkit_094_requires_exact_selected_bytes_metadata_and_meta_constraint(defect):
    meta = InertWheel("meerkat-meta", version="0.9.4")
    toolkit = InertWheel("meerkat-toolkit", version="0.9.4", meta_version="0.9.4")
    if defect == "metadata-version":
        toolkit.metadata["METADATA"] = toolkit.metadata["METADATA"].replace(
            b"Version: 0.9.4", b"Version: 0.9.0")
    elif defect == "native-host-version":
        toolkit.metadata["METADATA"] = toolkit.metadata["METADATA"].replace(
            b"meerkat-meta==0.9.4", b"meerkat-meta==0.9.0")
    content, expectation = toolkit.selected(**({"record": b""} if defect == "record" else {}))
    if defect == "selected-version":
        expectation = replace(expectation, version="0.9.0")
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair((meta, toolkit), toolkit=(content, expectation))


@pytest.mark.parametrize("version", ["0.1.0", "0.9.1"])
def test_final_pair_rejects_other_toolkit_release_even_when_exactly_pinned(pair, version):
    other = InertWheel("meerkat-toolkit", version=version)
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair(pair, toolkit=other.selected())


@pytest.mark.parametrize("requirement", [
    'meerkat-meta (==0.9.0) ; extra == "native-host"',
    "meerkat_meta == 0.9.0; (extra == 'native_host')",
    'meerkat.meta==0.9.0; "native-host" == extra',
])
def test_native_host_requirement_equivalent_spacing_and_names(pair, requirement):
    wheel = pair[1]
    wheel.metadata["METADATA"] = wheel.metadata["METADATA"].replace(
        b'meerkat-meta==0.9.0; extra == "native-host"', requirement.encode())
    assert check_pair(pair)[1].distribution == "meerkat-toolkit"


@pytest.mark.parametrize("requirement", [
    "meerkat-meta>=0.9.0; extra == 'native-host'",
    "meerkat-meta==0.9.0.*; extra == 'native-host'",
    "meerkat-meta==0.8.48; extra == 'native-host'",
    "meerkat-meta==0.9.0",
    "meerkat-meta==0.9.0; extra == 'other'",
    "meerkat-meta==0.9.0; extra != 'native-host'",
    "meerkat-meta==0.9.0; extra == 'native-host' or python_version >= '3.11'",
    "meerkat-meta==0.9.0; __import__('os').system('forbidden')",
    'meerkat-meta==0.9.0; extra == "native-host" # comment',
    'meerkat-meta==0.9.0; extra == "native-" "host"',
    'meerkat-meta==0.9.0; extra == """native-host"""',
    'meerkat-meta==0.9.0; (extra == "native-host"',
    "meerkat-meta[anything]==0.9.0; extra == 'native-host'",
    "meerkat-meta @ https://example.invalid/wheel; extra == 'native-host'",
])
def test_native_host_requires_exact_selected_meta_without_marker_execution(pair, requirement):
    wheel = pair[1]
    wheel.metadata["METADATA"] = wheel.metadata["METADATA"].replace(
        b'meerkat-meta==0.9.0; extra == "native-host"', requirement.encode())
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair(pair)


@pytest.mark.parametrize("index", [0, 1])
@pytest.mark.parametrize("requirement", ["meerkat-mobkit", "meerkat_mobkit==0.8.48; extra == 'unused'",
                                         "meerkat.mobkit>=0.8"])
def test_no_legacy_sdk_dependency_even_under_an_unselected_extra(pair, index, requirement):
    wheel = pair[index]
    wheel.metadata["METADATA"] += ("Requires-Dist: " + requirement + "\n").encode()
    # Headers belong before the blank body separator.
    wheel.metadata["METADATA"] = wheel.metadata["METADATA"].replace(b"\n\nRequires-Dist", b"\nRequires-Dist")
    with pytest.raises(integrity.WheelIntegrityError, match="legacy"):
        check_pair(pair)


@pytest.mark.parametrize("mutation", ["hash", "size", "version", "distribution", "payload-hash", "payload-size",
                                      "metadata-hash", "metadata-size", "duplicate-payload", "missing-module"])
def test_caller_held_identity_and_complete_source_inventory_are_required(pair, mutation):
    content, pin = pair[1].selected()
    if mutation in ("hash", "size", "version", "distribution"):
        key, value = {"hash": ("sha256", "f" * 64), "size": ("size", pin.size + 1),
                      "version": ("version", "0.1.1"), "distribution": ("distribution", "other")}[mutation]
        pin = replace(pin, **{key: value})
    elif mutation.startswith("payload") or mutation.startswith("metadata"):
        field = "source_payload" if mutation.startswith("payload") else "metadata"
        rows = list(getattr(pin, field))
        name, hashed, size = rows[0]
        rows[0] = (name, "f" * 64, size) if mutation.endswith("hash") else (name, hashed, size + 1)
        pin = replace(pin, **{field: tuple(rows)})
    elif mutation == "duplicate-payload":
        pin = replace(pin, source_payload=pin.source_payload + (pin.source_payload[0],))
    else:
        pin = replace(pin, source_payload=tuple(row for row in pin.source_payload
                                               if not row[0].endswith("application_runtime.py")))
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair(pair, toolkit=(content, pin))


@pytest.mark.parametrize("mutation", ["unexpected", "path-hook", "traversal", "case-collision", "symlink",
                                      "duplicate", "record-missing", "record-bad-hash", "record-self-hash", "corrupt"])
def test_archives_reject_unsafe_or_unrecorded_content_even_when_whole_bytes_are_pinned(pair, mutation):
    wheel = pair[1]
    values = {**wheel.payload, **{wheel.prefix + name: value for name, value in wheel.metadata.items()}}
    source = dict(wheel.payload)
    kwargs = {}
    if mutation in ("unexpected", "path-hook", "traversal", "case-collision"):
        name = {"unexpected": "other/__init__.py", "path-hook": "meerkat_toolkit/startup.pth",
                "traversal": "meerkat_toolkit/../escape.py",
                "case-collision": "meerkat_toolkit/HOST_CONTROL.py"}[mutation]
        values[name] = b"forbidden"
        if mutation != "unexpected":
            source[name] = values[name]
    elif mutation in ("symlink", "duplicate"):
        kwargs["link" if mutation == "symlink" else "duplicate"] = "meerkat_toolkit/host_control.py"
    elif mutation.startswith("record"):
        kwargs["record"] = {"record-missing": b"", "record-bad-hash": b"bad,sha256=wrong,1\n",
                            "record-self-hash": (wheel.prefix + "RECORD,sha256=bad,0\n").encode()}[mutation]
    selected = wheel.selected(values=values, source=source, **kwargs)
    if mutation == "corrupt":
        content = b"not a wheel"
        selected = content, replace(selected[1], sha256=digest(content), size=len(content))
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair(pair, toolkit=selected)


@pytest.mark.parametrize("mutation", ["duplicate-name", "duplicate-version", "tag", "top-level", "license-omitted",
                                      "license-extra", "duplicate-license", "native-extra-omitted", "duplicate-meta-dependency"])
def test_producer_metadata_and_licenses_are_exact(pair, mutation):
    wheel = pair[1]
    if mutation in ("duplicate-name", "duplicate-version", "license-extra", "duplicate-license",
                    "duplicate-meta-dependency"):
        header = {"duplicate-name": "Name: meerkat-toolkit", "duplicate-version": "Version: " + wheel.version,
                  "license-extra": "License-File: OTHER", "duplicate-license": "License-File: LICENSE-MIT",
                  "duplicate-meta-dependency": 'Requires-Dist: meerkat-meta==0.9.0; extra == "native-host"'}[mutation]
        wheel.metadata["METADATA"] = wheel.metadata["METADATA"].replace(b"\n\n", ("\n" + header + "\n\n").encode())
    elif mutation == "tag":
        wheel.metadata["WHEEL"] = wheel.metadata["WHEEL"].replace(b"py3-none-any", b"cp312-none-any")
    elif mutation == "top-level":
        wheel.metadata["top_level.txt"] = b"meerkat_mobkit\n"
    elif mutation == "license-omitted":
        wheel.metadata.pop("licenses/LICENSE-MIT")
    else:
        wheel.metadata["METADATA"] = wheel.metadata["METADATA"].replace(b"Provides-Extra: native-host\n", b"")
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair(pair)


def test_modern_license_inventory_does_not_require_optional_top_level_file(pair):
    pair[0].metadata.pop("top_level.txt")
    assert check_pair(pair)[0].version == "0.9.0"


@pytest.mark.parametrize("bound,value", [("MAX_WHEEL_BYTES", 8), ("MAX_WHEEL_TOTAL", 8), ("MAX_WHEEL_FILES", 1)])
def test_selected_pair_obeys_existing_finite_wheel_bounds(pair, monkeypatch, bound, value):
    monkeypatch.setattr(integrity, bound, value)
    with pytest.raises(integrity.WheelIntegrityError):
        check_pair(pair)


def test_legacy_wheel_reuses_shared_reader_and_record_checker_with_legacy_bounds(monkeypatch):
    wheel = InertWheel("meerkat-meta")
    version = "0.8.48"
    prefix = f"meerkat_mobkit-{version}.dist-info/"
    payload = {"meerkat_mobkit/__init__.py": b"raise RuntimeError('inert legacy SDK')\n"}
    values = {**payload, prefix + "METADATA": b"Name: meerkat-mobkit\nVersion: 0.8.48\n\n",
              prefix + "WHEEL": wheel.metadata["WHEEL"], prefix + "top_level.txt": b"meerkat_mobkit\n"}
    content = archive_bytes(values, prefix)
    observed = []
    original_reader, original_record = runtime_candidates.read_wheel_members, runtime_candidates.verify_wheel_record
    def reader(*args, **kwargs):
        observed.append(("read", kwargs))
        return original_reader(*args, **kwargs)
    def record(*args):
        observed.append(("record", args[1]))
        return original_record(*args)
    monkeypatch.setattr(runtime_candidates, "read_wheel_members", reader)
    monkeypatch.setattr(runtime_candidates, "verify_wheel_record", record)
    assert runtime_candidates._wheel(content, version, {name: digest(value) for name, value in payload.items()},
                                     {name: len(value) for name, value in payload.items()}) == tuple(
                                         sorted((name, digest(value)) for name, value in payload.items()))
    assert [row[0] for row in observed] == ["read", "record"]
    assert observed[0][1]["max_file_bytes"] == runtime_candidates.MAX_WHEEL_BYTES
    assert observed[1][1] == prefix + "RECORD"
