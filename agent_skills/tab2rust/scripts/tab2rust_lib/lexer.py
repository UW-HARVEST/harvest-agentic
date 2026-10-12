"""Text scan of C sources: comment/string stripping and initializer extents."""
import bisect
import hashlib
import os
import re

C_EXT = ('.c', '.h', '.inc', '.inl')
SKIP_DIRS = {'.git', 'target', 'CMakeFiles', '.harvest', 'node_modules'}
C_WORDS = set('''auto break case char const continue default do double else enum extern float for goto if
inline int long register restrict return short signed sizeof static struct switch typedef union unsigned
void volatile while _Bool bool true false NULL'''.split())

# '=' that is an assignment, not part of ==, <=, +=, ...
_EQ = r'(?<![=!<>+\-*/%&|^])='
INIT_RE = re.compile(_EQ + r'\s*\{')        # = {
STRING_RE = re.compile(_EQ + r'[ \t]*"')    # = "text"
CONCAT_RE = re.compile(_EQ + r'\s*\n')      # = <newline> MACRO MACRO ... ;
CONTROL_RE = re.compile(r'\b(if|while|for|return)\b\s*\(')
LINE_RE = re.compile(r'\s*#\s*(?:line\s+)?(\d+)(?:\s+"([^"]*)")?')
TYPE_BODY_RE = re.compile(r'\b(struct|union|enum)\s*(?:[A-Za-z_]\w*\s*)?$')
INCLUDE_RE = re.compile(r'\s*#\s*include\s*["<]([^">]+)[">]')
_BRACKETS = r'\s*(?:\[[^\]]*\]\s*)*'
DECL_NAME_RES = [re.compile(p) for p in (
    r'\(\s*\*[\s*]*(?:const\s+)?([A-Za-z_]\w*)' + _BRACKETS + r'\)\s*\([^()]*\)\s*$',  # (*const name[4])(int)
    r'([A-Za-z_]\w*(?:\s*\([^()]*\))?)' + _BRACKETS + '$',  # name[4], PRIV(name)[4]
    r'\(\s*\**\s*([A-Za-z_]\w*)' + _BRACKETS + r'\)' + _BRACKETS + '$')]  # *(name[])


def strip(src):
    """Comments become spaces. String and char contents become S and C. Same length, newlines kept."""
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c == '/' and i + 1 < n and src[i + 1] == '*':
            j = src.find('*/', i + 2)
            j = n if j < 0 else j + 2
            out.append(''.join('\n' if ch == '\n' else ' ' for ch in src[i:j]))
        elif c == '/' and i + 1 < n and src[i + 1] == '/':
            j = src.find('\n', i)
            j = n if j < 0 else j
            out.append(' ' * (j - i))
        elif c in '"\'':
            j = i + 1
            while j < n and src[j] != c and src[j] != '\n':
                j += 2 if src[j] == '\\' else 1
            j = min(j + 1, n)
            ph = 'S' if c == '"' else 'C'
            out.append(c + ''.join('\n' if ch == '\n' else ph for ch in src[i + 1:j - 1]) + (c if j - i >= 2 else ''))
        else:
            out.append(c)
            j = i + 1
        i = j
    return ''.join(out)


class SrcFile:
    """One C file: raw text, stripped text, preprocessor lines, line lookup."""

    def __init__(self, path, rel):
        self.path, self.rel = path, rel
        self.raw = open(path, encoding='latin-1').read()
        self.rawlines = self.raw.split('\n')
        self.lines = strip(self.raw).split('\n')
        self.pp = set()  # 0-based indexes of preprocessor lines (with continuations)
        cont = False
        for k, line in enumerate(self.lines):
            if cont or line.lstrip().startswith('#'):
                self.pp.add(k)
                cont = line.rstrip().endswith('\\')
            else:
                cont = False
        # stripped text with preprocessor lines blanked; positions match the raw text
        self.text = '\n'.join(' ' * len(l) if k in self.pp else l for k, l in enumerate(self.lines))
        self._nl = [0] + [k + 1 for k, ch in enumerate(self.text) if ch == '\n']
        # #line N "FILE" (also the GNU form # N "FILE"): (line of the directive, N, FILE or None)
        self.line_dirs = [(k + 1, int(m.group(1)), m.group(2)) for k in sorted(self.pp)
                          for m in [LINE_RE.match(self.rawlines[k])] if m]
        self._dir_lines = [d[0] for d in self.line_dirs]
        self._names, name = [], None
        for d in self.line_dirs:
            name = d[2] or name
            self._names.append(name)

    def presumed(self, line):
        """(file, line) that the compiler records for a line: the effect of #line. file is the name
        in the last #line, or None."""
        k = bisect.bisect_left(self._dir_lines, line) - 1
        if k < 0:
            return None, line
        return self._names[k], self.line_dirs[k][1] + line - self.line_dirs[k][0] - 1

    def line_of(self, pos):
        """1-based line number of a character position."""
        return bisect.bisect_right(self._nl, pos)


