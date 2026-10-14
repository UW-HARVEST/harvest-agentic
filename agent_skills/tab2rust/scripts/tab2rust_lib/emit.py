"""Rust text for decoded C values. Input: type descriptions and values from gdb_extract.py."""
import re
import struct
from decimal import Decimal

RUST_INT = {(1, False): 'u8', (1, True): 'i8', (2, False): 'u16', (2, True): 'i16', (4, False): 'u32',
            (4, True): 'i32', (8, False): 'u64', (8, True): 'i64', (16, False): 'u128', (16, True): 'i128'}
# bit width and sign of Rust integer types, used to print a value with the right sign
INT_BITS = {'u8': (8, False), 'i8': (8, True), 'u16': (16, False), 'i16': (16, True), 'u32': (32, False),
            'i32': (32, True), 'u64': (64, False), 'i64': (64, True), 'u128': (128, False), 'i128': (128, True),
            'usize': (64, False), 'isize': (64, True), 'core::ffi::c_char': (8, True),
            'core::ffi::c_int': (32, True), 'core::ffi::c_uint': (32, False), 'core::ffi::c_long': (64, True),
            'core::ffi::c_ulong': (64, False), 'core::ffi::c_short': (16, True), 'core::ffi::c_ushort': (16, False),
            'core::ffi::c_uchar': (8, False), 'core::ffi::c_schar': (8, True)}
# Strict and reserved keywords (Rust 2024): a field or constant with such a name gets the r# prefix.
RUST_KEYWORDS = {'as', 'async', 'await', 'break', 'const', 'continue', 'dyn', 'else', 'enum', 'extern', 'false', 'fn',
                 'for', 'gen', 'if', 'impl', 'in', 'let', 'loop', 'match', 'mod', 'move', 'mut', 'pub', 'ref',
                 'return', 'static', 'struct', 'trait', 'true', 'type', 'unsafe', 'use', 'where', 'while',
                 'abstract', 'become', 'box', 'do', 'final', 'macro', 'override', 'priv', 'try', 'typeof', 'unsized',
                 'virtual', 'yield'}
# Names that cannot be raw identifiers: they get a "_" suffix.
NOT_RAW = {'crate', 'self', 'Self', 'super', '_'}
STRING_TYPES = {'cstr': "&'static core::ffi::CStr", 'bytes': "&'static [u8]", 'bytes_nul': "&'static [u8]",
                'str': "&'static str"}
SCALARS = ('int', 'float', 'bool', 'enum')


class TranslateError(Exception):
    pass


def upper_snake(s):
    return re.sub(r'[^A-Za-z0-9_]', '_', re.sub(r'([a-z0-9])([A-Z])', r'\1_\2', s)).upper()


def is_char(d):
    """True for the C type char (also through typedefs), not signed char or unsigned char."""
    return d['kind'] == 'int' and d['names'][-1:] == ['char']


def field_ident(name):
    if name in NOT_RAW:
        return name + '_'
    return 'r#' + name if name in RUST_KEYWORDS else name


def field_name(spec, c_name):
    """Rust name of a struct field: the spec "fields" entry, else the C name in snake_case
    (nextState -> next_state)."""
    return field_ident(spec.get('fields', {}).get(c_name) or upper_snake(c_name).lower())


def esc_byte(x):
    if x == 0x5c:
        return '\\\\'
    if x == 0x22:
        return '\\"'
    if 0x20 <= x < 0x7f:
        return chr(x)
    return {0x0a: '\\n', 0x0d: '\\r', 0x09: '\\t'}.get(x, f'\\x{x:02x}')


def esc_char(c):
    """A character in a Rust string literal."""
    if c in '\\"':
        return '\\' + c
    if c in '\n\r\t':
        return {'\n': '\\n', '\r': '\\r', '\t': '\\t'}[c]
    return f'\\u{{{ord(c):x}}}' if ord(c) < 0x20 or ord(c) == 0x7f else c


