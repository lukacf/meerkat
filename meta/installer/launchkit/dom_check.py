"""Host-owned offline DOM execution, never a browser or a JavaScript security sandbox.

Only exact pinned jsdom package bytes are copied into the sandbox's read-only
dependency directory. The OS sandbox, not jsdom's VM, denies host I/O and forks.
No package manager or authored command is invoked.
"""
from __future__ import annotations

import hashlib
import hmac
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat

from .checker_runtime import CheckerRuntimeError, resolve_node

DOM_SUFFIX = ".dom.cjs"
DOM_RESULT_MARKER = "MOBKIT_DOM_RESULT="
SUPPORTED_NODE_VERSION = "25.9.0"
DOM_SCOPE = "Offline DOM scripts, events and storage; not browser form encoding, navigation, layout, authentication, native or installed proof."
MAX_DEPENDENCY_FILES = 2048
MAX_DEPENDENCY_BYTES = 16 * 1024 * 1024
# Host-reviewed installed dependency closure, including nested package resolution.
# Versions alone do not authenticate JavaScript that will be granted file reads.
DEPENDENCIES = {
  "node_modules/@asamuzakjp/css-color": {
    "bytes": 2097817,
    "files": 58,
    "sha256": "a0c12903ef6ccc9f806f30f86b81196953fb981226c3f49888b0125079798d17",
    "version": "3.2.0"
  },
  "node_modules/@csstools/color-helpers": {
    "bytes": 80870,
    "files": 7,
    "sha256": "a5bd48c76c4277059a1b6aa52ad20db20f9f6747eb0ebe3aae5a6b71be7ab425",
    "version": "5.1.0"
  },
  "node_modules/@csstools/css-calc": {
    "bytes": 71012,
    "files": 7,
    "sha256": "44343427b8ff9cb9a31cacb3e7119e614794163ce0907fc1c6efe5c9d6064862",
    "version": "2.1.4"
  },
  "node_modules/@csstools/css-color-parser": {
    "bytes": 105150,
    "files": 7,
    "sha256": "365d7c1de46174211437d8cf9863601182e224a52737687a0906b9246473ebd0",
    "version": "3.1.0"
  },
  "node_modules/@csstools/css-parser-algorithms": {
    "bytes": 45381,
    "files": 7,
    "sha256": "277fc1ef442360569eaaf41238d3e4ec92ece0f4468c92b9e70c522f9aae0717",
    "version": "3.0.5"
  },
  "node_modules/@csstools/css-tokenizer": {
    "bytes": 72983,
    "files": 7,
    "sha256": "f7d89a6911671bf9662d17a65141191855c8486b93fbc89f0b1220747f61167b",
    "version": "3.0.4"
  },
  "node_modules/agent-base": {
    "bytes": 23962,
    "files": 11,
    "sha256": "6e2e0e0d5d4376645372dc7ea18deaad23b3a7a9fb3c2ec289cd219147c22b52",
    "version": "7.1.4"
  },
  "node_modules/asynckit": {
    "bytes": 27362,
    "files": 20,
    "sha256": "7429c7577c94f7b5547387cb5111deb6e84cbdc60a16349f53530757883acdc6",
    "version": "0.4.0"
  },
  "node_modules/call-bind-apply-helpers": {
    "bytes": 15952,
    "files": 21,
    "sha256": "6be2e78c89df33f924969ce618fc5217c3ace438d3c703cccd0d24a41bc0cec4",
    "version": "1.0.2"
  },
  "node_modules/combined-stream": {
    "bytes": 11514,
    "files": 5,
    "sha256": "ec0edb392855cd76f9e1316fcf4a2fd9776aee1cd658a82aa876ec20fde23d72",
    "version": "1.0.8"
  },
  "node_modules/cssstyle": {
    "bytes": 216766,
    "files": 86,
    "sha256": "4a4ea85f25eaccfa86516add21ed8e64f33a07275cef2ad4e8c803ac3d493e0c",
    "version": "4.6.0"
  },
  "node_modules/cssstyle/node_modules/rrweb-cssom": {
    "bytes": 105478,
    "files": 30,
    "sha256": "7dd0d82c77f714567977aeee64ac639b05d84cec3e1b9daf4f95d61f766bfd34",
    "version": "0.8.0"
  },
  "node_modules/data-urls": {
    "bytes": 7834,
    "files": 5,
    "sha256": "d0c30662ccdfb7c3c0ab03878a421f380628f28ec71a8739eb8f5595cb2be768",
    "version": "5.0.0"
  },
  "node_modules/debug": {
    "bytes": 42793,
    "files": 7,
    "sha256": "b1aeefeb04f4dc5c955d4964958c377c300d36c3488ef5c9829486cc1d3aee80",
    "version": "4.4.3"
  },
  "node_modules/decimal.js": {
    "bytes": 284347,
    "files": 6,
    "sha256": "7e98209cc37afdbba8f1ac6ccc0db92b462d0c348eebe1b3869765b6e2b4d92b",
    "version": "10.6.0"
  },
  "node_modules/delayed-stream": {
    "bytes": 8021,
    "files": 6,
    "sha256": "65ff9eec681abdb4b38018d93ce3e7c1318785354e21079f7d47c77340294753",
    "version": "1.0.0"
  },
  "node_modules/dunder-proto": {
    "bytes": 13003,
    "files": 15,
    "sha256": "7b08d3bb2dbaf091ec64e549ed6e7f045d5687f9231737a3e05d8af896c28a4c",
    "version": "1.0.1"
  },
  "node_modules/entities": {
    "bytes": 540551,
    "files": 86,
    "sha256": "31469cd88cc09fb2db3e9b821232d759dbe99b08a445eb74475449827dc38859",
    "version": "6.0.1"
  },
  "node_modules/es-define-property": {
    "bytes": 10217,
    "files": 11,
    "sha256": "c92ddb9e837a718a3083fb35f5bf5d360e401dadd967469dff1456213c2254f5",
    "version": "1.0.1"
  },
  "node_modules/es-errors": {
    "bytes": 12324,
    "files": 22,
    "sha256": "07b5d2f090b9717ad0c7e21c470f68fe3a717ebf34b5e6478ec67398befe2f3a",
    "version": "1.3.0"
  },
  "node_modules/es-object-atoms": {
    "bytes": 11442,
    "files": 16,
    "sha256": "689e2d2a48ce60244dcea5afcd0251156c2d7362e8b37960e098715acf4bd962",
    "version": "1.1.1"
  },
  "node_modules/es-set-tostringtag": {
    "bytes": 14544,
    "files": 10,
    "sha256": "0602bc854882619a5eac9e233bdbc8fc1eef0f90e82e31560dd4217d700d9170",
    "version": "2.1.0"
  },
  "node_modules/form-data": {
    "bytes": 81211,
    "files": 8,
    "sha256": "438b4d6765230f77e9209497f92b3f0301aadc07885d4830f2b064e3797b438a",
    "version": "4.0.5"
  },
  "node_modules/function-bind": {
    "bytes": 31427,
    "files": 12,
    "sha256": "6f45157f1d22e9d35481cad8d8abdc884550a7033afa88640f6282a4e881e09e",
    "version": "1.1.2"
  },
  "node_modules/get-intrinsic": {
    "bytes": 46542,
    "files": 9,
    "sha256": "0ade266015b0e4f7da1c2309ddc4dbdebd56740f9dfc56d2633ce8cdcb1c82e1",
    "version": "1.3.0"
  },
  "node_modules/get-proto": {
    "bytes": 10840,
    "files": 15,
    "sha256": "8d34a531115afd07047e37d70fa2dbb7df4a3e795308b88388fde3700fd4da90",
    "version": "1.0.1"
  },
  "node_modules/gopd": {
    "bytes": 9869,
    "files": 12,
    "sha256": "f2bd4695fbdd8f638c38b303190a37a077b714675cc2cf483668319baae20095",
    "version": "1.2.0"
  },
  "node_modules/has-symbols": {
    "bytes": 23409,
    "files": 16,
    "sha256": "06a696fceec54b6996bf69e92e207a2a57a2b2f61ecfb9ca01b167afe5d53a0c",
    "version": "1.1.0"
  },
  "node_modules/has-tostringtag": {
    "bytes": 17603,
    "files": 16,
    "sha256": "aa52bd1e743f1dceed896795ae65d9b798528270e8c26c0da2f6fa26fbe5f3c8",
    "version": "1.0.2"
  },
  "node_modules/hasown": {
    "bytes": 8765,
    "files": 10,
    "sha256": "f872679aceb057cb222c7e4208ec2fad0eca2127ce3741686bb08e49742bdc21",
    "version": "2.0.2"
  },
  "node_modules/html-encoding-sniffer": {
    "bytes": 11662,
    "files": 4,
    "sha256": "dc26f05456b9126a90ecfe8f61161afd45627ad2a84ac4351aa9d8f9f0028b75",
    "version": "4.0.0"
  },
  "node_modules/http-proxy-agent": {
    "bytes": 17260,
    "files": 7,
    "sha256": "a6768cee458ac2161b1dcca47ad0ab1b54dc42f47781c3ef1381199c3bc1bbb7",
    "version": "7.0.2"
  },
  "node_modules/https-proxy-agent": {
    "bytes": 27706,
    "files": 11,
    "sha256": "569da3d11cb9bdcb33ade94d0a2b290f0e9669563a44eaf58f4af420a3aa9e82",
    "version": "7.0.6"
  },
  "node_modules/iconv-lite": {
    "bytes": 348518,
    "files": 33,
    "sha256": "cf2c4ccf59825bbeae7b336236ff64814e97fd7fe2d725dbd64ede21eaa5ff62",
    "version": "0.6.3"
  },
  "node_modules/is-potential-custom-element-name": {
    "bytes": 3918,
    "files": 4,
    "sha256": "09129786e56615439c971772101e9c33cf85215b46867af88229813e0aee8f02",
    "version": "1.0.1"
  },
  "node_modules/jsdom": {
    "bytes": 3110529,
    "files": 481,
    "sha256": "f4ad8539ff0bae139d36c1224648f4a099fc0ff2b678892d2ac311d87b0d48e9",
    "version": "24.1.0"
  },
  "node_modules/lru-cache": {
    "bytes": 804301,
    "files": 17,
    "sha256": "87c40521497618dadb5e5aba505c99aeac110a0f6a16030bf916687bb14eae86",
    "version": "10.4.3"
  },
  "node_modules/math-intrinsics": {
    "bytes": 17323,
    "files": 38,
    "sha256": "11001dd2b3632ff3e6f1ce4211ccae15b8b565307ac53b25c3aba6776565f771",
    "version": "1.1.0"
  },
  "node_modules/mime-db": {
    "bytes": 205539,
    "files": 6,
    "sha256": "244fd17ea76b3304bce08e9bfbd321e1fc4fc6ccb4cdea8cfb25d40467ead1ad",
    "version": "1.52.0"
  },
  "node_modules/mime-types": {
    "bytes": 18272,
    "files": 5,
    "sha256": "387bf4af8ba45ba5df831e7a73b36c6c49d14e3f4fc77444ff46e8ff3b3cd5e9",
    "version": "2.1.35"
  },
  "node_modules/ms": {
    "bytes": 6721,
    "files": 4,
    "sha256": "84b25a9f103ac8d0087d5105a523b4c5b5158b1d1b1074183aaa527a391516a7",
    "version": "2.1.3"
  },
  "node_modules/nwsapi": {
    "bytes": 86544,
    "files": 7,
    "sha256": "d5abe465b061946a936848524c04a0dec256798aa21ac94d8775a9dae8e9f02d",
    "version": "2.2.23"
  },
  "node_modules/parse5": {
    "bytes": 695873,
    "files": 64,
    "sha256": "082b6986b9a7faac310901969a321b0ce9c41c0f720e84e949be62b45f27b9c4",
    "version": "7.3.0"
  },
  "node_modules/psl": {
    "bytes": 711982,
    "files": 14,
    "sha256": "242bb8ddc9a3d67995adaa4d3b4195c6c3ab23127e779d05c5c29f017ed20936",
    "version": "1.15.0"
  },
  "node_modules/punycode": {
    "bytes": 33514,
    "files": 5,
    "sha256": "cec5ed6f68fb852fb0be10fc78885fcb39420cd8c064d5cb7ff11471dd730aa8",
    "version": "2.3.1"
  },
  "node_modules/querystringify": {
    "bytes": 6959,
    "files": 4,
    "sha256": "aff47e71d53ced435eb8daec44979c63c2f128866ff9e4236170e76204e447ff",
    "version": "2.2.0"
  },
  "node_modules/requires-port": {
    "bytes": 8565,
    "files": 7,
    "sha256": "f6d5b692897a67f54bacdb5639e7d8830b4955fecc0a7a26307818a0708bb7bb",
    "version": "1.0.0"
  },
  "node_modules/rrweb-cssom": {
    "bytes": 101567,
    "files": 29,
    "sha256": "34861a35f31f778e204df10aec73e476130156a139c9cf135d5b3318c541e02a",
    "version": "0.7.1"
  },
  "node_modules/safer-buffer": {
    "bytes": 42299,
    "files": 7,
    "sha256": "87f39522c6e433d42d62addd1ba7350456154e20cc8cd16f82feae9dcf4d3bbe",
    "version": "2.1.2"
  },
  "node_modules/saxes": {
    "bytes": 163774,
    "files": 5,
    "sha256": "9a62477650092294d9932cbf7fdd376c1e0130f83603d215126f310ad8e63fc6",
    "version": "6.0.0"
  },
  "node_modules/symbol-tree": {
    "bytes": 57073,
    "files": 7,
    "sha256": "77729237eb771b10220b128afa87e5ed850fc5767fe5b06517430ff53585248a",
    "version": "3.2.4"
  },
  "node_modules/tough-cookie": {
    "bytes": 111787,
    "files": 12,
    "sha256": "afa590cc28ad3bea77dc79ddae5128b462bf360eae9f3463629a6aec0e09555f",
    "version": "4.1.4"
  },
  "node_modules/tr46": {
    "bytes": 225902,
    "files": 7,
    "sha256": "3e8c61c5e5466b3394a20646cb04a6c89bdadb9d1676a0e3e62f40c9276e12b8",
    "version": "5.1.1"
  },
  "node_modules/universalify": {
    "bytes": 4787,
    "files": 4,
    "sha256": "a9fed3dcf23d899133ee9c6827493e97809ee0050388cdbeedab303eaad36df9",
    "version": "0.2.0"
  },
  "node_modules/url-parse": {
    "bytes": 62957,
    "files": 7,
    "sha256": "b36728f4d286b798d36aa4323240cefdca4f0fd7dcd0d61eb455fba69772c268",
    "version": "1.5.10"
  },
  "node_modules/w3c-xmlserializer": {
    "bytes": 17973,
    "files": 6,
    "sha256": "15eba5cdb51bc602de42c33eb171ea4d73f36ab06a330721b2586533f60319af",
    "version": "5.0.0"
  },
  "node_modules/webidl-conversions": {
    "bytes": 24242,
    "files": 4,
    "sha256": "941f866f77b986a08c58f239f8a2f22c02f4d32fb1b75aadd23c1067ddd8caee",
    "version": "7.0.0"
  },
  "node_modules/whatwg-encoding": {
    "bytes": 13339,
    "files": 6,
    "sha256": "474c0540eae42f22f6c20bcaf7a4e599aba1e004022fee0f8ab4ceeedd598df8",
    "version": "3.1.1"
  },
  "node_modules/whatwg-mimetype": {
    "bytes": 16721,
    "files": 8,
    "sha256": "2494db3d1bd1e91d5ca549e4011dc54c62efa2d2f40e8662f9ee7021c7244dfa",
    "version": "4.0.0"
  },
  "node_modules/whatwg-url": {
    "bytes": 98558,
    "files": 17,
    "sha256": "990b5687a9bd0592ff0ffb07809c5860325ef37eebe01ba8d800429ecad2939b",
    "version": "14.2.0"
  },
  "node_modules/ws": {
    "bytes": 147844,
    "files": 19,
    "sha256": "99b813301591b65e6e44ce43c44df885fa9d29387a5a8e7c025ecd903f87a3b1",
    "version": "8.19.0"
  },
  "node_modules/xml-name-validator": {
    "bytes": 13574,
    "files": 4,
    "sha256": "4dfa8c9637eb7658f56fb226209b79247580ad1b3b302333e9b62eb1673759de",
    "version": "5.0.0"
  },
  "node_modules/xmlchars": {
    "bytes": 58957,
    "files": 18,
    "sha256": "c7063701b0b4982a246ffb1f3a91afbd1dd0e292cd56822f9dd0fc532e1580fd",
    "version": "2.2.0"
  }
}


