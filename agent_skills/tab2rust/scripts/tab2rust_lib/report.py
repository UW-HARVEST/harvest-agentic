"""Text form of the detect report."""
CLASSES = ('PURE', 'TAINTED', 'DEAD')
LEGEND = '''classes: PURE = translate with the tool. TAINTED = in the build, but the tool cannot give the values.
  Translate it by hand. Your translation must have it. DEAD = not in this build, do not translate.
notes (they do not change the class):
  #if          preprocessor lines inside the initializer. The values are for this build only.
  body in F    the initializer body is in the included file F.
  macro items  macros expand to several items (the text has fewer items than the binary).
  zero fill    the text has fewer items than the binary. The other values are 0.
  no braces    string or macro concatenation. The text count is a line count.
  not in text  the binary has the array, but the text scan did not find it.
  writable     the array is in a writable section. The C program can change it at run time.
               "pointers can change": the elements or their fields have pointers.
  stack copy   the array is on the stack in C. The values come from a static copy that "build" made.
TAINTED reasons: stack (no static copy, and why), type (unsupported C type),
  count (text and binary counts differ, and no note explains it), key (no key selects only this array).
DEAD reasons: whole file under #if (no unit has the file, and these #if branches have all its code),
  file not in the binary (no unit has the file: it is not compiled, or it compiles to nothing),
  dead branch (no code of this #if branch is in the binary),
  function not emitted (for example an unused static inline function), cause not found.
REPETITIVE regions are hints only. See references/repetitive-structures.md.'''


def units_column(file, units):
    """' | in FILES': the .c files of the compilation units, if the file of the array is not one of them."""
    if file in units:
        return ''
    more = f' (+{len(units) - 3} more)' if len(units) > 3 else ''
    return ' | in ' + (', '.join(units[:3]) + more if units else '?')


def texts(items):
    return '; '.join(x['text'] for x in items)


def row(f):
    """key | file:lines | items text/binary | C type | storage [| in UNITS] [| why: REASONS] [| notes: NOTES]"""
    text = '-' if f['text_items'] is None else f['text_items']
    storage = f['storage'] if f['storage'] in ('global', 'file') else f'function {f["func"]}'
    line = (f'{f["key"]} | {f["file"]}:{f["lines"][0]}-{f["lines"][1]} | {text}/{f["binary_items"]}'
            f' | {f["ctype"][:70]} | {storage}{units_column(f["file"], f["units"])}')
    if f['reasons']:
        line += ' | why: ' + texts(f['reasons'])
    return line + (' | notes: ' + texts(f['notes']) if f['notes'] else '')


def dead_row(f):
    """name | file:lines | text items | reason [| notes: NOTES]"""
    line = f'{f["name"]} | {f["file"]}:{f["lines"][0]}-{f["lines"][1]} | {f["text_items"]} | {texts(f["reasons"])}'
    return line + (' | notes: ' + texts(f['notes']) if f['notes'] else '')


def print_report(rep, rep_show):
    by = {k: [f for f in rep['findings'] if f['cls'] == k] for k in CLASSES}
    plain = sum(1 for f in by['PURE'] if not f['notes'])
    print(f'tab2rust detect {rep["version"]}  src={rep["src"]}')
    print(f'binary={rep["binary"]}  min_text_items={rep["min_items"]}  code_lines={rep["code_lines"]}')
    print(f'summary: PURE={len(by["PURE"])} ({plain} without notes), TAINTED={len(by["TAINTED"])},'
          f' DEAD={len(by["DEAD"])}, REPETITIVE={len(rep["repetitive"])}')
    print('\n== PURE ==  (translate with the tool. KEY is the --symbol value)')
    print('key | file:lines | items text/binary | C type | storage [| in: compiled files] [| notes]')
    for f in by['PURE']:
        print(row(f))
    print('\n== TAINTED ==  (in the build: translate by hand. "why" gives all reasons)')
    print('key | file:lines | items text/binary | C type | storage [| in: compiled files] | why [| notes]')
    for f in by['TAINTED']:
        print(row(f))
    print('\n== DEAD ==  (not in this build. Do not translate)')
    print('name | file:lines | text items | reason [| notes]')
    for f in by['DEAD']:
        print(dead_row(f))
    print(f'\n== REPETITIVE ==  (loose. redundancy = share of repeated 8-token patterns.'
          f' >= {rep["rep_threshold"]} reported, >= 0.85 strong)')
    print('redundancy  file:lines (n lines) | most common first token of a line: lines that start with it (share)'
          ' | first line / middle line')
    for s in rep['repetitive'][:rep_show]:
        print(f'{s["redundancy"]:.2f}  {s["file"]}:{s["l0"]}-{s["l1"]} ({s["lines"]} lines)'
              f' | {s["line_head"]}: {s["line_head_lines"]} lines ({s["line_head_share"]:.0%})')
        for smp in s['sample']:
            print(f'        {smp}')
    if 0 < rep_show < len(rep['repetitive']):
        print(f'... {len(rep["repetitive"]) - rep_show} more regions (smaller). Use --json to see all, or --rep-show N.')
    print('\n' + LEGEND)