def str_literal(hexs, strings):
    """Rust literal of the bytes in hex string hexs. strings: cstr, bytes, bytes_nul or str (UTF-8 only)."""
    raw = bytes.fromhex(hexs)
    if strings == 'str':
        try:
            return '"' + ''.join(esc_char(c) for c in raw.decode('utf-8')) + '"'
        except UnicodeDecodeError:
            raise TranslateError(f'the string {raw[:40]!r} is not UTF-8, so it cannot be a &str.'
                                 ' Use "strings": "cstr" or "bytes".') from None
    body = ''.join(esc_byte(x) for x in raw)
    return {'cstr': f'c"{body}"', 'bytes': f'b"{body}"', 'bytes_nul': f'b"{body}\\0"'}[strings]


def float_bits(text, size):
    """Bits of a float literal as a C float (size 4) or double (size 8). None if it is too large."""
    fmt, ifmt = ('<f', '<I') if size == 4 else ('<d', '<Q')
    try:
        return struct.unpack(ifmt, struct.pack(fmt, float(text)))[0]
    except OverflowError:
        return None


def float_literal(bits, size):
    """Literal that gives exactly these bits. It has the shortest digits. It uses the plain form
    (3600.0) if that is at most 3 characters longer than the exponent form (4.6427e14). NaN and
    infinities use from_bits."""
    fmt, ty = ('<f', 'f32') if size == 4 else ('<d', 'f64')
    f = struct.unpack(fmt, struct.pack('<I' if size == 4 else '<Q', bits))[0]
    if f != f or f in (float('inf'), float('-inf')):
        return f'{ty}::from_bits({hex(bits)})'
    s = repr(f) if size == 8 else next(c for c in (f'{f:.{p}e}' for p in range(12)) if float_bits(c, 4) == bits)
    t = Decimal(s).normalize().as_tuple()
    sign, digits = '-' * t.sign, ''.join(map(str, t.digits))
    if digits == '0':
        return sign + '0.0'
    exp = t.exponent + len(digits) - 1  # of the first digit
    sci = f'{digits[0]}.{digits[1:] or "0"}e{exp}'
    if exp >= 0:
        plain = digits[:exp + 1].ljust(exp + 1, '0') + '.' + (digits[exp + 1:] or '0')
    else:
        plain = '0.' + '0' * (-exp - 1) + digits
    lit = sign + (plain if len(plain) <= len(sci) + 3 else sci)
    assert float_bits(lit, size) == bits, (lit, bits)
    return lit


def shorten(text, width):
    """text on one line, cut to width characters."""
    t = re.sub(r'\s+', ' ', text).replace('[ ', '[').replace(', ]', ']')
    return t if len(t) <= width else t[:width - 3] + '...'


def struct_row_name(d, ctx):
    """Default Rust name of a C struct type: its C name, or '<array>_row' for an anonymous struct
    ('<struct>_<field>_row' for an anonymous struct in a field)."""
    return field_ident(d['names'][0]) if d['names'] else re.sub(r'\W', '_', ctx.split('[')[0]).lower() + '_row'


def index_path(td, off, to):
    """Indexes into the target type td that reach an object of type `to` at byte offset off."""
    if off == 0 and td['kind'] == to['kind'] and td['size'] == to['size'] and td.get('len') == to.get('len'):
        return []
    if td['kind'] == 'array' and td['of']['size']:
        i, rem = divmod(off, td['of']['size'])
        if i < td['len']:
            sub = index_path(td['of'], rem, to)
            return None if sub is None else [i] + sub
    return None


