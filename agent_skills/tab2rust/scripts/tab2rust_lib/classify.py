"""Class of a finding that is in the build: PURE or TAINTED, with reasons and notes.

PURE: the tool gives exact values. TAINTED: it cannot (the reasons say why). Notes never change
the class. detect and translate both use classify(). deadcode.py gives the reason of DEAD.
"""
COND = ('if', 'ifdef', 'ifndef', 'elif', 'elifdef', 'elifndef', 'else', 'endif')


def item(tag, detail=None, text=None):
    """A reason or a note: a fixed tag, and its text for the report ("tag (detail)")."""
    return {'tag': tag, 'text': text or (f'{tag} ({detail})' if detail else tag)}


def text_notes(c):
    """Notes about the text of the initializer: preprocessor lines, included bodies, no braces."""
    f, notes = c['facts'], []
    kinds = [k for k in f['pp_kinds'] if k in COND + ('define', 'undef') and k != 'endif']
    if kinds:
        notes.append(item('#if', ', '.join('#' + k for k in kinds)))
    if f['includes']:
        notes.append(item('body', text='body in ' + ', '.join(c['bodies'] + [f'{m} (not found)' for m in c['missing']])))
    if c['kind'] == 'concat':
        notes.append(item('no braces'))
    return notes


def count_check(c, rec, macros):
    """(notes, reason): compare the text items with the binary values. reason is None if the counts
    agree or a known cause explains the difference."""
    f, text, full = c['facts'], c['items'], rec['leaves']
    unexplained = item('count', f'text {text}, binary {full}: no known cause')
    if c['kind'] == 'concat' or c['missing'] or text == full or (f['strings'] and text == rec['leaves_chararr1']):
        return [], None
    if text > full:
        if any(k in COND for k in f['pp_kinds']):
            return [], None  # #if alternatives that this build does not compile
        return [], unexplained
    multi = [w for w in f['idents'] if w in macros.multi]
    notes = [item('macro items', ', '.join(multi[:4]) + (', ...' if len(multi) > 4 else ''))] if multi else []
    if multi:
        return notes, None
    nz = rec['nonzero']
    if nz is not None and nz > text and f['strings']:
        nz = rec['nonzero_chararr1']  # a string literal fills a char array: one item
    if nz is None:
        return [item('zero fill', 'not checked: no values')], None
    if nz <= text:
        return [item('zero fill', f'{nz} of {full} values are not 0')], None
    return [], unexplained


def classify(c, rec, macros, not_in_text=None):
    """{cls, reasons, notes} for a record that is in the build. c is its text candidate, or None
    (then not_in_text says why the text scan did not find it)."""
    reasons, notes = [], []
    if rec['storage'] == 'stack' and rec.get('copy'):  # the values come from a static copy (stackcopy.py)
        notes.append(item('stack copy'))
    elif rec['storage'] == 'stack':
        reasons.append(item('stack', rec.get('copy_error')))
    reasons += [item('type', u) for u in rec['unsupported']]
    if c is None:
        notes.append(item('not in text', not_in_text))
    else:
        notes += text_notes(c)
        more, why = count_check(c, rec, macros)
        notes += more
        if why:
            reasons.append(why)
    if rec.get('exported'):  # the dynamic symbol table (elfinfo.py)
        notes.append(item('exported', rec['name']))
    if rec['writable']:  # ELF section flags (elfinfo.py)
        notes.append(item('writable', rec['section'] + (', pointers can change' if rec['pointers'] else '')))
    return dict(cls='TAINTED' if reasons else 'PURE', reasons=reasons, notes=notes)
