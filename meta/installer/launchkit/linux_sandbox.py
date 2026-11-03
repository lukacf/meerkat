"""Host-owned Linux check isolation. No privileged or unsandboxed fallback."""
from __future__ import annotations

from contextlib import contextmanager
import ctypes
import errno
import fcntl
import functools
import json
import os
from pathlib import Path
import select
import shutil
import stat
import subprocess
import sys
import sysconfig
import tempfile

PREFIX = "mobkit-linux-bwrap:"
DESCRIPTION = "Linux bubblewrap: private namespaces, mandatory seccomp, no external network or child processes, clean environment, read-only source/tests, exact writable scratch; no unsandboxed fallback."


class LinuxSandboxError(ValueError):
    pass


class _Argument(ctypes.Structure):
    _fields_ = [("arg", ctypes.c_uint), ("op", ctypes.c_uint),
                ("a", ctypes.c_uint64), ("b", ctypes.c_uint64)]


def _trusted_binary(name):
    value = shutil.which(name)
    if not value:
        raise LinuxSandboxError("Required Linux isolation executables are unavailable.")
    path = Path(value).resolve(strict=True)
    info = path.stat()
    if (not stat.S_ISREG(info.st_mode) or info.st_uid not in {0, os.getuid()}
            or info.st_mode & (0o022 | stat.S_ISUID | stat.S_ISGID) or not info.st_mode & 0o111):
        raise LinuxSandboxError("Linux isolation executable ownership or permissions are unsupported.")
    return str(path)


