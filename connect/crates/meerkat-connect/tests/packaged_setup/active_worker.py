"""Inert job-body substitution for installed native shutdown acceptance only."""
import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import time
from types import ModuleType


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def marker(root, token, name, **values):
    # Publish complete observations atomically; never wait on the test gate here.
    target = root / (name + ".json")
    temporary = root / (name + "." + str(os.getpid()) + ".tmp")
    temporary.write_text(json.dumps(dict(token=token, monotonic_ns=time.monotonic_ns(), **values)) + "\n")
    temporary.replace(target)


def identity():
    return dict(pid=os.getpid(), pgid=os.getpgrp(), sid=os.getsid(0), ppid=os.getppid())


def child(root, token):
    marker(root, token, "child-entered", **identity())
    deadline = time.monotonic() + 90
    while not (root / "release").exists():
        if time.monotonic() >= deadline:
            marker(root, token, "child-expired", **identity())
            return 70
        time.sleep(0.01)
    if (root / "release").read_text() != token:
        return 71
    marker(root, token, "child-completed", **identity())
    return 0


def launch(root, token, helper, arguments):
    # Load exact installed launcher bytes, retaining its real __file__, checks,
    # local-session lease and original controller factory. No installed write.
    helper_raw = helper.read_bytes()
    expected = arguments[arguments.index("--helper-sha256") + 1]
    if hashlib.sha256(helper_raw).hexdigest() != expected:
        raise RuntimeError("fixture launcher selection changed")
    launcher = ModuleType("connect_active_worker_installed_launcher")
    launcher.__file__ = str(helper)
    exec(compile(helper_raw, str(helper), "exec"), launcher.__dict__)
    factory = launcher.controller_factory
    selected = {str(path): digest(path) for path in
                (helper, helper.with_name("mcp_adapter.py"), helper.with_name("backend.py"))}
    marker(root, token, "adapter-entered", sources=selected, shim_sha256=digest(Path(__file__)), **identity())

    def observed_factory(backend, *args, **kwargs):
        create = factory(backend, *args, **kwargs)

        def observed_create(*positional, **keywords):
            owner = create(*positional, **keywords)
            if type(owner.runner) is not backend.Runner:
                raise RuntimeError("fixture requires the actual installed Runner")
            start_job, close_admission, join = owner.start_job, owner.close_admission, owner.join
            substituted = False

            def controlled_job(kind, family, worker, before_start=None, on_success=None, result_status=None):
                nonlocal substituted
                if substituted or kind != "prepare" or family != "services":
                    raise RuntimeError("fixture permits one exact preparation worker")
                substituted = True

                def controlled_worker():
                    # Only this job body is substituted. Runner.run owns the
                    # real child session, stop checks, wait and reaping.
                    argv = [owner.python, "-I", "-B", str(Path(__file__)), "--child", str(root), token]
                    marker(root, token, "worker-entered", argv=argv, python_sha256=digest(Path(owner.python)))
                    code, output = owner.runner.run(argv)
                    entered = json.loads((root / "child-entered.json").read_text())
                    try:
                        os.waitpid(entered["pid"], os.WNOHANG)
                    except ChildProcessError:
                        reaped = True
                    else:
                        reaped = False
                    marker(root, token, "worker-returned", exit_code=code, output_bytes=len(output), child_reaped=reaped)
                    if code != 0 or output or not reaped:
                        raise RuntimeError("controlled child did not complete and reap")
                    return {"scope": "inert_shutdown_fixture_only"}

                return start_job(kind, family, controlled_worker, before_start, on_success, result_status)

            def observed_close_admission():
                result = close_admission()
                marker(root, token, "admission-closed", closing=owner.closing)
                return result

            def observed_join():
                result = join()
                marker(root, token, "controller-joined", closing=owner.closing,
                       workers_joined=all(not thread.is_alive() for thread in owner.threads),
                       job=copy.deepcopy(owner.job), job_count=len(owner.jobs))
                return result

            owner.start_job = controlled_job
            owner.close_admission = observed_close_admission
            owner.join = observed_join
            return owner

        return observed_create

    launcher.controller_factory = observed_factory
    code = launcher.main(arguments)
    current = {path: digest(Path(path)) for path in selected}
    marker(root, token, "adapter-returned", launcher_return_code=code, sources_unchanged=current == selected)
    return code


if __name__ == "__main__":
    if sys.argv[1] == "--child":
        raise SystemExit(child(Path(sys.argv[2]), sys.argv[3]))
    raise SystemExit(launch(Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]), sys.argv[4:]))
