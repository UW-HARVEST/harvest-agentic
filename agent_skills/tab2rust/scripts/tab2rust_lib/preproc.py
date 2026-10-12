"""Preprocessor structure of the C text: conditional branches and macro definitions."""
import re

IF = ('if', 'ifdef', 'ifndef')
ELSE = ('elif', 'elifdef', 'elifndef', 'else')
DEFINE_RE = re.compile(r'#\s*define\s+([A-Za-z_]\w*)(\([^)]*\))?(.*)')
IDENT_RE = re.compile(r'[A-Za-z_]\w*')
GUARD_RE = re.compile(r'#(?:ifndef\s+(\w+)|if\s+!\s*defined\s*\(?\s*(\w+)\s*\)?)$')


def directives(sf):
    """(line, kind, text) of each preprocessor directive. Continuation lines are joined. The text
    has no comments."""
    out, cur = [], None
    for k in sorted(sf.pp):
        line = sf.lines[k]
        if cur is not None and cur[3]:
            cur[2] += ' ' + line.strip().rstrip('\\')
        else:
            m = re.match(r'\s*#\s*(\w+)', line)
            cur = [k + 1, m.group(1) if m else '', line.strip().rstrip('\\'), False]
            out.append(cur)
        cur[3] = line.rstrip().endswith('\\')
    return [(n, kind, re.sub(r'^#\s*', '#', re.sub(r'\s+', ' ', text).strip())) for n, kind, text, _ in out]


def branches(sf):
    """Conditional branches: {first, last, text, if_line, if_text}. first is the line of the #if,
    #elif or #else. last is the line of the next #elif, #else or #endif."""
    out, stack = [], []
    for line, kind, text in directives(sf):
        if kind in IF:
            stack.append(dict(first=line, text=text, if_line=line, if_text=text))
        elif kind in ELSE + ('endif',) and stack:
            out.append(dict(stack[-1], last=line))
            if kind == 'endif':
                stack.pop()
            else:
                stack[-1] = dict(stack[-1], first=line, text=text)
    out += [dict(b, last=len(sf.lines)) for b in stack]
    return out


def guard_lines(sf):
    """Lines of the include guards: an #ifndef X (or #if !defined X) that the next directive #define X follows."""
    ds = directives(sf)
    out = set()
    for (line, _, text), (_, kind, nxt) in zip(ds, ds[1:]):
        m = GUARD_RE.match(text)
        if m and kind == 'define' and re.match(r'#define\s+' + (m.group(1) or m.group(2)) + r'\b', nxt):
            out.add(line)
    return out


def several_items(body):
    """True if a macro body can give more than one initializer item: a comma outside parentheses,
    or a brace."""
    depth = 0
    for ch in body:
        if ch == '(':
            depth += 1
        elif ch == ')':
            depth -= 1
        elif ch == '{' or (ch == ',' and depth == 0):
            return True
    return False


class Macros:
    """The #define names of the scanned files. multi: the names that can expand to several
    initializer items, directly or through another macro."""

    def __init__(self, sfs):
        uses = {}
        self.multi = set()
        for sf in sfs:
            for _, kind, text in directives(sf):
                m = DEFINE_RE.match(text) if kind == 'define' else None
                if m:
                    body = m.group(3)
                    uses.setdefault(m.group(1), set()).update(IDENT_RE.findall(body))
                    if several_items(body):
                        self.multi.add(m.group(1))
        grew = True
        while grew:
            more = {n for n, ids in uses.items() if n not in self.multi and ids & self.multi}
            self.multi |= more
            grew = bool(more)