def is_line_file(path, name):
    """True if path (from the debug information) is the file of '#line N "name"'. The compiler joins
    a relative name to its working directory, so the path ends with name."""
    parts = os.path.normpath(name).split(os.sep)
    while parts and parts[0] == os.pardir:  # ../src/parse.y: the directory part above is unknown
        parts.pop(0)
    tail = os.sep.join(parts)
    return path == tail or path.endswith(os.sep + tail.lstrip(os.sep))


def scan_braces(text, start):
    """text[start] is '{'. Return (end, items). Commas inside () and [] do not split items."""
    depth = paren = items = 0
    cur = False
    j = start
    while j < len(text):
        ch = text[j]
        if ch in '([':
            paren += 1
            cur = True
        elif ch in ')]':
            paren -= 1
        elif paren == 0 and ch == '{':
            depth += 1
            cur = False
        elif paren == 0 and ch in '},':
            items += cur
            cur = False
            if ch == '}':
                depth -= 1
                if depth == 0:
                    return j, items
        elif paren == 0 and ch == ';':
            return j, items
        elif ch == '"':
            cur = True
            k = text.find('"', j + 1)
            j = k if k > 0 else j
        elif not ch.isspace():
            cur = True
        j += 1
    return j, items


def decl_start(sf, pos):
    """Position of the first character of the declaration whose '=' is at pos. The search goes
    back to the previous ';', '{' or '}'. A '}' that closes a struct, union or enum body belongs to
    the declaration (static const struct X { ... } name[] = ...)."""
    t = sf.text
    j = pos
    while True:
        k = max(t.rfind(ch, 0, j) for ch in ';{}')
        if k < 0 or t[k] != '}':
            break
        o = matching_open(t, k)
        m = TYPE_BODY_RE.search(t, 0, o) if o >= 0 else None
        if not m:
            break
        j = m.start()
    k += 1
    while k < pos and t[k].isspace():
        k += 1
    return k


def matching_open(t, close):
    """Position of the '{' that matches the '}' at close, or -1."""
    depth = 0
    for j in range(close, -1, -1):
        if t[j] == '}':
            depth += 1
        elif t[j] == '{':
            depth -= 1
            if depth == 0:
                return j
    return -1


def function_bodies(sf):
    """[(open, close, name)] of the function bodies. A function body is the outermost brace block
    whose head (the text after the previous ';', '{' or '}') is a function head (function_name).
    Other blocks around it (extern "C" { ... }) do not count. The binary gives the function of a
    variable that it has. This text rule is only for the arrays that it does not have."""
    out, stack, fn, last, t = [], [], None, -1, sf.text
    for m in re.finditer(r'[{};]', t):
        ch, j = m.group(), m.start()
        if ch == '{':
            name = None if fn else function_name(t[last + 1:j].strip())
            if name:
                fn = (j, name, len(stack))
            stack.append(j)
        elif ch == '}' and stack:
            stack.pop()
            if fn and len(stack) == fn[2]:
                out.append((fn[0], j, fn[1]))
                fn = None
        last = j
    return out


def function_name(head):
    """The name of a function definition head ('static int f(int a)'), or None."""
    if not head.endswith(')') or '=' in head:
        return None
    depth = 0
    for j in range(len(head) - 1, -1, -1):
        depth += {')': 1, '(': -1}.get(head[j], 0)
        if depth == 0:
            m = re.search(r'([A-Za-z_]\w*)\s*$', head[:j])
            return m.group(1) if m else None
    return None


def enclosing_function(sf, bodies, pos):
    """(name, first line, last line) of the function whose body contains pos, or None at file scope."""
    k = bisect.bisect_right([b[0] for b in bodies], pos) - 1
    if k >= 0 and pos < bodies[k][1]:
        start, end, name = bodies[k]
        return name, sf.line_of(start), sf.line_of(end)
    return None


def decl_name(head):
    """Source name of the declared object, for example 'PRIV(ucd_stage2)' or 'table'."""
    t = re.sub(r'\s+', ' ', head).strip()
    m = next((m for p in DECL_NAME_RES for m in [p.search(t)] if m), None)
    return m.group(1).replace(' ', '') if m else t[-40:]