def _filter():
    """Allow threads, never processes. clone3 must fall back to filtered clone."""
    fd, context = None, None
    try:
        library = ctypes.CDLL("libseccomp.so.2")
        library.seccomp_init.argtypes = [ctypes.c_uint32]
        library.seccomp_init.restype = ctypes.c_void_p
        library.seccomp_syscall_resolve_name.argtypes = [ctypes.c_char_p]
        library.seccomp_syscall_resolve_name.restype = ctypes.c_int
        library.seccomp_rule_add_array.argtypes = [ctypes.c_void_p, ctypes.c_uint32,
                                                  ctypes.c_int, ctypes.c_uint, ctypes.POINTER(_Argument)]
        library.seccomp_rule_add_array.restype = ctypes.c_int
        library.seccomp_export_bpf.argtypes = [ctypes.c_void_p, ctypes.c_int]
        library.seccomp_export_bpf.restype = ctypes.c_int
        library.seccomp_release.argtypes = [ctypes.c_void_p]
        context = library.seccomp_init(0x7fff0000)
        if not context:
            raise LinuxSandboxError("Required Linux seccomp policy could not be compiled.")

        def deny(name, argument=None, error=errno.EPERM, required=False):
            number = library.seccomp_syscall_resolve_name(name.encode("ascii"))
            if number < 0:
                if required:
                    raise LinuxSandboxError("Required Linux syscall controls are unavailable.")
                return
            arguments = (_Argument * 1)(argument) if argument is not None else None
            if library.seccomp_rule_add_array(context, 0x00050000 | error, number,
                                             1 if argument is not None else 0, arguments):
                raise LinuxSandboxError("Required Linux seccomp policy could not be compiled.")

        for name in ("fork", "vfork", "unshare", "setns", "mount", "umount2", "pivot_root",
                     "chroot", "ptrace", "process_vm_readv", "process_vm_writev", "bpf",
                     "perf_event_open", "userfaultfd", "keyctl", "add_key", "request_key",
                     "io_uring_setup", "open_by_handle_at"):
            deny(name, required=name in {"unshare", "setns", "ptrace"})
        deny("clone3", error=errno.ENOSYS)
        # SCMP_CMP_MASKED_EQ: required thread flags must all be set. Namespace
        # bits must all be absent. clone's flags are argument zero on both
        # supported Linux architectures, aarch64 and x86_64.
        for bit in (0x100, 0x800, 0x10000):  # CLONE_VM, SIGHAND, THREAD
            deny("clone", _Argument(0, 7, bit, 0), required=True)
        for bit in (0x80, 0x20000, 0x02000000, 0x04000000, 0x08000000,
                    0x10000000, 0x20000000, 0x40000000):
            deny("clone", _Argument(0, 7, bit, bit), required=True)
        # AF_UNIX is useful to asyncio but isolated by the private network and
        # mount namespaces. No inherited host sockets are passed to the child.
        deny("socket", _Argument(0, 1, 1, 0), required=True)  # SCMP_CMP_NE
        deny("socketpair", _Argument(0, 1, 1, 0), required=True)
        fd = os.memfd_create("mobkit-linux-seccomp", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
        if library.seccomp_export_bpf(context, fd):
            raise LinuxSandboxError("Required Linux seccomp policy could not be compiled.")
        os.lseek(fd, 0, os.SEEK_SET)
        # Linux UAPI values remain usable when Python's build headers omit
        # these names. The kernel must still apply and report every seal.
        add_seals = getattr(fcntl, "F_ADD_SEALS", 1033)
        get_seals = getattr(fcntl, "F_GET_SEALS", 1034)
        seals = (getattr(fcntl, "F_SEAL_SEAL", 0x0001)
                 | getattr(fcntl, "F_SEAL_SHRINK", 0x0002)
                 | getattr(fcntl, "F_SEAL_GROW", 0x0004)
                 | getattr(fcntl, "F_SEAL_WRITE", 0x0008))
        fcntl.fcntl(fd, add_seals, seals)
        if fcntl.fcntl(fd, get_seals) & seals != seals:
            raise LinuxSandboxError("Required Linux seccomp policy could not be sealed.")
        return fd
    except LinuxSandboxError:
        if fd is not None:
            os.close(fd)
        raise
    except Exception:
        if fd is not None:
            os.close(fd)
        raise LinuxSandboxError("Required Linux libseccomp policy is unavailable; no code was executed.") from None
    finally:
        if context:
            library.seccomp_release(context)


def profile(snapshot, scratch, staging, reads):
    return PREFIX + json.dumps({"snapshot": str(snapshot), "scratch": str(scratch),
                               "reads": [str(staging / "host-check.py"), *map(str, reads)],
                               "writes": [str(scratch)], "frozen": []}, sort_keys=True)


def _runtime_reads():
    # Standard executable and loader directories contain the trusted runtime,
    # not an entire /usr or custom Python installation prefix. Site packages
    # and a private Node executable are separate explicit caller grants.
    paths = ["/usr/bin", "/bin", "/usr/lib64", "/lib64", "/usr/lib/locale", "/usr/share/locale", "/usr/share/zoneinfo",
             sysconfig.get_path("stdlib"), str(Path(sys.executable).resolve())]
    multiarch = sysconfig.get_config_var("MULTIARCH")
    if multiarch and "/" not in multiarch:
        paths.extend(["/usr/lib/" + multiarch, "/lib/" + multiarch])
    paths.extend(["/lib/ld-linux-aarch64.so.1", "/lib64/ld-linux-x86-64.so.2"])
    library, directory = sysconfig.get_config_var("LDLIBRARY"), sysconfig.get_config_var("LIBDIR")
    if library and directory:
        path = Path(directory) / library
        # The ELF loader may request the versioned SONAME behind this alias.
        # Bind both exact names, never the surrounding library directory.
        paths.extend([str(path), str(path.resolve())])
    return paths


@contextmanager
def launch(binary, encoded, command, cwd, environment):
    if not encoded.startswith(PREFIX):
        raise LinuxSandboxError("A host-owned Linux isolation policy is required.")
    policy = json.loads(encoded[len(PREFIX):])
    filter_fd, sync_read, sync_write = None, None, None
    descriptors = []
    try:
        filter_fd = _filter()
        descriptors.append(filter_fd)
        sync_read, sync_write = os.pipe2(os.O_CLOEXEC)
        descriptors.append(sync_write)
        arguments = [binary, "--unshare-user", "--unshare-pid", "--unshare-net", "--unshare-ipc",
                     "--unshare-uts", "--disable-userns", "--cap-drop", "ALL", "--clearenv",
                     "--new-session", "--die-with-parent", "--proc", "/proc", "--dev", "/dev",
                     "--dir", "/etc"]
        reads = [*_runtime_reads(), policy["snapshot"], *policy["reads"]]
        for raw in dict.fromkeys(reads):
            path = Path(raw)
            if path.exists():
                arguments += ["--ro-bind", str(path.resolve()), str(path)]
        for raw in policy["writes"]:
            path = Path(raw)
            if path.is_symlink() or not path.is_dir() or path.stat().st_uid != os.getuid():
                raise LinuxSandboxError("Linux scratch must be an ordinary owned directory.")
            arguments += ["--bind", str(path), str(path)]
        for raw in policy["frozen"]:
            arguments += ["--ro-bind", raw, raw]
        arguments += ["--remount-ro", "/", "--chdir", str(cwd), "--seccomp", str(filter_fd),
                      "--sync-fd", str(sync_write)]
        for name, value in environment.items():
            arguments += ["--setenv", name, value]
        arguments += ["--", *command]
        # prlimit is a trusted exec wrapper, not an authored subprocess. The
        # virtual address bound accommodates V8's large reserved address cage.
        arguments = [_trusted_binary("prlimit"), "--cpu=60", "--as=8796093022208",
                     "--fsize=16777216", "--nofile=256", "--core=0", "--", *arguments]
        yield arguments, descriptors, sync_read
    finally:
        for descriptor in [*descriptors, sync_read]:
            if descriptor is not None:
                os.close(descriptor)


def confirm_cleanup(descriptor, timeout=2):
    # bubblewrap's trusted namespace PID1 owns the writer. Authored code never
    # inherits it. EOF after reaping the outer child confirms PID1 teardown.
    if not select.select([descriptor], [], [], timeout)[0] or os.read(descriptor, 1) != b"":
        raise LinuxSandboxError("Linux namespace termination is unconfirmed; no verification receipt can be issued.")


@functools.lru_cache(maxsize=8)
def _available(binary, executable, identity):
    try:
        with tempfile.TemporaryDirectory(prefix="mobkit-linux-isolation-probe-") as folder:
            staging = Path(folder).resolve()
            source, scratch = staging / "source", staging / "scratch"
            source.mkdir(mode=0o700)
            scratch.mkdir(mode=0o700)
            frozen, private = source / "frozen.txt", staging / "private.txt"
            frozen.write_text("fixed read-only fixture")
            private.write_text("fictional host-only fixture")
            namespaces = {name: os.readlink("/proc/self/ns/" + name)
                          for name in ("user", "pid", "net", "ipc", "uts", "mnt")}
            encoded = profile(source, scratch, staging, [])
            hidden_config = [str(path) for path in (Path("/usr/local/etc"),
                             Path(sys.prefix) / "pyvenv.cfg", Path(sys.base_prefix) / "etc")
                             if path.exists()]
            code = """import asyncio,ctypes,errno,json,os,pathlib,socket,sqlite3,subprocess,sys,threading
namespaces,hidden=json.loads(sys.argv[1]),json.loads(sys.argv[2])
frozen,private,scratch=sys.argv[3:]
assert all(os.readlink('/proc/self/ns/'+name)!=old for name,old in namespaces.items())
s=pathlib.Path('/proc/self/status').read_text()
assert 'NoNewPrivs:\\t1' in s and 'Seccomp:\\t2' in s
assert all(x not in os.environ for x in ['OPENAI_API_KEY','ANTHROPIC_API_KEY','NODE_OPTIONS','PYTHONPATH'])
assert pathlib.Path(frozen).read_text()=='fixed read-only fixture'
assert all(not pathlib.Path(path).exists() for path in hidden)
for f in [os.fork,lambda:socket.socket(socket.AF_INET),lambda:socket.socket(socket.AF_INET6),lambda:subprocess.run(['/usr/bin/true']),lambda:pathlib.Path(frozen).write_text('changed'),lambda:pathlib.Path(private).read_text()]:
 try: f()
 except OSError: pass
 else: raise AssertionError('forbidden operation was admitted')
done=[]; t=threading.Thread(target=lambda:done.append(True)); t.start();t.join();assert done==[True]
asyncio.run(asyncio.sleep(0))
database=sqlite3.connect(str(pathlib.Path(scratch)/'probe.db'));database.execute('create table probe(value)');database.execute('insert into probe values (1)');database.commit();assert database.execute('select value from probe').fetchone()==(1,);database.close()
print('MOBKIT_LINUX_ISOLATION_READY',flush=True)
"""
            env = {"PATH": "/usr/bin:/bin", "HOME": str(scratch), "TMPDIR": str(scratch)}
            command = [executable, "-I", "-S", "-c", code, json.dumps(namespaces), json.dumps(hidden_config),
                       str(frozen), str(private), str(scratch)]
            with launch(binary, encoded, command, source, env) as (argv, fds, completed):
                process = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, pass_fds=fds,
                                           start_new_session=True)
                # The host must release its writer before waiting for PID1 EOF.
                os.close(fds.pop())
                try:
                    output, _ = process.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)
                    confirm_cleanup(completed)
                    return False
                confirm_cleanup(completed)
                return (process.returncode == 0 and output.strip() == b"MOBKIT_LINUX_ISOLATION_READY"
                        and frozen.read_text() == "fixed read-only fixture"
                        and private.read_text() == "fictional host-only fixture")
    except Exception:
        return False


def executable():
    try:
        binary = _trusted_binary("bwrap")
        info = Path(binary).stat()
        python = str(Path(sys.executable).resolve())
        return binary if _available(binary, python, (info.st_ino, info.st_mtime_ns)) else None
    except (OSError, LinuxSandboxError):
        return None
