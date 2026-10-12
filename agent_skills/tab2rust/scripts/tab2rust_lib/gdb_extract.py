"""Runs inside gdb (gdb -batch -x gdb_extract.py BINARY). Do not import it from normal Python.

Reads the debug information of BINARY without running it. Environment:
  T2R_ARGS  JSON: {"mode": "inventory"} or {"mode": "dump", "targets": [{key, name, file, func}]}
  T2R_OUT   path of the JSON result

The inventory result is {"records": [...], "live": {file: [lines]}, "sources": [files]}. "live" has
the lines of each file that have code or a variable in the binary. "sources" is the file list of
"info sources". Each variable record has: name, func (None at file scope), storage (global, file,
function, or stack), file, line, addr, section, ctype, size, is_array, leaves, leaves_chararr1,
nonzero, nonzero_chararr1, unsupported (a list), pointers (the type has a pointer), units (the .c
files of its compilation units).
A stack record has no address and no values. In dump mode a record also has desc, and value and
targets if the tool can read the values.
Values are decoded from the raw bytes in the file, so the result is exact.
"""
import bisect
import json
import os
import re
import sys

import gdb

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tab2rust_lib import elfinfo, keys  # noqa: E402

INFERIOR = gdb.selected_inferior()
ELF = elfinfo.read(gdb.current_progspace().filename)
NO_SECTION = {'nobits': False, 'writable': False}
LOADED = sorted(ELF['loader'])
_sym_cache = {}
_unit_cache = {}


def info_symbol(addr):
    """(symbol, offset, section) for an address, or None."""
    if addr not in _sym_cache:
        try:
            s = gdb.execute('info symbol 0x%x' % addr, to_string=True).strip()
        except gdb.error:
            s = ''
        m = re.match(r'^(\S+)(?: \+ (\d+))? in section (\S+)', s)
        _sym_cache[addr] = (m.group(1), int(m.group(2) or 0), m.group(3)) if m else None
    return _sym_cache[addr]


def type_names(t):
    """Typedef chain names, outermost first: ['U16', 'uint16_t', '__uint16_t', 'unsigned short']."""
    names = []
    while True:
        if t.name or t.tag:
            names.append(t.name or t.tag)
        if t.code != gdb.TYPE_CODE_TYPEDEF:
            return names
        t = t.target()


