"""Loose repetition detector: share of repeated token patterns in a sliding window.

Names become I, numbers N, strings S. Keywords, punctuation and preprocessor directive
names stay. A window is repetitive when few of its 8-token sequences are distinct.
"""
import collections
import re

KEYWORDS = set('if else for while do switch case default return break continue goto sizeof struct union '
               'enum typedef static const extern'.split())
TOKEN_RE = re.compile(r'#\s*\w+|[A-Za-z_]\w*|0[xX][0-9a-fA-F]+\w*|\d[\w.]*|"S*"|\'C*\'|\S')

WINDOW = 160   # tokens per window (tuned on harvest-bench: fewest false regions)
K = 8          # tokens per pattern


def skeleton(sf):
    """(symbol, line) for each token of the stripped text."""
    out = []
    for ln, line in enumerate(sf.lines, 1):
        for m in TOKEN_RE.finditer(line):
            s = m.group(0)
            if s[0] == '#':
                s = '#' + s[1:].strip()
            elif s[0].isalpha() or s[0] == '_':
                s = s if s in KEYWORDS else 'I'
            elif s[0].isdigit():
                s = 'N'
            elif s[0] in '"\'':
                s = 'S'
            out.append((s, ln))
    return out


def redundancy(grams):
    return 1 - len(set(grams)) / max(1, len(grams))


def regions(sf, threshold, min_lines, skip_lines=frozenset()):
    """Maximal line ranges whose windows have redundancy >= threshold, without skip_lines."""
    toks = skeleton(sf)
    n = len(toks)
    if n < WINDOW:
        return []
    sym = [s for s, _ in toks]
    grams = [tuple(sym[i:i + K]) for i in range(n - K + 1)]
    hot = [False] * n
    for st in range(0, n - WINDOW + 1, WINDOW // 4):
        if redundancy(grams[st:st + WINDOW - K + 1]) >= threshold:
            hot[st:st + WINDOW] = [True] * WINDOW
    res, i = [], 0
    while i < n:
        if not hot[i] or toks[i][1] in skip_lines:
            i += 1
            continue
        j = i
        while j < n and hot[j] and toks[j][1] not in skip_lines:
            j += 1
        l0, l1 = toks[i][1], toks[j - 1][1]
        if l1 - l0 + 1 >= min_lines:
            res.append(dict(file=sf.rel, l0=l0, l1=l1, lines=l1 - l0 + 1,
                            redundancy=round(redundancy(grams[i:max(i + 1, j - K + 1)]), 2)))
        i = j
    return res


def line_head(sf, l0, l1):
    """(token, lines, share): the most common first token of the non-blank lines l0..l1, the number
    of lines that start with it (the items of the main pattern), and their share of these lines.
    Numbers and strings count as one token each."""
    heads = []
    for line in sf.lines[l0 - 1:l1]:
        m = TOKEN_RE.search(line)
        if m:
            t = m.group(0)
            heads.append('#' + t[1:].strip() if t[0] == '#' else '<number>' if t[0].isdigit()
                         else '<string>' if t[0] in '"\'' else t)
    if not heads:
        return None, 0, 0.0
    top, n = collections.Counter(heads).most_common(1)[0]  # a tie goes to the token seen first
    return top, n, n / len(heads)
