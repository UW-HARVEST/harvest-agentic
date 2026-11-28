"""The difference between two sets of files, by their bytes. It uses no git, so no setting of a
repository changes what it finds."""
import difflib
import os
from pathlib import Path

# A file with more bytes than this is compared, but its lines are not.
LARGE = 20 * 2 ** 20
# What `content` gives for the files that are compared by their bytes only.
COMPARED_BY_BYTES = ('binary file', 'large file')
CHUNK = 2 ** 20


def files(root, skip=lambda relative: False):
    """The entries under `root` that are not directories: path relative to `root` -> path. `skip`
    gets the relative path of each entry, and an entry that it accepts is left out with all below
    it. A symbolic link is an entry of its own: nothing is read through it. A directory that
    cannot be read is an entry too."""
    found = {}
    root = Path(root)
    pending = [root]
    while pending:
        directory = pending.pop()
        try:
            entries = list(directory.iterdir())
        except OSError:
            found[directory.relative_to(root).as_posix()] = directory
            continue
        for entry in entries:
            relative = entry.relative_to(root)
            if skip(relative):
                continue
            if entry.is_dir() and not entry.is_symlink():
                pending.append(entry)
            else:
                found[relative.as_posix()] = entry
    return found


def difference(old, new):
    """Compares the files `old` and `new` (relative path -> path, as `files` gives them). Returns
    the changed files as (path, added lines, removed lines), and the full difference as unified
    diff text. The counts are None for a file whose lines cannot be compared: a binary file, a
    file that cannot be read, and an entry that is not a regular file. Such a file on one side
    is always a change."""
    changes, text = [], []
    for path in sorted(old.keys() | new.keys()):
        before, after = (content(side.get(path)) for side in (old, new))
        if before == after and (isinstance(before, list) or before in COMPARED_BY_BYTES
                                and same_bytes(old[path], new[path])):
            continue
        if isinstance(before, str) or isinstance(after, str):
            states = ' and '.join(f'{side}/{path}: {state}' for side, state in (('a', before), ('b', after))
                                  if isinstance(state, str))
            text.append(f'Not compared by lines: {states}\n')
            changes.append((path, None, None))
            continue
        lines = list(difflib.unified_diff(before, after, f'a/{path}' if path in old else '/dev/null',
                                          f'b/{path}' if path in new else '/dev/null'))
        text += lines
        changes.append((path, count(lines, '+'), count(lines, '-')))
    return changes, ''.join(text)


def same_bytes(old, new):
    if old.stat().st_size != new.stat().st_size:
        return False
    with old.open('rb') as one, new.open('rb') as other:
        while chunk := one.read(CHUNK):
            if chunk != other.read(CHUNK):
                return False
    return True


def content(path):
    """The lines of a file for the diff, and [] for no file. For an entry whose lines cannot be
    compared, the text that tells why. The content of a symbolic link is its target."""
    if path is None:
        return []
    try:
        if path.is_symlink():
            return [f'symbolic link to {os.readlink(path)}\n']
        if not path.is_file():
            return 'not a regular file'
        if path.stat().st_size > LARGE:
            return 'large file'
        data = path.read_bytes()
    except OSError:
        return 'cannot be read'
    if b'\0' in data:
        return 'binary file'
    lines = data.decode(errors='surrogateescape').splitlines(keepends=True)
    if lines and not lines[-1].endswith('\n'):
        lines[-1] += '\n\\ No newline at end of file\n'
    return lines


def count(diff_lines, sign):
    """The lines of a unified diff that start with `sign`, without the two header lines."""
    return sum(line.startswith(sign) for line in diff_lines[2:])


def totals(changes):
    """The counts of a list of changes, with the list. The line counts are of the files that are
    compared by lines."""
    return {'files': len(changes), 'added': sum(c[1] or 0 for c in changes),
            'removed': sum(c[2] or 0 for c in changes),
            'files_not_compared_by_lines': sum(c[1] is None for c in changes), 'list': changes}
