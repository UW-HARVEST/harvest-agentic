"""Root causes of the unsafe operations of a scan, grouped for global (cross-file) refactoring:
which types are reached through raw pointers, which local pointers walk buffers, which unsafe
functions make their callers unsafe, which structs hold raw pointers, and how much sits in the
exported C ABI."""
import collections
import re

RAW_POINTER_FUNCTIONS = frozenset('''
add sub offset offset_from byte_add byte_sub wrapping_add wrapping_sub read write read_unaligned
write_unaligned read_volatile write_volatile copy copy_nonoverlapping write_bytes swap replace
as_ref as_mut from_raw_parts from_raw_parts_mut drop_in_place zeroed transmute assume_init
'''.split())
PTR_PREFIX = re.compile(r'^\*\s*(const|mut)\b\s*')


def pointee(type_text):
    """`*mut Foo` -> `Foo`; other type texts unchanged."""
    t = ' '.join(type_text.split())
    while PTR_PREFIX.match(t):
        t = PTR_PREFIX.sub('', t, count=1)
    return t or '?'


def report(scan, top=15):
    t = scan['totals']
    rows = scan['rows']
    ops = scan['ops']
    by_name = collections.defaultdict(list)
    for r in rows:
        by_name[r['fn'].split('::')[-1]].append(r)
    out = ['# Root causes of the unsafe operations', '',
           f"Total {t['unsafe_ops']}: in exported (C ABI) functions {t['unsafe_ops_exported']}, internal "
           f"{t['unsafe_ops_internal']}, outside functions {t['unsafe_ops_outside_fn']}. By kind: "
           + ', '.join(f'{k} {v}' for k, v in t['unsafe_ops_by_kind'].items()), '']

    derefs = [o for o in ops if o['kind'] == 'deref']
    typed = collections.defaultdict(list)
    local = collections.Counter()
    for o in derefs:
        vt = o.get('var_type', '')
        if vt in ('', 'local'):
            local[(o['file'], o['fn'], o.get('var') or '?')] += 1
        else:
            typed[pointee(vt)].append(o)
    out += ['## 1. Dereferences through raw pointer parameters, by pointee type', '',
            'A type that is reached through raw pointers in many functions is a candidate for a global '
            'change: pass `&T`/`&mut T` (or slices) through the call chain, and convert once at the C ABI.', '',
            '| pointee type | dereferences | functions | files | top functions |', '|---|---|---|---|---|']
    for ty, lst in sorted(typed.items(), key=lambda kv: -len(kv[1]))[:top]:
        fns = collections.Counter(o['fn'] for o in lst)
        files = sorted({o['file'] for o in lst})
        out.append(f"| {ty} | {len(lst)} | {len(fns)} | {', '.join(files[:4])}{' ...' if len(files) > 4 else ''} | "
                   + ', '.join(f'{f} ({n})' for f, n in fns.most_common(3)) + ' |')
    out += ['', f"## 2. Dereferences through local pointers (buffer cursors and the like): {sum(local.values())}", '',
            'A local pointer that walks a buffer becomes an index into a slice once the buffer is a slice.', '',
            '| file | function | local pointer | dereferences |', '|---|---|---|---|']
    for (f, fn, var), n in local.most_common(top):
        out.append(f'| {f} | {fn} | {var} | {n} |')

    callees = collections.Counter()
    raw_methods = collections.Counter()
    for o in ops:
        if o['kind'] == 'call' and o.get('detail'):
            name = o['detail'].split('::')[-1]
            (raw_methods if name in RAW_POINTER_FUNCTIONS and name not in by_name else callees)[name] += 1
    out += ['', f"## 3a. Raw pointer arithmetic and raw memory functions: {sum(raw_methods.values())} calls", '',
            ', '.join(f'{k} {v}' for k, v in raw_methods.most_common()) or '-', '',
            'These go away when the buffer becomes a slice and the cursor an index (or `copy_from_slice`).']
    out += ['', '## 3b. Calls of unsafe crate and library functions, by callee', '',
            'A crate function that is `unsafe` only because of its raw pointer signature makes every caller '
            'unsafe. Making it safe (references, slices) removes these call operations in all callers.', '',
            '| callee | calls | defined in the crate | callee: exported, own unsafe ops, raw pointer params |',
            '|---|---|---|---|']
    for name, n in callees.most_common(top):
        defs = by_name.get(name, [])
        if defs:
            d = defs[0]
            info = f"{d['vis']}, {d['ops']}, {d['raw_params']}"
            where = d['file']
        else:
            info, where = '-', 'no (std or extern)'
        out.append(f'| {name} | {n} | {where} | {info} |')

    structs = t.get('structs_with_raw_fields', [])
    out += ['', f'## 4. Structs with raw pointer fields: {len(structs)}', '',
            'Raw pointer fields (buffers, links, back-pointers) force unsafe at every use. Candidates: owned '
            '`Vec`/`Box`, `Option<&T>`/indices, or a small trusted module that owns the pointer.', '',
            '| struct | file | raw pointer fields (of all fields) |', '|---|---|---|']
    for st in sorted(structs, key=lambda s: -len(s['raw_fields']))[:top]:
        out.append(f"| {st['name']} | {st['file']}:{st['line']} | {len(st['raw_fields'])} of {st['fields']}: "
                   + '; '.join(st['raw_fields'][:6]) + ('; ...' if len(st['raw_fields']) > 6 else '') + ' |')

    out += ['', '## 5. Files', '', '| file | unsafe ops | in exported functions | internal | functions with ops |',
            '|---|---|---|---|---|']
    per_file = collections.defaultdict(collections.Counter)
    for r in rows:
        c = per_file[r['file']]
        c['ops'] += r['ops']
        c['exp' if r['vis'] == 'exported' else 'int'] += r['ops']
        c['fns'] += r['ops'] > 0
    for f, c in sorted(per_file.items(), key=lambda kv: -kv[1]['ops']):
        if c['ops']:
            out.append(f"| {f} | {c['ops']} | {c['exp']} | {c['int']} | {c['fns']} |")
    missing = sum(1 for o in derefs if not o.get('var'))
    out += ['', f'Note: the pointee type comes from the parameter or typed `let` that the dereferenced variable '
            f'names; {missing} dereferences had no simple variable. Counts per type are approximate.']
    return '\n'.join(out) + '\n'
