"""Linux memfd seals of accepted remote input on Python builds whose fcntl
module lacks the seal names. The seccomp filter's seals are covered in
test_implementation_workspace.py.

Offline controls; no sandbox, SSH or child process.
"""
import errno
import fcntl
import os
import sys

import pytest

from launchkit import remote_destination

SEAL_NAMES = ("F_ADD_SEALS", "F_GET_SEALS", "F_SEAL_SEAL", "F_SEAL_SHRINK", "F_SEAL_GROW", "F_SEAL_WRITE")
LINUX_UAPI = {"F_ADD_SEALS": 1033, "F_GET_SEALS": 1034, "F_SEAL_SEAL": 0x1, "F_SEAL_SHRINK": 0x2,
              "F_SEAL_GROW": 0x4, "F_SEAL_WRITE": 0x8}
linux_memfd = pytest.mark.skipif(not sys.platform.startswith("linux") or not hasattr(os, "memfd_create"),
                                 reason="Linux memfd sealing")


def test_fallback_values_are_the_linux_uapi_values_and_agree_with_fcntl():
    assert remote_destination._LINUX_MEMFD_SEALS == LINUX_UAPI
    for name, value in LINUX_UAPI.items():
        if hasattr(fcntl, name):
            assert getattr(fcntl, name) == value


@linux_memfd
@pytest.mark.parametrize("names", ["fcntl", "fallback"])
def test_accepted_input_is_sealed_and_the_seals_are_read_back(monkeypatch, tmp_path, names):
    if names == "fallback":
        for name in SEAL_NAMES:
            monkeypatch.delattr(fcntl, name, raising=False)
    with remote_destination._input_descriptor(b"accepted input", tmp_path) as fd:
        assert os.pread(fd, 64, 0) == b"accepted input"
        assert fcntl.fcntl(fd, LINUX_UAPI["F_GET_SEALS"]) & 0xF == 0xF
        for change in (lambda: os.pwrite(fd, b"changed", 0), lambda: os.ftruncate(fd, 1),
                       lambda: os.ftruncate(fd, 4096)):
            with pytest.raises(OSError) as failure:
                change()
            assert failure.value.errno == errno.EPERM


@linux_memfd
def test_a_descriptor_whose_seals_did_not_hold_is_refused(monkeypatch, tmp_path):
    real = fcntl.fcntl

    def skip_add(fd, command, *arguments):
        if command == LINUX_UAPI["F_ADD_SEALS"]:
            return 0
        return real(fd, command, *arguments)

    monkeypatch.setattr(fcntl, "fcntl", skip_add)
    with pytest.raises(OSError, match="did not hold"):
        with remote_destination._input_descriptor(b"accepted input", tmp_path):
            pytest.fail("an unsealed descriptor must not be handed out")


def test_seal_values_are_never_assumed_off_linux(monkeypatch):
    monkeypatch.setattr(sys, "platform", "freebsd14")
    monkeypatch.delattr(fcntl, "F_ADD_SEALS", raising=False)
    with pytest.raises(AttributeError):
        remote_destination._memfd_seal_constant("F_ADD_SEALS")