def describe(t, shallow=False):
    """JSON description of a C type. shallow: do not expand struct fields (pointer targets).
    No cache: str(t) is not a unique key (each anonymous struct prints as "struct {...}", and two
    files can use one name for different types), and gdb.Type is not hashable."""
    s = t.strip_typedefs().unqualified()
    c = s.code
    d = {'names': type_names(t), 'size': s.sizeof}
    if c == gdb.TYPE_CODE_ARRAY:
        lo, hi = s.range()
        d.update(kind='array', len=hi - lo + 1, of=describe(s.target(), shallow))
    elif c == gdb.TYPE_CODE_STRUCT:
        fields = [] if shallow else s.fields()
        bits = [f.name for f in fields if f.bitsize]
        if bits:  # n: the items of an initializer row, so that the text count can be compared
            d.update(kind='unsupported', why="bit-field '%s'" % bits[0],
                     n=sum(1 if f.bitsize else leaves(describe(f.type)) for f in fields))
        else:
            d.update(kind='struct', fields=None if shallow else
                     [{'name': f.name, 'offset': f.bitpos // 8, 'type': describe(f.type)} for f in fields])
    elif c == gdb.TYPE_CODE_UNION:  # an initializer sets the first member
        d.update(kind='unsupported', why='union', n=leaves(describe(s.fields()[0].type)) if s.fields() else 1)
    elif c == gdb.TYPE_CODE_ENUM:
        d.update(kind='enum', signed=any(f.enumval < 0 for f in s.fields()),
                 members={f.name: f.enumval for f in s.fields()})
    elif c == gdb.TYPE_CODE_BOOL:
        d.update(kind='bool')
    elif c in (gdb.TYPE_CODE_INT, gdb.TYPE_CODE_CHAR):
        d.update(kind='int', signed=bool(s.is_signed))
    elif c == gdb.TYPE_CODE_FLT and s.sizeof in (4, 8):
        d.update(kind='float')
    elif c == gdb.TYPE_CODE_PTR:
        tc = s.target().strip_typedefs()
        if tc.code == gdb.TYPE_CODE_FUNC:
            d.update(kind='fnptr')
        elif tc.code in (gdb.TYPE_CODE_INT, gdb.TYPE_CODE_CHAR) and tc.sizeof == 1:
            d.update(kind='str', to=describe(s.target()))  # a string, or a pointer into a named array
        elif tc.code == gdb.TYPE_CODE_VOID:
            d.update(kind='unsupported', why='void pointer')
        else:
            d.update(kind='ptr', to=describe(s.target(), True))
    elif c == gdb.TYPE_CODE_FLT:
        d.update(kind='unsupported', why='long double')
    else:
        d.update(kind='unsupported', why='type code %d' % c)
    return d


def subtypes(d):
    return [d['of']] if d['kind'] == 'array' else [f['type'] for f in d.get('fields') or []]


def unsupported(d):
    """All unsupported parts of a type, without repeats."""
    if d['kind'] in ('unsupported', 'fnptr'):
        return [d.get('why', 'function pointer')]
    return list(dict.fromkeys(w for sub in subtypes(d) for w in unsupported(sub)))


def has_pointer(d):
    """True if the type has a pointer: the element, or a field at any depth."""
    if d['kind'] in ('str', 'ptr', 'fnptr') or d.get('why') == 'void pointer':
        return True
    return any(has_pointer(sub) for sub in subtypes(d))


def leaves(d, chararr_as_one=False):
    """Scalar values in a type. With chararr_as_one, a char array counts as one value (a string)."""
    if d['kind'] == 'array':
        o = d['of']
        if chararr_as_one and o['kind'] == 'int' and o['size'] == 1:
            return 1
        return d['len'] * leaves(o, chararr_as_one)
    if d['kind'] == 'struct':
        return sum(leaves(f['type'], chararr_as_one) for f in d['fields'] or [])
    return d.get('n', 1)


def nonzero(d, buf, off, chararr_as_one=False):
    """Scalar values in a type that are not zero. With chararr_as_one, as in leaves."""
    k = d['kind']
    if k == 'array':
        o, es = d['of'], d['of']['size']
        if chararr_as_one and o['kind'] == 'int' and es == 1:
            return int(any(buf[off:off + d['len']]))
        if o['kind'] in ('array', 'struct'):
            return sum(nonzero(o, buf, off + i * es, chararr_as_one) for i in range(d['len']))
        zero = bytes(es)
        return sum(leaves(o) for i in range(d['len']) if buf[off + i * es:off + (i + 1) * es] != zero)
    if k == 'struct':
        return sum(nonzero(f['type'], buf, off + f['offset'], chararr_as_one) for f in d['fields'] or [])
    return leaves(d) if any(buf[off:off + d['size']]) else 0


def read_cstr(addr, limit=1 << 20):
    out = bytearray()
    step = 256
    while len(out) < limit:
        try:
            chunk = bytes(INFERIOR.read_memory(addr + len(out), step))
        except gdb.error:
            if step == 1:
                break
            step = 1
            continue
        z = chunk.find(b'\0')
        if z >= 0:
            return bytes(out + chunk[:z])
        out += chunk
    return bytes(out)


def read_bytes(r):
    """The bytes of a variable. A section without file contents (.bss, .tbss) is all zero. A word that
    the loader fills is 0 in the file: it gets a mark that is not 0. decode() takes its value from
    ELF['loader']."""
    try:
        buf = bytearray(INFERIOR.read_memory(r['addr'], r['size']))
    except gdb.error:
        if ELF['sections'].get(r['section'], NO_SECTION)['nobits']:
            return bytes(r['size'])
        raise
    for a in LOADED[bisect.bisect_left(LOADED, r['addr']):bisect.bisect_left(LOADED, r['addr'] + r['size'])]:
        buf[a - r['addr']] = 1
    return bytes(buf)


def decode(d, buf, off, base):
    """The value of type d at byte off of buf. base is the address of buf[0]."""
    k = d['kind']
    if k == 'array':
        es = d['of']['size']
        return [decode(d['of'], buf, off + i * es, base) for i in range(d['len'])]
    if k == 'struct':
        return [decode(f['type'], buf, off + f['offset'], base) for f in d['fields']]
    raw = buf[off:off + d['size']]
    reloc = ELF['loader'].get(base + off)  # the loader writes an address here: it is never a number
    if reloc is None:
        if k in ('int', 'enum'):
            return int.from_bytes(raw, 'little', signed=d['signed'])
        if k == 'bool':
            return bool(raw[0])
        if k == 'float':
            return {'bits': int.from_bytes(raw, 'little')}
    elif reloc[0]:
        return {'sym': reloc[0], 'off': reloc[1]}
    addr = reloc[1] if reloc else int.from_bytes(raw, 'little')  # a pointer, or an address in a number
    if addr == 0:
        return None
    r = info_symbol(addr)
    if r:  # into a named variable or function
        return {'sym': r[0], 'off': r[1]}
    return {'s': read_cstr(addr).hex()} if k == 'str' else {'addr': addr}  # a string literal has no symbol


def listed_names(cmd):
    """The identifiers in the output of 'info variables -n' or 'info functions -n'."""
    return set(re.findall(r'[A-Za-z_]\w*', gdb.execute(cmd, to_string=True)))


def symbols_named(name):
    """The global symbol and the file-scope static symbols of this name that this binary defines (an
    extern declaration is not a definition)."""
    g = gdb.lookup_global_symbol(name)
    return [s for s in ([g] if g else []) + gdb.lookup_static_symbols(name)
            if s.addr_class != gdb.SYMBOL_LOC_UNRESOLVED and s.symtab]


def unit_files(sym):
    """The .c files of the compilation unit of sym. gdb has no Python object for a compilation unit,
    but each unit has its own global and static block. The result is the .c files of their symbols:
    the compiled file, and also the .c files that it includes."""
    sb = sym.symtab.static_block()
    if sb not in _unit_cache:
        files = {s.symtab.fullname() for b in (sb, sb.global_block) for s in b if s.symtab}
        _unit_cache[sb] = sorted({os.path.realpath(f) for f in files if f.endswith('.c') and os.path.exists(f)})
    return _unit_cache[sb]


def same_data(a, b):
    """True if two records are one header table in two units: the same type and bytes, or both on
    the stack. If the bytes cannot be read, the records stay separate."""
    if (a['ctype'], a['size']) != (b['ctype'], b['size']):
        return False
    if a['addr'] is None or a['addr'] == b['addr']:
        return True
    try:
        return read_bytes(a) == read_bytes(b)
    except gdb.error:
        return False


def record(sym, func, storage):
    """The record of one variable, or None (no source file, or no address). A stack variable has no
    address and no values."""
    st = sym.symtab
    if st is None:
        return None
    d = describe(sym.type)
    r = {'name': sym.name, 'func': func, 'storage': storage, 'file': st.fullname(),
         'line': sym.line, 'addr': None, 'section': None, 'writable': False, 'nobits': False,
         'exported': storage == 'global' and sym.linkage_name in ELF['exports'],
         'ctype': str(sym.type), 'size': sym.type.sizeof,
         'desc': d, 'is_array': d['kind'] == 'array', 'leaves': leaves(d), 'leaves_chararr1': leaves(d, True),
         'nonzero': None, 'nonzero_chararr1': None, 'unsupported': unsupported(d), 'pointers': has_pointer(d),
         'units': unit_files(sym)}
    if storage == 'stack':
        return r
    addr = sym.value().address
    if addr is None:
        return None
    r['addr'] = int(addr)
    sec = info_symbol(r['addr'])
    r['section'] = sec[2] if sec else None
    r['writable'] = ELF['sections'].get(r['section'], NO_SECTION)['writable']
    r['nobits'] = ELF['sections'].get(r['section'], NO_SECTION)['nobits']
    try:
        buf = read_bytes(r)
        r['nonzero'], r['nonzero_chararr1'] = nonzero(d, buf, 0), nonzero(d, buf, 0, True)
    except gdb.error:
        pass
    return r


def is_stack_array(sym):
    return (sym.is_variable and not sym.is_argument and sym.addr_class != gdb.SYMBOL_LOC_STATIC
            and sym.type.strip_typedefs().code == gdb.TYPE_CODE_ARRAY)


def source_files():
    """The files of "info sources": the compiled files and the files that they include."""
    out = gdb.execute('info sources', to_string=True)
    return sorted({os.path.realpath(f.strip()) for ln in out.split('\n') if ln and not ln.endswith(':')
                   and not ln.startswith('(') for f in ln.split(', ') if f.strip()})


def inventory():
    """(records, live lines): all variables with static storage (file scope, and every block of
    every function), and the arrays on the stack. live: {file: set of lines} with code or variables."""
    recs, seen, live = [], {}, {}

    def mark(path, line):
        live.setdefault(os.path.realpath(path), set()).add(line)

    def add(sym, func, storage):
        """One record for each variable. The copies of a header table in several units are one
        record with several units, if their bytes are the same. The compiler's own variables
        (DW_AT_artificial, for example __PRETTY_FUNCTION__) are not records."""
        if sym.is_artificial:
            return
        try:
            r = record(sym, func, storage)
        except gdb.error:
            return
        if not r:
            return
        mark(r['file'], r['line'])
        k = (os.path.realpath(r['file']), sym.line, sym.name, func)
        for old in seen.get(k, []):
            if same_data(old, r):
                old['units'] = sorted(set(old['units'] + r['units']))
                return
        seen.setdefault(k, []).append(r)
        recs.append(r)

    # The compilation units: gdb has no list of them, but each defined symbol gives its unit. The
    # names come from the gdb lists of variables and functions. The symtab of each function has the
    # line table. A header has one symtab per compilation unit: keep them all.
    units, symtabs = set(), {}
    for name in listed_names('info variables -n') | listed_names('info functions -n'):
        for s in symbols_named(name):
            units.add(s.symtab.static_block())
            if s.is_function:
                mark(s.symtab.fullname(), s.line)
                symtabs[(s.symtab.fullname(), s.symtab.static_block().start)] = s.symtab
    for sb in units:  # file scope: the global and the static block of each unit
        for b in (sb.global_block, sb):
            for s in b:
                if s.is_variable and s.addr_class == gdb.SYMBOL_LOC_STATIC:
                    add(s, None, 'global' if b.is_global else 'file')
    # Function scope. Nested blocks have no direct list, so visit the block of every line-table entry.
    blocks = {}
    for st in symtabs.values():
        for e in st.linetable() or []:
            if not e.pc:
                continue
            mark(st.fullname(), e.line)
            b = gdb.block_for_pc(e.pc)
            while b is not None and not (b.is_static or b.is_global) and (b.start, b.end) not in blocks:
                blocks[(b.start, b.end)] = b
                b = b.superblock
    for b in blocks.values():
        fb = b
        while fb is not None and fb.function is None:
            fb = fb.superblock
        func = fb.function.name if fb is not None else None
        for s in b:
            if s.is_variable and s.addr_class == gdb.SYMBOL_LOC_STATIC:
                add(s, func, 'function')
            elif is_stack_array(s):
                add(s, func, 'stack')
    return recs, live


def pointer_targets(value):
    """The type of every array that a pointer in value points into."""
    targets, stack = {}, [value]
    while stack:
        v = stack.pop()
        if isinstance(v, list):
            stack += v
        elif isinstance(v, dict) and 'sym' in v and v['sym'] not in targets:
            syms = symbols_named(v['sym'])
            targets[v['sym']] = describe(syms[0].type) if syms else None
    return targets


def dump(recs, targets):
    """One result for each target: the record with its values, or {key, error, choices}. A stack
    record or a record with an unsupported type has no value."""
    out = []
    for w in targets:
        sel = keys.select(recs, w)
        if len(sel) != 1:
            out.append({'key': w['key'], 'error': 'matched %d variables' % len(sel),
                        'choices': [keys.key_of(r, recs)[0] for r in sel]})
            continue
        r = dict(sel[0], key=w['key'])
        if r['storage'] != 'stack' and not r['unsupported']:
            r['value'] = decode(r['desc'], read_bytes(r), 0, r['addr'])
            r['targets'] = pointer_targets(r['value'])
        out.append(r)
    return out


def main():
    args = json.loads(os.environ['T2R_ARGS'])
    recs, live = inventory()
    if args['mode'] == 'inventory':
        for r in recs:
            del r['desc']
        result = {'records': recs, 'live': {f: sorted(ls) for f, ls in live.items()}, 'sources': source_files()}
    else:
        result = dump(recs, args['targets'])
    with open(os.environ['T2R_OUT'], 'w') as f:
        json.dump(result, f)


main()