def _record(sf, bodies, kind, m, end, items):
    start = decl_start(sf, m.start())
    head = sf.text[start:m.start()]
    fn = enclosing_function(sf, bodies, m.start())
    return dict(file=sf.rel, kind=kind, name=decl_name(head), pos=(m.start(), end), decl_line=sf.line_of(start),
                eq_line=sf.line_of(m.start()), l0=sf.line_of(start), l1=sf.line_of(end), items=items,
                func=fn[0] if fn else None, func_lines=fn[1:] if fn else None,
                decl=re.sub(r'\s+', ' ', head).strip()[:120], head=head)


def initializers(sf):
    """Initializer-like structures in one file: '= {', and '= "text"' or a concatenation without
    braces for an array. Each has its text item count. A concatenation counts its lines."""
    res, bodies = [], function_bodies(sf)
    t = sf.text
    for m in INIT_RE.finditer(t):
        e, n = scan_braces(t, m.end() - 1)
        r = _record(sf, bodies, 'init', m, e, n)
        if not CONTROL_RE.search(r['head']):
            res.append(r)
    for m in STRING_RE.finditer(t):
        e = t.find(';', m.end())
        if e < 0 or re.search(r'[^\s"S]', t[m.end() - 1:e]):
            continue  # not a plain string literal
        r = _record(sf, bodies, 'string', m, e, 1)
        if '[' in r['head'] and not CONTROL_RE.search(r['head']):
            res.append(r)
    for m in CONCAT_RE.finditer(t):
        e = t.find(';', m.end())
        if e < 0 or '{' in t[m.end():e] or '[' not in t[max(0, t.rfind(';', 0, m.start())):m.start()]:
            continue
        eq, l1 = sf.line_of(m.start()), sf.line_of(e)
        n = sum(1 for k in range(eq, l1) if sf.lines[k].strip() and k not in sf.pp)
        res.append(_record(sf, bodies, 'concat', m, e, n))
    out, end = [], -1  # drop '= {' inside another initializer (designated items such as [X] = {...})
    for r in sorted(res, key=lambda r: r['pos'][0]):
        if r['pos'][0] > end:
            out.append(r)
            end = r['pos'][1]
    return out


def body_items(sf):
    """Text items of a file that is the body of an initializer (an #include inside the braces)."""
    return scan_braces('{' + sf.text + '\n}', 0)[1]


def extent_facts(sf, c):
    """Facts about the text of one initializer: preprocessor lines and their kinds, included files,
    identifiers (not field names after '.'), string literals, and hex and decimal numbers."""
    l0, l1 = c['eq_line'], c['l1']
    pp_lines = [k + 1 for k in range(l0 - 1, l1) if k in sf.pp]
    pp_kinds = sorted({m.group(1) for k in pp_lines for m in [re.match(r'\s*#\s*(\w+)', sf.lines[k - 1])] if m})
    includes = [m.group(1) for k in pp_lines for m in [INCLUDE_RE.match(sf.rawlines[k - 1])] if m]
    body = '\n'.join(sf.text.split('\n')[l0 - 1:l1]).split('=', 1)[-1]
    strings = len(re.findall(r'"S*"', body))
    body = re.sub(r'"S*"|\'C*\'', ' ', body)
    idents = sorted({w for w in re.findall(r'(?<![.\w])[A-Za-z_]\w*', body) if w not in C_WORDS})
    return dict(pp_lines=pp_lines, pp_kinds=pp_kinds, includes=includes, idents=idents, strings=strings,
                hex=len(re.findall(r'\b0[xX][0-9a-fA-F]+', body)),
                dec=len(re.findall(r'(?<![\w.])\d+(?![\w.])', body)))


def paths_wanted(rels, exclude=None, only=None):
    """False if one of the relative paths matches exclude, or none matches only (regular expressions)."""
    return (not (exclude and any(re.search(exclude, r) for r in rels))
            and not (only and not any(re.search(only, r) for r in rels)))


def scan_tree(root):
    """C files under root: {path: [paths of copies with the same content]}. A copy is not scanned
    again, but the debug information can name it."""
    first, files = {}, {}
    for dp, dn, fn in os.walk(root):
        # a directory with CMakeCache.txt is a build directory: its C files are generated or copies
        dn[:] = sorted(d for d in dn if d not in SKIP_DIRS and not os.path.exists(os.path.join(dp, d, 'CMakeCache.txt')))
        for f in sorted(fn):
            if f.endswith(C_EXT):
                p = os.path.join(dp, f)
                h = hashlib.sha1(open(p, 'rb').read()).hexdigest()
                if h in first:
                    files[first[h]].append(p)
                else:
                    first[h] = p
                    files[p] = []
    return files
