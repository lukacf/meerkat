"""Owner access over the temporary directories a test session leaves behind."""
import os
import stat


def restore_owner_access(top):
    """Give the owner read, write and search on every directory under top.

    Some rows take write or search away from a directory to prove a refusal.
    pytest's own cleanup restores only read and write, and only on the path
    that failed, so such a directory with children stays behind as garbage.
    Symbolic links are not followed.
    """
    pending = [os.fspath(top)]
    while pending:
        path = pending.pop()
        try:
            info = os.lstat(path)
            if not stat.S_ISDIR(info.st_mode):
                continue
            if info.st_mode & stat.S_IRWXU != stat.S_IRWXU:
                os.chmod(path, stat.S_IMODE(info.st_mode) | stat.S_IRWXU)
            with os.scandir(path) as entries:
                pending.extend(entry.path for entry in entries if entry.is_dir(follow_symlinks=False))
        except FileNotFoundError:
            continue


def lacking_owner_access(top):
    """The directories under top, relative to it, without owner read, write and search."""
    lacking, pending = [], [os.fspath(top)]
    while pending:
        path = pending.pop()
        try:
            info = os.lstat(path)
        except PermissionError:
            # Inside a directory without search, which is already listed.
            continue
        if not stat.S_ISDIR(info.st_mode):
            continue
        if info.st_mode & stat.S_IRWXU != stat.S_IRWXU:
            lacking.append(os.path.relpath(path, top))
        try:
            with os.scandir(path) as entries:
                pending.extend(entry.path for entry in entries if entry.is_dir(follow_symlinks=False))
        except PermissionError:
            continue
    return sorted(lacking)