class Emitter:
    """Converts one batch of arrays. Collects the struct definitions that the batch needs."""

    def __init__(self, spec, notes):
        self.types = spec.get('types', {})
        self.structs = spec.get('structs', {})
        self.enums = spec.get('enums', {})
        self.pointers = spec.get('pointers', {})
        self.strings = spec.get('strings', 'cstr')
        self.char_arrays = spec.get('char_arrays', 'bytes')
        if self.strings not in STRING_TYPES:
            raise TranslateError(f'bad "strings": {self.strings}. Use cstr, bytes, bytes_nul or str.')
        self.notes = notes
        self.defs = {}        # Rust struct name -> definition text (None while it is built)
        self.batch = {}       # C symbol -> (Rust name, type description) of the arrays in this batch
        self.nullable = set()  # pointer positions (ctx) that hold NULL somewhere
        self.char_refs = set()  # char pointer positions that point into a named array (not a string literal)
        self.char_strs = set()  # char pointer positions that hold a string literal
        self.row_keys = {}    # ctx of the element struct of an array -> ['KEY[]', 'NAME[]']
        self.used = set()     # the keys of self.structs that a struct used
        self.checked = set()  # Rust struct names whose spec "fields" are checked

    @staticmethod
    def _spec(table, d):
        return next((table[n] for n in d['names'] if n in table), None)

    def struct_spec(self, d, ctx):
        """The spec "structs" entry of a struct type. The key is a C type name, the default Rust name,
        or KEY[] (the element struct of the array KEY)."""
        for k in d['names'] + [struct_row_name(d, ctx)] + self.row_keys.get(ctx, []):
            if k in self.structs:
                self.used.add(k)
                return self.structs[k]
        return {}

    def struct_name(self, d, ctx):
        return self.struct_spec(d, ctx).get('rust') or struct_row_name(d, ctx)

    def find_nullable(self, d, v, ctx):
        """Mark the pointer positions that hold NULL somewhere (they need Option<...>), and the kind of
        the char pointer positions."""
        k = d['kind']
        if k == 'array':
            for x in v:
                self.find_nullable(d['of'], x, ctx + '[]')
        elif k == 'struct':
            local = self.struct_name(d, ctx).split('::')[-1]
            for f, x in zip(d['fields'], v):
                self.find_nullable(f['type'], x, local + '.' + f['name'])
        elif k in ('str', 'ptr') and v is None:
            self.nullable.add(ctx)
        elif k == 'str':
            (self.char_strs if 's' in v else self.char_refs).add(ctx)

    # ----- types -----

    def scalar(self, d):
        override = self._spec(self.types, d)
        if override:
            return override
        k = d['kind']
        if k == 'bool':
            return 'bool'
        if k == 'float':
            return 'f32' if d['size'] == 4 else 'f64'
        if k == 'enum' and self._spec(self.enums, d):
            return self._spec(self.enums, d)['rust']
        if is_char(d):
            return 'core::ffi::c_char'
        return RUST_INT[(d['size'], d['signed'])]

    def is_text(self, d):
        """A plain 'char' array: written as a byte string."""
        return (self.char_arrays == 'bytes' and d['kind'] == 'array' and is_char(d['of'])
                and not self._spec(self.types, d['of']))

    def rtype(self, d, ctx):
        k = d['kind']
        if self.is_text(d):
            return f'[u8; {d["len"]}]'
        if k == 'array':
            return f'[{self.rtype(d["of"], ctx + "[]")}; {d["len"]}]'
        if k == 'struct':
            return self.struct_type(d, ctx)
        if k == 'fnptr':
            raise TranslateError('function pointers are not supported. Translate this table by hand.')
        if k == 'str' and ctx in self.char_refs and ctx in self.char_strs:
            raise TranslateError('a char pointer field has string literals and pointers into arrays.'
                                 ' Translate this table by hand.')
        if k in ('str', 'ptr'):
            base = (STRING_TYPES[self.strings] if k == 'str' and ctx not in self.char_refs
                    else f"&'static {self.pointee_type(d['to'], ctx)}")
            return f'Option<{base}>' if ctx in self.nullable else base
        return self.scalar(d)

    def pointee_type(self, t, ctx):
        if is_char(t) and self.char_arrays == 'bytes' and not self._spec(self.types, t):
            return 'u8'  # a char pointer into a char array: the array is a byte string ([u8; N])
        if t['kind'] in SCALARS:
            return self.scalar(t)
        if t['kind'] == 'array':
            return self.rtype(t, ctx + '*')
        if t['kind'] == 'struct':
            s = self.struct_spec(t, ctx + '*')
            if s.get('rust'):
                return s['rust']
            if t['names']:
                return field_ident(t['names'][0])
            raise TranslateError('pointer to an anonymous struct is not supported')
        raise TranslateError(f'pointer to {t["kind"]} is not supported')

    def struct_type(self, d, ctx):
        """Rust path of the struct. Writes a #[repr(C)] definition unless the spec gives a path."""
        s = self.struct_spec(d, ctx)
        name = s.get('rust') or struct_row_name(d, ctx)
        local = name.split('::')[-1]
        if local not in self.checked:
            self.checked.add(local)
            c_fields = [f['name'] for f in d['fields']]
            self.notes += [f'spec "fields" of struct {local}: "{k}" is not a field of the C struct'
                           f' (fields: {", ".join(c_fields)})' for k in s.get('fields', {}) if k not in c_fields]
        if s.get('define', not s.get('rust')) and local not in self.defs:
            self.defs[local] = None  # a pointer field to this struct does not define it again
            try:
                fields = [f'    pub {field_name(s, f["name"])}: {self.rtype(f["type"], local + "." + f["name"])},'
                          for f in d['fields']]
            except TranslateError:
                del self.defs[local]  # each array of this struct gets the refusal, not a missing definition
                raise
            self.defs[local] = ('#[repr(C)]\n#[derive(Clone, Copy, Debug, PartialEq)]\n'
                                '#[allow(non_camel_case_types, non_snake_case, dead_code)]\n'
                                f'pub struct {local} {{\n' + '\n'.join(fields) + '\n}\n')
        return name

    # ----- values -----

    @staticmethod
    def int_literal(v, rt, radix):
        if rt == 'bool':
            return 'true' if v else 'false'
        bits, signed = INT_BITS.get(rt, (64, True))
        v &= (1 << bits) - 1
        if signed and v >= 1 << (bits - 1):
            v -= 1 << bits
        if signed and v == -(1 << (bits - 1)):
            return f'{rt}::MIN'  # the positive literal of MIN does not fit the type
        return hex(v) if radix == 'hex' and v >= 0 else str(v)

    def pointer_ref(self, v, to):
        if 'sym' not in v:
            raise TranslateError(f'pointer to 0x{v["addr"]:x} has no symbol (for example a string inside a struct)')
        sym, off = v['sym'], v['off']
        if sym in self.batch:
            path, td = self.batch[sym]
        else:
            path, td = self.pointers.get(sym), v.get('target')
            if path is None:
                path = upper_snake(sym)
                self.notes.append(f'pointer target "{sym}" is not in this command and not in spec "pointers". '
                                  f'The file uses "{path}". Make sure that it exists.')
        idx = index_path(td, off, to) if td else ([] if off == 0 else None)
        if idx is None:
            raise TranslateError(f'pointer to {sym}+{off} does not point at a whole element of {sym}')
        if td and to['kind'] in SCALARS:
            want, have = self.pointee_type(to, ''), self.element_type(td, idx)
            if have != want:
                raise TranslateError(f'pointer to {sym}+{off}: the pointer is to {want}, the element of {sym} is {have}'
                                     ' (a cast in C). Translate this table by hand.')
        return f'&{path}' + ''.join(f'[{i}]' for i in idx)

    def element_type(self, td, idx):
        """Rust type of the scalar element td[idx...] as the emitter writes it (a char of a byte string is u8)."""
        parent, d = None, td
        for _ in idx:
            parent, d = d, d['of']
        if parent is not None and self.is_text(parent):
            return 'u8'
        return self.scalar(d) if d['kind'] in SCALARS else None

    def value(self, d, v, ctx, radix, indent, per_line, brief=False):
        """Rust text of value v of type d. brief (for the command output): one line. An array of numbers,
        strings or pointers shows its first three items and its last item. An array of arrays or structs
        shows its first and its last element ('first' or 'last': only that one, for the elements inside
        them). Thus the output has the first and the last element in full."""
        k = d['kind']
        if k in SCALARS and isinstance(v, dict) and 'bits' not in v:
            raise TranslateError('a number holds an address (a pointer cast to a number). Translate this table by hand.')
        if self.is_text(d):
            text = '*b"' + ''.join(esc_byte(x & 0xff) for x in v) + '"'
            return shorten(text, 40) if brief else text
        if k == 'array' and brief:
            one = lambda x, pos: self.value(d['of'], x, ctx + '[]', radix, 0, 0, pos)
            if d['of']['kind'] not in ('array', 'struct') or self.is_text(d['of']):
                head, tail = [one(x, True) for x in v[:3]], [one(x, True) for x in v[3:][-1:]]
            else:
                head = [one(v[0], 'first')] if v and brief != 'last' else []
                tail = [one(v[-1], 'last')] if len(v) > len(head) and brief != 'first' else []
            return '[' + ', '.join(head + (['...'] if len(v) > len(head) + len(tail) else []) + tail) + ']'
        if k == 'array':
            items = [self.value(d['of'], x, ctx + '[]', radix, indent + 1, per_line) for x in v]
            pad = '    ' * (indent + 1)
            if d['of']['kind'] in SCALARS and items:
                n = per_line or max(1, min(32, 100 // (max(map(len, items)) + 2)))
                rows = [', '.join(items[i:i + n]) + ',' for i in range(0, len(items), n)]
            else:
                rows = [x + ',' for x in items]
            return '[\n' + '\n'.join(pad + r for r in rows) + '\n' + '    ' * indent + ']'
        if k == 'struct':
            s = self.struct_spec(d, ctx)
            name = self.struct_type(d, ctx)
            local = name.split('::')[-1]
            vals = [self.value(f['type'], x, local + '.' + f['name'], radix, indent + 1, per_line, brief)
                    for f, x in zip(d['fields'], v)]
            if s.get('ctor'):
                return f'{s["ctor"]}({", ".join(vals)})'
            return name + ' { ' + ', '.join(f'{field_name(s, f["name"])}: {x}'
                                            for f, x in zip(d['fields'], vals)) + ' }'
        if k in ('str', 'ptr'):
            if v is None:
                return 'None'
            lit = str_literal(v['s'], self.strings) if 's' in v else self.pointer_ref(v, d['to'])
            return f'Some({lit})' if ctx in self.nullable else lit
        if k == 'float':
            return float_literal(v['bits'], d['size'])
        if k == 'enum' and self._spec(self.enums, d):
            e = self._spec(self.enums, d)
            member = next((m for m, val in d['members'].items() if val == v), None)
            if member is None:
                raise TranslateError(f'enum value {v} has no enumerator')
            return f'{e["rust"]}::{e.get("variants", {}).get(member, member)}'
        return self.int_literal(v, self.scalar(d), radix)

    def preview(self, rec, rust_name, radix):
        """Short Rust text of the first elements and the last element, for the command output. Each
        element is complete. Only arrays inside it are short."""
        return self.value(rec['desc'], rec['value'], rust_name, radix, 0, 0, True)

    def scan(self, rec, rust_name):
        """Collect the facts of one array that the shared struct definitions need. Call it for all
        arrays of the batch before the first item(): arrays of one struct type share one definition."""
        ctx, d = rust_name, rec['desc']
        while d['kind'] == 'array':
            ctx, d = ctx + '[]', d['of']
        self.row_keys[ctx] = [rec['key'] + '[]', rec['name'] + '[]']
        self.find_nullable(rec['desc'], rec['value'], rust_name)

    def item(self, rec, rust_name, opts, radix):
        """Rust type and complete Rust item (attributes, declaration, value) for one array."""
        rt = self.rtype(rec['desc'], rust_name)
        body = self.value(rec['desc'], rec['value'], rust_name, radix, 0, opts.get('per_line', 0))
        kind = opts.get('kind', 'static')
        if kind not in ('static', 'const'):
            raise TranslateError(f'bad kind "{kind}". Use static or const.')
        attrs = ['#[rustfmt::skip]', '#[allow(non_upper_case_globals, dead_code, clippy::all)]']
        if opts.get('export_name'):
            if kind != 'static':
                raise TranslateError('export_name needs kind "static"')
            attrs.append(f'#[unsafe(export_name = "{opts["export_name"]}")]')
        vis = opts.get('vis', 'pub')
        return rt, '\n'.join(attrs) + f'\n{vis + " " if vis else ""}{kind} {rust_name}: {rt} = {body};\n'

    def unused_structs(self):
        """The spec "structs" keys that no struct of this command used."""
        return [k for k in self.structs if k not in self.used]

    def definitions(self):
        return ''.join(t + '\n' for t in self.defs.values() if t)
