"""Keys of variable records: the --symbol value of translate and dump.
gdb_extract.py also imports this module, so it must not import gdb or other tab2rust modules."""
import os


def parse(key):
    """{key, name, file, func, unit} from NAME, FILE:NAME or FILE:FUNCTION:NAME, with an optional
    @UNIT at the end (the .c file of one compilation unit). None if the key is malformed."""
    body, _, unit = key.partition('@')
    parts = body.split(':')
    if not 1 <= len(parts) <= 3 or not all(parts):
        return None
    return dict(key=key, name=parts[-1], file=parts[0] if len(parts) > 1 else None,
                func=parts[1] if len(parts) == 3 else None, unit=unit or None)


def _base(r):
    return os.path.basename(r['file'])


def matches(r, want):
    """want: a parsed key. It can also have path (the real path of the file) and line."""
    return (r['name'] == want['name']
            and (want.get('func') is None or r['func'] == want['func'])
            and (want.get('path') is None or os.path.realpath(r['file']) == want['path'])
            and (want.get('line') is None or r['line'] == want['line'])
            and (want.get('file') is None or _base(r) == want['file']
                 or r['file'].endswith('/' + want['file']))
            and (want.get('unit') is None or any(os.path.basename(u) == want['unit'] for u in r['units'])))


def select(recs, want):
    return [r for r in recs if matches(r, want)]


def key_of(r, recs):
    """(key, unique): the shortest key that selects only r. The forms are NAME, then FILE:NAME (file
    scope) or FILE:FUNCTION:NAME (function scope), then that form with @UNIT."""
    same = [x for x in recs if x['name'] == r['name']]
    full = f'{_base(r)}:{r["func"]}:{r["name"]}' if r['func'] else f'{_base(r)}:{r["name"]}'
    forms = [r['name'], full] + [f'{full}@{os.path.basename(u)}' for u in r['units']]
    for k in forms:
        sel = select(same, parse(k))
        if len(sel) == 1 and sel[0] is r:
            return k, True
    return full, False