class DOMUnavailable(ValueError):
    pass


def is_dom_test(path):
    return Path(path).name.startswith("test_") and path.endswith(DOM_SUFFIX)


def _package_files(folder):
    rows, contents, total = [], {}, 0
    for current, dirs, names in os.walk(folder, followlinks=False):
        dirs[:] = sorted(name for name in dirs if name != "node_modules")
        for name in dirs:
            if (Path(current) / name).is_symlink():
                raise DOMUnavailable("The pinned DOM dependencies contain a directory link.")
        for name in sorted(names):
            path = Path(current) / name
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                raise DOMUnavailable("The pinned DOM dependencies contain a file link.")
            if info.st_size > MAX_DEPENDENCY_BYTES or len(rows) >= MAX_DEPENDENCY_FILES:
                raise DOMUnavailable("The pinned DOM dependencies exceed their bounds.")
            content = path.read_bytes()
            if len(content) != info.st_size:
                raise DOMUnavailable("The pinned DOM dependencies changed during capture.")
            total += len(content)
            if total > MAX_DEPENDENCY_BYTES:
                raise DOMUnavailable("The pinned DOM dependencies exceed their bounds.")
            relative = path.relative_to(folder).as_posix()
            contents[relative] = (content, info.st_mode & 0o777)
            rows.append([relative, info.st_mode & 0o777, hashlib.sha256(content).hexdigest(), len(content)])
    # The reviewed manifest used Path's component ordering. A sibling x.js
    # sorts after directory x, unlike a flat string containing x/file.js.
    rows.sort(key=lambda row: PurePosixPath(row[0]).parts)
    digest = hashlib.sha256(json.dumps(rows, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return rows, contents, digest


def _dependency_source(root=None):
    root = Path(root) if root is not None else Path(__file__).resolve().parents[1]
    manifest = root / "standalone-manifest.json"
    if manifest.exists() or manifest.is_symlink():
        from .distribution import verify, DistributionError
        try:
            verify(root)
        except (DistributionError, OSError, ValueError, KeyError, TypeError):
            raise DOMUnavailable("The standalone checker dependencies could not be verified. Unpack a fresh bundle.") from None
        return root / "checker-dependencies"
    return root.parent / "console"


def prepare_dependencies(staging, *, source=None):
    """Copy only the authenticated closure; never expose its installation parent."""
    base = Path(source) if source is not None else _dependency_source()
    destination = Path(staging) / "dom-dependencies"
    if destination.exists():
        raise DOMUnavailable("DOM dependency staging must be fresh.")
    captured, total_bytes, total_files = {}, 0, 0
    try:
        for relative, pin in DEPENDENCIES.items():
            folder = base / relative
            for parent in [folder, *folder.parents]:
                if parent.is_symlink():
                    raise DOMUnavailable("The pinned DOM dependency path contains a link.")
                if parent == base:
                    break
            if not folder.is_dir():
                raise DOMUnavailable("Pinned jsdom 24 dependencies are unavailable.")
            rows, contents, digest = _package_files(folder)
            total_bytes += sum(row[3] for row in rows)
            total_files += len(rows)
            if (digest != pin["sha256"] or len(rows) != pin["files"]
                    or sum(row[3] for row in rows) != pin["bytes"]
                    or total_bytes > MAX_DEPENDENCY_BYTES or total_files > MAX_DEPENDENCY_FILES):
                raise DOMUnavailable("Pinned jsdom 24 dependency bytes do not match the host manifest.")
            metadata = json.loads(contents["package.json"][0].decode("utf-8"))
            if metadata.get("version") != pin["version"]:
                raise DOMUnavailable("Pinned jsdom 24 dependency versions do not match.")
            captured[relative] = contents
    except (OSError, ValueError, KeyError, UnicodeError) as error:
        if isinstance(error, DOMUnavailable):
            raise
        raise DOMUnavailable("Pinned jsdom 24 dependencies could not be authenticated.") from None
    destination.mkdir(mode=0o700)
    for relative, contents in captured.items():
        for name, (content, mode) in contents.items():
            target = destination / relative / name
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            target.write_bytes(content)
            target.chmod(mode & 0o555)
    manifest_digest = hashlib.sha256(json.dumps(DEPENDENCIES, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return destination, manifest_digest


def execution_result(raw, key, selected):
    """Authenticate completion before granting count or per-file test authority."""
    result = {**raw, "tests_run": 0, "test_results": []}
    lines = raw.get("detail", "").splitlines()
    # Any unsigned, malformed or duplicate marker fails closed, even exit 0.
    completions = [line for line in lines if line.startswith(DOM_RESULT_MARKER)]
    clean = "\n".join(line for line in lines if not line.startswith(DOM_RESULT_MARKER))
    reason = "missing completion marker" if not completions else "duplicate completion markers"
    try:
        if len(completions) != 1:
            raise ValueError("missing or duplicate completion")
        reason = "malformed completion marker"
        signature, payload = completions[0][len(DOM_RESULT_MARKER):].split("=", 1)
        reason = "unauthenticated completion marker"
        expected = hmac.digest(key, payload.encode("utf-8"), "sha256").hex()
        if len(signature) != 64 or not hmac.compare_digest(signature, expected):
            raise ValueError("unauthenticated completion")
        reason = "invalid JSON or schema"
        value = json.loads(payload)
        rows = value["test_results"]
        if set(value) != {"tests_run", "test_results"}:
            raise ValueError("invalid test schema")
        reason = "invalid counts or ownership"
        if (type(value["tests_run"]) is not int or not 0 < value["tests_run"] <= 1000
                or not isinstance(rows, list) or len(rows) != value["tests_run"]
                or any(not isinstance(row, dict) or set(row) != {"id", "file", "status", "detail"}
                       or row.get("file") not in selected or not isinstance(row.get("id"), str)
                       or not row["id"].startswith(row["file"] + "::")
                       or len(row["id"]) > len(row["file"]) + 202
                       or row.get("status") not in ("pass", "fail", "error", "skipped")
                       or not isinstance(row.get("detail"), str) or len(row["detail"]) > 4000 for row in rows)
                or len({row["id"] for row in rows}) != len(rows)
                or {row["file"] for row in rows} != set(selected)
                or not any(row["status"] != "skipped" for row in rows)):
            raise ValueError("invalid test counts or ownership")
        result.update(tests_run=value["tests_run"], test_results=rows)
        if any(row["status"] in ("fail", "error") for row in rows):
            result["status"] = "fail"
        result["detail"] = clean + "\nHost DOM harness authenticated completion."
    except (KeyError, TypeError, ValueError, UnicodeError):
        if raw["status"] != "unavailable":
            result["status"] = "fail"
        result["detail"] = "No authenticated nonempty DOM test completion.\nRejection: " + reason + ".\n" + clean
    return result


def _node_permissions(read_paths, scratch):
    # Permission mode supplements the OS sandbox. It is not a JS/VM isolation
    # claim. Exact roots exclude the original node_modules and checkout.
    paths = [str(Path(path).resolve()) for path in read_paths]
    if any(any(character in path for character in ("*", ",", "\n", "\r")) for path in [*paths, str(scratch)]):
        raise DOMUnavailable("DOM permission paths cannot be expressed exactly.")
    return ["--permission", *["--allow-fs-read=" + path for path in paths],
            "--allow-fs-write=" + str(Path(scratch).resolve())]


def run_dom_tests(selected, snapshot, test_root, scratch, staging, execute):
    """execute is the host's bounded sandbox launcher, not a project callback."""
    try:
        node = resolve_node(shutil.which("node"))
    except CheckerRuntimeError as error:
        return {"status": "unavailable", "detail": str(error), "tests_run": 0, "test_results": []}
    if not node:
        return {"status": "unavailable", "detail": "Node.js is unavailable for requested DOM tests. On macOS, run .venv/bin/python -m launchkit.checker_runtime before building.",
                "tests_run": 0, "test_results": []}
    try:
        dependencies, digest = prepare_dependencies(staging)
        runner = Path(staging) / "host-dom-check.cjs"
        runner.write_text(DOM_HARNESS, encoding="utf-8")
        runner.chmod(0o400)
        reads = [runner, dependencies, Path(node).resolve(), test_root, snapshot, scratch]
        flags = _node_permissions(reads, scratch)
    except DOMUnavailable as error:
        return {"status": "unavailable", "detail": str(error), "tests_run": 0, "test_results": []}
    # Unsupported flags or native access controls never launch a weaker runner.
    probe = execute([str(Path(node).resolve()), *flags, "-e", NODE_PREFLIGHT], reads)
    if probe["status"] != "pass" or probe.get("detail", "").strip() != "DOM_PERMISSION_READY":
        reason = ("DOM checks require the reviewed Node.js " + SUPPORTED_NODE_VERSION + " runtime."
                  if "DOM_NODE_VERSION_UNSUPPORTED" in probe.get("detail", "")
                  else "Required Node permission controls are unavailable for DOM checks.")
        return {"status": "unavailable", "detail": reason,
                "tests_run": 0, "test_results": []}
    key = os.urandom(32)
    raw = execute([str(Path(node).resolve()), *flags, "--max-old-space-size=256", str(runner),
                   str(snapshot), str(test_root), str(scratch), str(dependencies), *selected], reads, key)
    result = execution_result(raw, key, selected)
    result["detail"] += "\n" + DOM_SCOPE + "\nPinned jsdom 24.1.0 dependency manifest: " + digest
    return result


NODE_PREFLIGHT = r'''"use strict";
if (process.versions.node !== "25.9.0") { process.stdout.write("DOM_NODE_VERSION_UNSUPPORTED\n"); process.exit(1); }
if (!process.permission || ["inspector","addons","child","worker","wasi","net"].some(name => process.permission.has(name))) process.exit(1);
try { process.binding("heap_utils"); process.exit(1); } catch (error) { if (error.code !== "ERR_ACCESS_DENIED") process.exit(1); }
try { new (require("node:inspector").Session)().connect(); process.exit(1); } catch (error) { if (error.code !== "ERR_ACCESS_DENIED") process.exit(1); }
process.stdout.write("DOM_PERMISSION_READY\n");'''


DOM_HARNESS = r'''"use strict";
const fs = require("node:fs"), path = require("node:path"), crypto = require("node:crypto");
const assert = require("node:assert/strict"), v8 = require("node:v8");
// Capture completion primitives before any authored module can replace them.
const apply = Reflect.apply, make = Object.create, define = Object.defineProperty;
const keys = Object.keys, freeze = Object.freeze, proto = Object.getPrototypeOf, setProto = Object.setPrototypeOf;
const quote = JSON.stringify, string = String, trim = String.prototype.trim, slice = String.prototype.slice;
const read = fs.readSync, close = fs.closeSync, write = fs.writeSync;
// Initialize libuv's lazy output streams while stdin still occupies fd 0.
// Otherwise the pinned Darwin build can duplicate an output descriptor into
// the closed stdin slot and abort when authored code first opens stdout.
void process.stdout; void process.stderr;
const alloc = Buffer.alloc, fill = Buffer.prototype.fill, byteLength = Buffer.byteLength;
const secretKey = crypto.createSecretKey, createHmac = crypto.createHmac;
const selected = make(null);
const sourceRoot = process.argv[2], testRoot = process.argv[3], scratchRoot = process.argv[4], dependencyRoot = process.argv[5];
const selectedCount = process.argv.length - 6;
for (let i=0;i<selectedCount;i++) selected[i]=process.argv[i+6];
// The owned stdin is consumed and closed before authors run. No string encoding
// or raw key Buffer remains; the only retained key is a native KeyObject.
let signingKey;
const input = alloc(32), extra = alloc(1);
try {
  let offset=0;
  while(offset<32){const n=read(0,input,offset,32-offset,null);if(!n)throw new Error("Host input incomplete");offset+=n;}
  if(read(0,extra,0,1,null)!==0)throw new Error("Host input exceeds bound");
  signingKey=secretKey(input);
} finally { apply(fill,input,[0]); apply(fill,extra,[0]); close(0); }
// Allowed heap queries can invoke custom inspection with a live native key.
// Remove those exact APIs (including named ESM aliases), then protect only the
// key-specific prototypes. This does not turn authored JavaScript into a sandbox.
const denyHeap = () => { throw new Error("Heap inspection is unavailable in host DOM checks."); };
for(const name of ["queryObjects","getHeapSnapshot","writeHeapSnapshot","setFlagsFromString","setHeapSnapshotNearHeapLimit"])
  define(v8,name,{value:denyHeap,writable:false,configurable:false});
require("node:module").syncBuiltinESMExports();
for(let p=proto(signingKey);p && p!==Object.prototype;p=proto(p)) freeze(p);
// Native secret preparation also probes a missing private symbol. Detach only
// this private native instance so an authored Object.prototype getter cannot
// receive it. The immutable own type and native key handle remain intact.
define(signingKey,"type",{value:"secret",writable:false,configurable:false});
setProto(signingKey,null);freeze(signingKey);
// The public Hmac methods delegate through a mutable native handle prototype.
// Initialize before authors, capture the actual native methods, and detach only
// our private handle. The exact reviewed Node version is checked by the host.
const nativeMac = (() => {
  const wrapper=createHmac("sha256",signingKey);
  const symbols=Object.getOwnPropertySymbols(wrapper);
  for(let i=0;i<symbols.length;i++)if(symbols[i].description==="kHandle")return wrapper[symbols[i]];
  throw new Error("Reviewed native MAC handle is unavailable.");
})();
const update=nativeMac.update, digest=nativeMac.digest;
if(typeof update!=="function" || typeof digest!=="function")throw new Error("Reviewed native MAC methods are unavailable.");
setProto(nativeMac,null);freeze(nativeMac);signingKey=null;
const {JSDOM, VirtualConsole} = require(path.join(dependencyRoot,"node_modules/jsdom"));
const observed = make(null), windows = make(null);
let observedCount=0, windowCount=0;
const maxCases=1000;
const shorten = value => apply(slice,string(value),[0,4000]);
const errorText = error => { try { return shorten(error && error.stack || error); } catch { return "DOM case failed."; } };
const record = (file,id,status,detail) => {
  const row=make(null);row.file=file;row.id=id;row.status=status;row.detail=detail;
  observed[observedCount++]=row;
};
const closeWindows = () => {
  for(let i=0;i<windowCount;i++){try{windows[i].window.close();}catch{}}
  windowCount=0;
};
async function main(){
  for(let fileIndex=0;fileIndex<selectedCount;fileIndex++){
    const file=selected[fileIndex], cases=make(null), names=make(null);
    let caseCount=0;
    const test=(name,fn,options={})=>{
      const optionKeys=keys(options);
      if(typeof name!=="string" || !apply(trim,name,[]) || name.length>200 || names[name]
          || typeof fn!=="function" || caseCount+observedCount>=maxCases)
        throw new Error("Register unique bounded named DOM test functions.");
      for(let i=0;i<optionKeys.length;i++)if(optionKeys[i]!=="skip")throw new Error("Only skip is a DOM test option.");
      if(typeof(options.skip??false)!=="boolean")throw new Error("skip must be a boolean.");
      names[name]=true;
      const entry=make(null);entry.name=name;entry.fn=fn;entry.skip=options.skip===true;
      cases[caseCount++]=entry;
    };
    const createDOM=(html,options={})=>{
      if(typeof html!=="string" || byteLength(html,"utf8")>1000000)throw new Error("DOM fixture HTML exceeds its UTF-8 bound.");
      const optionKeys=keys(options);
      for(let i=0;i<optionKeys.length;i++)if(optionKeys[i]!=="url" && optionKeys[i]!=="beforeParse")throw new Error("DOM fixture options are url and beforeParse only.");
      const virtualConsole=new VirtualConsole(), entry=make(null);
      entry.errors=make(null);entry.count=0;
      virtualConsole.on("jsdomError",error=>{if(error.type==="unhandled exception")entry.errors[entry.count++]=error;});
      const dom=new JSDOM(html,{url:options.url||"https://offline.invalid/",runScripts:"dangerously",
        pretendToBeVisual:true,storageQuota:100000,virtualConsole,beforeParse:options.beforeParse});
      entry.window=dom.window;windows[windowCount++]=entry;return dom;
    };
    try{
      const register=require(path.resolve(testRoot,file));
      if(typeof register!=="function")throw new Error("A DOM test file exports one registration function.");
      const api=make(null);api.test=test;api.createDOM=createDOM;api.assert=assert;api.sourceRoot=sourceRoot;api.scratchRoot=scratchRoot;
      await register(freeze(api));
      if(!caseCount)throw new Error("The DOM test file registered no tests.");
    }catch(error){record(file,file+"::registration","error",errorText(error));closeWindows();continue;}
    for(let i=0;i<caseCount;i++){
      const entry=cases[i];let status=entry.skip?"skipped":"pass",detail="";
      try{
        if(!entry.skip)await entry.fn();
        for(let j=0;j<windowCount;j++)if(windows[j].count)throw windows[j].errors[0];
      }catch(error){status="fail";detail=errorText(error);}
      finally{closeWindows();}
      record(file,file+"::"+entry.name,status,detail);
    }
  }
  // Serialize private primitive rows manually; authored toJSON/prototype hooks
  // never receive the result collection, key, signature or final wire string.
  let body='{"tests_run":'+string(observedCount)+',"test_results":[';
  let failed=false;
  for(let i=0;i<observedCount;i++){
    const row=observed[i];if(i)body+=",";
    body+='{"id":'+quote(row.id)+',"file":'+quote(row.file)+',"status":'+quote(row.status)+',"detail":'+quote(row.detail)+'}';
    if(row.status==="fail" || row.status==="error")failed=true;
  }
  body+="]}";
  apply(update,nativeMac,[body,"utf8"]);
  const signature=apply(digest,nativeMac,["hex"]);
  write(1,"MOBKIT_DOM_RESULT="+signature+"="+body+"\n");
  process.exitCode=failed?1:0;
}
main().catch(()=>{closeWindows();write(2,"Host DOM harness failed without completion.\n");process.exitCode=1;});
'''
