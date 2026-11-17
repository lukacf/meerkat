"""A stand-in for `gh` as the upload call of the release tooling meets it. Never GitHub, no network.

The tests put a `gh` first on PATH that starts this file with a record directory and then gh's own
arguments. It answers one call, the upload of a release asset (`gh api <an upload address> --method
POST ...`), the way the installed GitHub CLI and the upload host answer it:

- the request carries the length of its body only when that length is known before the body is
  read: a `Content-Length` header states it, or `--input` names a file, whose size it is. A body
  from standard input with no such header has no length, and neither has an empty body in any form;
- the upload host refuses a request that carries no length: `gh: Bad Content-Length (HTTP 400)` on
  standard error, exit status 1, and nothing is stored;
- a body that is not as long as the request states fails in transport: exit status 1, nothing stored.

Every call adds one line to `requests.jsonl` in the record directory: the arguments, the length the
request carried (null: none), the outcome, and for an accepted body its name, size and sha256. The
body is read one block at a time. It is kept as `body-<name>` unless the record directory holds a
file named `no-bodies`.
"""
import hashlib
import json
import os
import sys
import urllib.parse

BLOCK = 1 << 20


def main(arguments):
    directory, arguments = arguments[0], arguments[1:]
    record = {"argv": arguments, "length": None, "outcome": None}

    def end(outcome, status, out=b"", err=b""):
        record["outcome"] = outcome
        with open(os.path.join(directory, "requests.jsonl"), "a", encoding="utf-8") as stream:
            stream.write(json.dumps(record, sort_keys=True) + "\n")
        os.write(1, out)
        os.write(2, err)
        return status

    options, position = {"--header": [], "--input": [], "--method": [], "--hostname": []}, 1
    upload = len(arguments) >= 2 and arguments[0] == "api" and "/assets?name=" in arguments[1]
    while upload and position + 1 < len(arguments):
        position += 1
        option = {"-H": "--header", "-X": "--method"}.get(arguments[position], arguments[position])
        if option not in options or position + 1 >= len(arguments):
            upload = False
            break
        position += 1
        options[option].append(arguments[position])
    if not upload or options["--method"] != ["POST"] or len(options["--input"]) != 1:
        return end("not an upload call", 97, err=b"stand-in gh: not an upload call\n")
    source = options["--input"][0]
    # As the GitHub CLI does: a named file's size first, then the headers in the order given.
    length = None if source == "-" else os.stat(source).st_size
    for header in options["--header"]:
        name, _, value = header.partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    record["length"] = length or None
    if not length:
        return end("refused: the request carried no length", 1, out=b'{"message":"Bad Content-Length"}\n',
                   err=b"gh: Bad Content-Length (HTTP 400)\n")
    name = urllib.parse.parse_qs(urllib.parse.urlsplit(arguments[1]).query)["name"][0]
    keep = not os.path.exists(os.path.join(directory, "no-bodies"))
    kept = os.path.join(directory, "body-" + name)
    descriptor = 0 if source == "-" else os.open(source, os.O_RDONLY)
    sha, size = hashlib.sha256(), 0
    with open(kept if keep else os.devnull, "wb") as store:
        while size <= length:
            block = os.read(descriptor, BLOCK)
            if not block:
                break
            sha.update(block)
            size += len(block)
            store.write(block)
    if size != length:
        if keep:
            os.unlink(kept)
        return end(f"failed in transport: the request stated {length} bytes and the body is not that long", 1,
                   err=b"stand-in gh: the body is not as long as the request states\n")
    record.update(name=name, size=size, sha256=sha.hexdigest())
    return end("stored", 0, out=json.dumps({"id": 1, "name": name, "size": size, "state": "uploaded"}).encode() + b"\n")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
