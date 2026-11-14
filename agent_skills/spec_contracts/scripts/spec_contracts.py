#!/usr/bin/env python3
"""Reads the function contracts of a C library, with its C source and the tests of verify_env/.

    python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py list
    python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py show FUNCTION
    python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py notes
    python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py check DOMAINS.md
    python3 inputs/agent_skills/spec_contracts/scripts/spec_contracts.py properties [FUNCTION ...]

Run it from the root of the checkout. Each command accepts --spec DIR (default
inputs/extra_info/spec), --c-src DIR (default inputs/c_src) and --tests DIR (default verify_env).
Exit status: 0 for success, 1 when check finds a problem, 2 for a line that starts with "Error:".

The commands do syntax work only. They do not decide what a clause means for the tests.
See inputs/agent_skills/spec_contracts/SKILL.md.
"""
import argparse
import os
import re
import subprocess
import sys

SPEC = 'inputs/extra_info/spec'
C_SRC = 'inputs/c_src'
TESTS = 'verify_env'
NO_PROBLEMS = 'No problems found.'
NOT_COVERED = 'not covered:'

STRING = r'"(?:[^"\\\n]|\\.)*"'
LITERAL_OR_COMMENT = re.compile(STRING + r"|'(?:[^'\\\n]|\\.)*'|//[^\n]*|/\*.*?\*/", re.S)
DIRECTIVE = re.compile(r'^[ \t]*#(?:[^\n]*\\\n)*[^\n]*', re.M)
BRACKET_OR_STRING = re.compile(STRING + r'|[(){}]')
C_NAME = r'[A-Za-z_]\w*'
NOT_FUNCTIONS = ('if', 'for', 'while', 'switch', 'catch', 'return', 'sizeof')


class SpecError(Exception):
    """The command cannot do its task. The message is one line for the reader."""


# ---- C text ----

def read(path):
    with open(path, encoding='utf-8', errors='replace') as f:
        return f.read()


def blank(text):
    """The text with a space in the place of each character. The line breaks stay, and thus the
    line numbers and the positions."""
    return re.sub(r'[^\n]', ' ', text)


def uncommented(text, literals=True):
    """The text with blank comments. With literals=False, the string literals and the character
    literals are blank too, so that a bracket in a literal is not read as code."""
    def replacement(m):
        return m.group() if literals and m.group()[0] in '"\'' else blank(m.group())
    return LITERAL_OR_COMMENT.sub(replacement, text)


def code_of(text, literals=True):
    """The text with blank comments and blank preprocessor directives, so that a comment or the
    body of a macro is not read as a function or as a test."""
    return DIRECTIVE.sub(lambda m: blank(m.group()), uncommented(text, literals))


def line_of(text, position):
    return text.count('\n', 0, position) + 1


def closing(text, start):
    """The position after the bracket that closes the bracket at `start`. Brackets in string
    literals do not count."""
    opener = text[start]
    closer = {'(': ')', '{': '}'}[opener]
    depth = 0
    for m in BRACKET_OR_STRING.finditer(text, start):
        if m.group() == opener:
            depth += 1
        elif m.group() == closer:
            depth -= 1
            if depth == 0:
                return m.end()
    return len(text)


def calls(code, name=C_NAME):
    """(name, position of the name, text between the brackets, position after the brackets) of each
    `name(...)` in the code. A definition and a declaration of a function have this form too."""
    for m in re.finditer(rf'\b({name})\s*\(', code):
        end = closing(code, m.end() - 1)
        yield m.group(1), m.start(1), code[m.end():end - 1], end


def split_top(text, operator):
    """The parts of the text between the operators that are in no bracket."""
    parts, depth, start, i = [], 0, 0, 0
    while i < len(text):
        depth += text[i] in '([{'
        depth -= text[i] in ')]}'
        if depth == 0 and text.startswith(operator, i):
            parts.append(text[start:i].strip())
            start = i = i + len(operator)
        else:
            i += 1
    return parts + [text[start:].strip()]


# ---- The contracts of the annotated C files ----

CLAUSE = re.compile(r'\s*__CPROVER_(requires|ensures|assigns|frees)\s*\(')
DEFINE = re.compile(r'^[ \t]*#[ \t]*define[ \t]+(\w+)(\([^)\n]*\))?[ \t]*([^\n]*)$', re.M)


def spec_files(spec):
    """The annotated C files: the C files directly in a directory, or one file."""
    if os.path.isdir(spec):
        paths = sorted(os.path.join(spec, name) for name in os.listdir(spec) if name.endswith('.c'))
    else:
        paths = [spec]
    paths = [path for path in paths if os.path.isfile(path)]
    if not paths:
        raise SpecError(f'no C file with contracts in {spec}')
    return paths


def contracts(spec):
    """Function name -> contract, for each function with a contract in the annotated C files."""
    found = {}
    for path in spec_files(spec):
        for name, contract in contracts_of(path).items():
            found.setdefault(name, contract)
    return found


def contracts_of(path):
    """Function name -> contract, for each function of one annotated C file that has clauses
    between its parameter list and its body. A contract has the keys file, line, parameters,
    clauses, comments and macros (see macros_of, for the file). When a name has two contracts, the
    first one counts."""
    text = read(path)
    code, plain = code_of(text, literals=False), uncommented(text)
    macros = macros_of(plain)
    found = {}
    for name, start, parameters, end in calls(code):
        if name in NOT_FUNCTIONS or name.startswith('__CPROVER_') or not CLAUSE.match(code, end):
            continue
        clauses, stop = clauses_at(code, plain, end)
        found.setdefault(name, {'file': path, 'line': line_of(code, start),
                                'parameters': parameter_names(parameters), 'clauses': clauses,
                                'comments': comments_between(text, end, stop), 'macros': macros})
    return found


def macros_of(plain):
    """Name -> (parameters or None, body) of each macro that a text without comments defines."""
    return {m.group(1): (m.group(2), ' '.join(m.group(3).split()))
            for m in DEFINE.finditer(plain.replace('\\\n', ' '))}


def clauses_at(code, plain, position):
    """([(kind, line, text)], position after the last clause) of the clauses that start at the
    position. `plain` gives the text of a clause, with its literals."""
    clauses = []
    while m := CLAUSE.match(code, position):
        position = closing(code, m.end() - 1)
        text = ' '.join(plain[m.end():position - 1].split())
        clauses.append((m.group(1), line_of(code, m.start(1)), text))
    return clauses, position


def comments_between(text, start, stop):
    """(line, words) of each comment between two positions, and to the end of the line of `stop`."""
    line_end = text.find('\n', stop)
    found = LITERAL_OR_COMMENT.finditer(text, start, len(text) if line_end < 0 else line_end)
    return [(line_of(text, m.start()), ' '.join(line.strip(' \t/*') for line in m.group().split('\n')).strip())
            for m in found if m.group().startswith('/')]


def parameter_names(parameters):
    """The names of the parameters of a parameter list. A parameter without a name has none."""
    names = []
    for parameter in split_top(parameters, ','):
        pointer = re.search(rf'\(\s*\*\s*({C_NAME})\s*\)', parameter)
        words = re.findall(C_NAME, re.sub(r'\[[^\]]*\]', '', parameter))
        if pointer:
            names.append(pointer.group(1))
        elif len(words) > 1:
            names.append(words[-1])
    return names


def clause_parts(kind, text):
    """The parts of a clause: a requires clause that is `a && b` has one part for a and one for b.
    Each other clause is one part."""
    lower = any(len(split_top(text, operator)) > 1 for operator in ('||', '?', '==>', ','))
    return split_top(text, '&&') if kind == 'requires' and not lower else [text]


def operands(text):
    """The operands of a condition: the text is split at each `==>`, `||` and `&&` that is in no
    bracket, also in an operand that is in brackets. A text with a `,` or a `?` in no bracket is
    one operand."""
    if len(split_top(text, ',')) > 1:
        return [text]
    for operator in ('==>', '?', '||', '&&'):
        parts = split_top(text, operator)
        if len(parts) > 1:
            return [text] if operator == '?' else [operand for part in parts for operand in operands(part)]
    if text.startswith('(') and closing(text, 0) == len(text):
        return operands(text[1:-1].strip())
    return [text]


def numbered_parts(contract):
    """(id, kind, line, text) of each part of each clause. The id of a part is the number of its
    clause, or `number.part` for a clause with two or more parts."""
    for number, (kind, line, text) in enumerate(contract['clauses'], 1):
        parts = clause_parts(kind, text)
        for part_number, part in enumerate(parts, 1):
            yield (f'{number}' if len(parts) == 1 else f'{number}.{part_number}'), kind, line, part


def clause_ids(contract):
    """The ids that name a clause or a part of a clause of the contract."""
    numbers = {f'{number}' for number in range(1, len(contract['clauses']) + 1)}
    return numbers | {part_id for part_id, *_ in numbered_parts(contract)}


# ---- The C source ----

VISIBILITY = ('PRIVATE', 'PUBLIC', 'INTERFACE')
# A declaration has a type before the name: a word or a `*`. A call after one of NOT_TYPES is none.
TYPE_BEFORE = re.compile(r'(\w+|\*)\s*$')
NOT_TYPES = ('return', 'else', 'do')
SEMICOLON_AFTER = re.compile(r'\s*;')
BODY_AFTER = re.compile(r'\s*\{')
CONDITIONAL = re.compile(r'^[ \t]*#[ \t]*(if|ifdef|ifndef|elif|else|endif)\b(?:[^\n]*\\\n)*[^\n]*', re.M)
LINE_MARKER = re.compile(r'# (\d+) "((?:[^"\\]|\\.)*)"')
# The compiler that reads the preprocessor directives of the C source.
COMPILER = 'gcc'


def source_files(c_src, suffix):
    """The paths of the files of the C source with the suffix, relative to it, in sorted order."""
    if not os.path.isdir(c_src):
        raise SpecError(f'{c_src} is not a directory')
    found = []
    for directory, subdirs, files in os.walk(c_src):
        subdirs.sort()
        found += [os.path.relpath(os.path.join(directory, name), c_src)
                  for name in sorted(files) if name.endswith(suffix)]
    return found


def headers(c_src):
    """(path, text, code, function name -> position of its declaration in the code) of each header
    of the C source."""
    found = []
    for path in source_files(c_src, '.h'):
        text = read(os.path.join(c_src, path))
        code = code_of(text, literals=False)
        found.append((os.path.join(c_src, path), text, code, declarations(code)))
    return found


def declarations(code):
    """Function name -> position of the name, for each function declaration `type name(...);` in
    the code of a header."""
    found = {}
    for name, start, _, end in calls(code):
        before = TYPE_BEFORE.search(code, max(0, start - 200), start)
        if (name not in NOT_FUNCTIONS and before and before.group(1) not in NOT_TYPES
                and SEMICOLON_AFTER.match(code, end)):
            found.setdefault(name, start)
    return found


def statement_line(code, position):
    """The line where the statement that has the position starts."""
    start = max(code.rfind(c, 0, position) for c in ';{}') + 1
    return line_of(code, start + len(code[start:position]) - len(code[start:position].lstrip()))


def comment_blocks(text):
    """(first line, last line, text) of each comment that starts its line. `//` comments on
    adjacent lines are one block."""
    blocks = []
    for m in LITERAL_OR_COMMENT.finditer(text):
        comment = m.group()
        if not comment.startswith('/') or text[text.rfind('\n', 0, m.start()) + 1:m.start()].strip():
            continue
        first, last = line_of(text, m.start()), line_of(text, m.end())
        before = blocks[-1] if blocks else (0, 0, '')
        if comment.startswith('//') and before[2].startswith('//') and before[1] == first - 1:
            blocks[-1] = (before[0], last, before[2] + '\n' + comment)
        else:
            blocks.append((first, last, comment))
    return blocks


def first_words(comment):
    """The first line of a comment that has a word, without the comment marks."""
    for line in comment.split('\n'):
        line = line.strip(' \t/*!<-^=#')
        if re.search(r'\w', line):
            return line
    return ''


def c_definitions(c_src, name, preferred):
    """(path, line) of each definition of the function in the C files of the C source. The files
    with the base name `preferred` are first."""
    paths = sorted(source_files(c_src, '.c'), key=lambda path: os.path.basename(path) != preferred)
    found = []
    for path in (os.path.join(c_src, path) for path in paths):
        text = read(path)
        if name in text:
            code = code_of(text, literals=False)
            found += [(path, line_of(code, start)) for _, start, _, end in calls(code, re.escape(name))
                      if BODY_AFTER.match(code, end)]
    return found


def conditions_around(path, line):
    """(line, directive) of the #if, #elif and #else directives that are open at a line of a file."""
    text = uncommented(read(path))
    stack = []
    for m in CONDITIONAL.finditer(text):
        number = line_of(text, m.start())
        if number >= line:
            break
        if m.group(1).startswith('if'):
            stack.append([])
        if m.group(1) == 'endif':
            del stack[-1:]
        elif stack:
            stack[-1].append((number, ' '.join(m.group().replace('\\\n', ' ').split())))
    return [directive for level in stack for directive in level]


def compile_definitions(c_src):
    """The definitions of the target_compile_definitions(...) calls in the CMakeLists.txt of the C
    source, as NAME or NAME=VALUE."""
    try:
        text = read(os.path.join(c_src, 'CMakeLists.txt'))
    except OSError:
        return []
    found = []
    for m in re.finditer(r'\btarget_compile_definitions\s*\(([^)]*)\)', text):
        found += [word for word in m.group(1).split()[1:]
                  if word not in VISIBILITY and re.fullmatch(r'[A-Za-z_][\w=]*', word)]
    return list(dict.fromkeys(found))


def defined_macros(c_src, definitions):
    """The names of the macros that the files of the C source or its compile definitions define."""
    names = {definition.split('=')[0] for definition in definitions}
    for path in source_files(c_src, '.c') + source_files(c_src, '.h'):
        names.update(m.group(1) for m in DEFINE.finditer(read(os.path.join(c_src, path))))
    return names


def preprocess(path, all_headers, definitions, *options):
    """The output of the preprocessor for a C file, with the directories of the headers and the
    compile definitions of the C source. None when the compiler does not run or reports an error."""
    includes = sorted({os.path.dirname(header) for header, *_ in all_headers})
    command = [COMPILER, '-E', *options, *(f'-I{d}' for d in includes), *(f'-D{d}' for d in definitions), path]
    try:
        result = subprocess.run(command, capture_output=True, text=True, errors='replace', timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout if result.returncode == 0 else None


def built_lines(output, path):
    """The numbers of the lines of a file that have text in the output of the preprocessor. A line
    in an #if branch that the build does not select has none."""
    lines, current, number = set(), None, 0
    for text in output.split('\n'):
        marker = LINE_MARKER.match(text)
        if marker:
            number, current = int(marker.group(1)) - 1, marker.group(2)
            continue
        number += 1
        if current == path and text.strip():
            lines.add(number)
    return lines


def macro_values(output):
    """Macro name -> value, for each #define in the output of the preprocessor with -dM."""
    return {m.group(1): m.group(3) if m.group(2) is None
            else f'function-like macro {m.group(1)}{m.group(2)} {m.group(3)}'
            for m in DEFINE.finditer(output)}


# ---- list, show and notes ----

# Constructs that set up the proof when they are in a requires clause. Each remark tells what the
# construct does not say.
SETUP = [
    ('__CPROVER_allocate', 'proof setup: the contract makes this buffer itself. Read it only as "the buffer has '
     'this many bytes". It does not say that the pointer is not NULL, or that the buffers are separate.'),
    ('__CPROVER_is_fresh', 'proof setup: the proof gives the pointer a new object of this size. Read it only as '
     '"the buffer has this many bytes". It does not say that the pointer is not NULL, or that the buffers are '
     'separate.'),
    ('__CPROVER_assume', 'proof setup: an assumption that limits the proof. It is not a rule for callers.'),
]
# The meaning of the other constructs.
CONSTRUCTS = [
    ('__CPROVER_return_value', '__CPROVER_return_value is the result of the function.'),
    ('__CPROVER_old', '__CPROVER_old(x) is the value of x when the function starts.'),
    ('__CPROVER_forall', '__CPROVER_forall { T i; c }: c is true for each value of i.'),
    ('__CPROVER_same_object', '__CPROVER_same_object(p, q): p and q point into the same object.'),
    ('__CPROVER_r_ok', '__CPROVER_r_ok(p, n): n bytes at p are readable.'),
    ('__CPROVER_w_ok', '__CPROVER_w_ok(p, n): n bytes at p are writable.'),
    ('__CPROVER_rw_ok', '__CPROVER_rw_ok(p, n): n bytes at p are readable and writable.'),
    ('__CPROVER_object_whole', '__CPROVER_object_whole(p): all bytes of the object that p points into.'),
    ('__CPROVER_object_upto', '__CPROVER_object_upto(p, n): the n bytes that start at p.'),
    ('__CPROVER_POINTER_OFFSET', '__CPROVER_POINTER_OFFSET(p) is the offset of p in its object.'),
    ('__CPROVER_OBJECT_SIZE', '__CPROVER_OBJECT_SIZE(p) is the size of the object that p points into.'),
    ('__CPROVER_memory_leak', '__CPROVER_memory_leak: the proof examines that the function leaks no memory. A '
     'clause with it says nothing about argument values.'),
]
# The meaning of the constructs of SETUP in a clause that is not a requires clause.
RESULT_CONSTRUCTS = [
    ('__CPROVER_is_fresh', '__CPROVER_is_fresh(p, n) in an ensures clause: when the function returns, p points '
     'to a new object of n bytes.'),
    ('__CPROVER_allocate', '__CPROVER_allocate(n, 0) in an ensures clause: a new object of n bytes.'),
]


def declared_contracts(spec, all_headers):
    """Function name -> contract, for each function with a contract that a header declares. These
    are the functions that a test can call, and each command reads only these."""
    declared = set().union(*(names for *_, names in all_headers))
    return {name: contract for name, contract in contracts(spec).items() if name in declared}


def contract_of(found, name):
    """The contract of one function. Raises SpecError for a function without a contract."""
    if name not in found:
        raise SpecError(f'no contract for the function {name}. The list command prints the functions with a '
                        'contract.')
    return found[name]


def show(name, contract, c_src):
    """What the commands know about one function with a contract, as text."""
    all_headers, definitions = headers(c_src), compile_definitions(c_src)
    c_macros = defined_macros(c_src, definitions)
    macros = {macro: value for macro, value in contract['macros'].items() if macro not in c_macros}
    found = [(path, line, preprocess(path, all_headers, definitions))
             for path, line in c_definitions(c_src, name, os.path.basename(contract['file']))]
    clauses, by_parameter, names, result_names = clause_lines(contract, macros)
    values = preprocess(found[0][0], all_headers, definitions, '-dM') if found else None
    meanings = [meaning for construct, meaning in CONSTRUCTS if construct in names]
    meanings += [meaning for construct, meaning in RESULT_CONSTRUCTS if construct in result_names]
    return '\n'.join([
        f"# {name}({', '.join(contract['parameters'])})", '',
        f"contract: {contract['file']}:{contract['line']}",
        *(definition_lines(found) or [f'C definition: not found in {c_src}', 'in the build: unknown']),
        *header_lines(name, all_headers), '',
        'comments in the contract:',
        *([f'  spec line {line}: {words}' for line, words in contract['comments']] or ['  none']), '',
        *value_lines(sorted(names & c_macros), values, definitions),
        'clauses:', *clauses, '',
        'clauses by parameter:',
        *(f"  {parameter}: {', '.join(ids) or 'no clause'}" for parameter, ids in by_parameter.items()), '',
        'constructs in the clauses:',
        *([f'  {meaning}' for meaning in meanings] or ['  none']),
    ])


def definition_lines(found):
    """The place of each C definition, and whether the build has it. `found` has the path, the
    line and the output of the preprocessor for each definition."""
    lines = []
    for path, line, output in found:
        lines.append(f'C definition: {path}:{line}')
        if output is not None:
            lines.append('in the build: ' + ('yes' if line in built_lines(output, path) else 'no'))
            continue
        conditions = conditions_around(path, line)
        lines.append('in the build: unknown. The compiler did not read the file. The definition is in '
                     + ('these conditions:' if conditions else 'no #if condition.'))
        lines += [f'  {path}:{number}: {directive}' for number, directive in conditions]
    return lines


def header_lines(name, all_headers):
    """The place of the declaration, and each comment block of the headers that names the function."""
    declaration, comments = 'not found', []
    for path, text, code, names in all_headers:
        above = None
        if name in names and declaration == 'not found':
            declaration = f'{path}:{line_of(code, names[name])}'
            above = statement_line(code, names[name]) - 1
        for first, last, comment in comment_blocks(text) if name in text else []:
            if re.search(rf'\b{name}\b', comment):
                mark = ' (directly above the declaration)' if last == above else ''
                comments.append(f'  {path}:{line_range(first, last)}{mark}: {first_words(comment)}')
    return [f'header declaration: {declaration}', '',
            f'header comments that name {name}:', *(comments or ['  none'])]


def line_range(first, last):
    return f'{first}' if first == last else f'{first}-{last}'


def value_lines(names, output, definitions):
    """The value in the build of each named macro. `output` is the output of the preprocessor with
    -dM, or None."""
    if not names:
        return []
    values = macro_values(output) if output is not None else {}
    missing = 'not defined in this build' if output is not None else 'unknown. The compiler did not read the file.'
    command = ' '.join([COMPILER, '-dM', '-E', *(f'-D{d}' for d in definitions)])
    return [f'macros in the clauses, value in this build ({command}):',
            *(f'  {name} = {values.get(name, missing)}' for name in names), '']


def clause_lines(contract, macros):
    """(the lines of the clauses, parameter -> ids of the parts that name it, the names in the
    clauses, the names in the clauses that are not requires clauses) of a contract. `macros` are
    the macros that only the contracts use."""
    lines, names, result_names = [], set(), set()
    by_parameter = {parameter: [] for parameter in contract['parameters']}
    for part_id, kind, line, text in numbered_parts(contract):
        text, used = expand(text, macros)
        lines.append(f'  [{part_id}] {kind} (spec line {line}): {text}')
        lines += [f'        {mark}' for mark in condition_marks(text)]
        if kind == 'requires':
            lines += [f'        {mark}' for mark in setup_marks(text, contract['parameters'])]
        lines += [f'        note: {macro}{macros[macro][0]} is a macro of the contracts only: {body}'
                  for macro, body in used.items()]
        own = set(re.findall(C_NAME, ' '.join([text, *used.values()])))
        names |= own
        result_names |= own if kind != 'requires' else set()
        for parameter in contract['parameters']:
            if re.search(rf'\b{parameter}\b', text):
                by_parameter[parameter].append(part_id)
    return lines, by_parameter, names, result_names


def expand(text, macros):
    """(the text with the body of each macro without parameters in its place, name -> body of the
    macros with parameters that the text or one of these bodies uses)."""
    text = with_values(text, macros)
    used, todo = {}, [text]
    while todo:
        current = todo.pop()
        for name, (parameters, body) in macros.items():
            if parameters is not None and name not in used and re.search(rf'\b{name}\b', current):
                used[name] = with_values(body, macros)
                todo.append(used[name])
    return text, used


def with_values(text, macros):
    """The text with the body of each macro without parameters in its place, until no such macro
    is left. A macro that names itself stops after one round for each macro."""
    for _ in macros:
        before = text
        for name, (parameters, body) in macros.items():
            if parameters is None:
                text = re.sub(rf'\b{name}\b', lambda _: f'({body})', text)
        if text == before:
            break
    return text


def condition_marks(text):
    """The condition and the result of a part of a clause that is `A ==> B`."""
    condition, *result = split_top(text, '==>')
    return [f'if: {condition}', f"then: {' ==> '.join(result)}"] if result else []


def setup_marks(text, parameters):
    """The marks of proof setup in one part of a requires clause. When only some operands of the
    part are proof setup, each mark names its operand."""
    found = []
    parts = operands(text)
    for operand in parts:
        remarks = [remark for construct, remark in SETUP if construct in operand]
        assigned = [p for p in parameters if re.search(rf'(?<![.>\w]){p}\s*=(?!=)', operand)]
        if assigned:
            remarks.append(f"the clause assigns a value to {', '.join(assigned)}. The contract makes this value "
                           'itself.')
        if remarks and len(parts) > 1:
            found.append(f'SETUP in part: {operand}')
            found += [f'      {remark}' for remark in remarks]
        else:
            found += [f'SETUP {remark}' for remark in remarks]
    return found


def notes(all_headers, functions):
    """One line for each comment block that is not directly above a function declaration, in the
    headers that declare one of the functions."""
    lines = []
    for path, text, code, names in all_headers:
        if not names.keys() & functions:
            continue
        above = {statement_line(code, position) - 1 for position in names.values()}
        lines += [f'{path}:{line_range(first, last)}: {first_words(comment)}'
                  for first, last, comment in comment_blocks(text) if last not in above]
    return lines


# ---- The tests of verify_env/ ----

# Directories of verify_env/ that hold no test source: build output, documents, saved inputs.
SKIPPED_DIRS = ('build', '_deps', 'dist', 'docs', 'fuzz-artifacts')
SOURCE_SUFFIXES = ('.cc', '.cpp', '.h', '.inc')
TEST_DEF = re.compile(r'\b(FUZZ_TEST_F|FUZZ_TEST|TEST_F|TEST_P|TEST)\s*\(\s*(\w+)\s*,\s*(\w+)\s*\)')
CHAIN_CALL = re.compile(r'\s*\.\s*(\w+)\s*\(')
FUNCTION_DEF = re.compile(r'\b(\w+)\s*\([^;{}()]*(?:\([^()]*\)[^;{}()]*)*\)\s*(?:const\s*)?\{')
STRING_OR_NAME = re.compile(STRING + r'|\b[A-Za-z_]\w*')


def read_sources(root):
    """Path -> text of each test source file under root."""
    if not os.path.isdir(root):
        raise SpecError(f'{root} is not a directory')
    texts = {}
    for directory, subdirs, files in os.walk(root):
        subdirs[:] = sorted(s for s in subdirs if not s.startswith(SKIPPED_DIRS))
        for name in sorted(files):
            if name.endswith(SOURCE_SUFFIXES):
                path = os.path.join(directory, name)
                texts[path] = read(path)
    return texts


def code_texts(texts):
    """Path -> code of each test source (see code_of): where the tests and their functions are."""
    return {path: code_of(text) for path, text in texts.items()}


def named_in(texts):
    """The names in the test sources, also in the bodies of their macros. A name in a comment or in
    a string literal does not count."""
    return names_in('\n'.join(uncommented(text, literals=False) for text in texts.values()))


def test_definitions(texts):
    """(macro, 'Suite.Name', 'path:line', path, position after the macro call) of each TEST and
    FUZZ_TEST in the code of the test sources."""
    for path, text in texts.items():
        for m in TEST_DEF.finditer(text):
            yield m.group(1), f'{m.group(2)}.{m.group(3)}', f'{path}:{line_of(text, m.start())}', path, m.end()


def tests_in(texts):
    """The 'Suite.Name' of each TEST and FUZZ_TEST in the code of the test sources."""
    return {name for _, name, *_ in test_definitions(texts)}


def names_in(text):
    """The names in a text. A name in a string literal does not count."""
    return {name for name in STRING_OR_NAME.findall(text) if not name.startswith('"')}


def chained_calls(text, position):
    """Method name -> argument text of the calls that follow a FUZZ_TEST(...), as .WithDomains(...)."""
    found = {}
    while m := CHAIN_CALL.match(text, position):
        position = closing(text, m.end() - 1)
        found[m.group(1)] = ' '.join(text[m.end():position - 1].split())
    return found


def function_bodies(texts):
    """Path -> (function name -> the bodies of the functions with that name in the file)."""
    bodies = {}
    for path, text in texts.items():
        own = bodies[path] = {}
        for m in FUNCTION_DEF.finditer(text):
            if m.group(1) not in NOT_FUNCTIONS:
                body = text[m.end() - 1:closing(text, m.end() - 1)]
                own[m.group(1)] = own.get(m.group(1), '') + body
    return bodies


def body_of(name, path, bodies):
    """The bodies of the function `name` for a caller in the file `path`: those of that file, or
    those of the other files when that file has none. Two test files can have different functions
    with one name."""
    if name in bodies[path]:
        return bodies[path][name]
    return ''.join(own[name] for own in bodies.values() if name in own)


def reached_functions(name, path, bodies, functions):
    """The functions of `functions` that the function `name` of the file `path` reaches: their
    names are in its body, or in the body of a function of the test files that it names, to any
    depth. A name is thus a call or an argument (a function pointer)."""
    seen, todo = set(), [name]
    while todo:
        names = names_in(body_of(todo.pop(), path, bodies))
        todo += names - seen
        seen |= names
    return [f for f in functions if f in seen]


def properties(texts, functions, wanted=()):
    """One line for each FUZZ_TEST in the texts: name, place, domains, seeds, and the functions of
    `functions` that it reaches. With `wanted`, only the properties that reach one of these
    functions."""
    bodies = function_bodies(texts)
    lines = []
    for macro, name, place, path, position in test_definitions(texts):
        if not macro.startswith('FUZZ'):
            continue
        reaches = reached_functions(name.split('.')[1], path, bodies, functions)
        if wanted and not set(wanted) & set(reaches):
            continue
        chain = chained_calls(texts[path], position)
        # .WithSeeds is a call of the chain, or a call on a domain in .WithDomains(...).
        seeds = 'WithSeeds' in ' '.join([*chain, *chain.values()])
        lines.append('\t'.join([name, place,
                                'domains: ' + chain.get('WithDomains', '(default domains)'),
                                'seeds: ' + ('yes' if seeds else 'no'),
                                'reaches: ' + (', '.join(reaches) or 'none found')]))
    return lines


# ---- check ----

HEADER = '| function | parameters | values | kind | C result | source | test | note |'
COLUMNS = [cell.strip() for cell in HEADER.strip('|').split('|')]
# The kinds of a row. A test passes values of the first two kinds only.
TESTED_KINDS = ('must cover', 'C rejects')
UNTESTED_KINDS = ('never pass', 'unsure')
KINDS = TESTED_KINDS + UNTESTED_KINDS
# A line or a range of lines of the C source in the cell `source`.
SOURCE_LINES = re.compile(r'inputs/c_src/(.+):(\d+)(?:-(\d+))?')


def table_rows(text):
    """(line number, cells) of each table row of a DOMAINS.md, without the rows of dashes. A cell
    has no backticks around it."""
    rows = []
    for number, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line.startswith('|'):
            continue
        # One pipe at the start and one pipe at the end are not between two cells.
        line = line[1:-1] if line.endswith('|') and not line.endswith('\\|') else line[1:]
        cells = [cell.strip(' `') for cell in re.split(r'(?<!\\)\|', line)]
        if not all(re.fullmatch(r':?-+:?', cell) for cell in cells):
            rows.append((number, cells))
    return rows


def check(rows, functions, tests, test_names, c_src):
    """The problems of a DOMAINS.md, one line each. `functions` are the functions with a contract,
    `tests` the tests of verify_env/, and `test_names` the names in the test sources."""
    if not rows or rows[0][1] != COLUMNS:
        return [f'the first table row is not the header row: {HEADER}']
    problems, with_row = [], set()
    for number, cells in rows[1:]:
        if cells != COLUMNS:
            problems += [f'line {number}: {problem}' for problem in row_problems(cells, functions, tests, c_src)]
            with_row.add(cells[0])
    return problems + [f'{function}: the tests name this function, but it has no row'
                       for function in functions if function in test_names and function not in with_row]


def row_problems(cells, functions, tests, c_src):
    """The problems of one row."""
    if len(cells) != len(COLUMNS):
        return [f'the row has {len(cells)} cells, not {len(COLUMNS)}']
    function, _, _, kind, _, source, test, note = cells
    if function not in functions:
        return [f'no contract for the function {function}']
    names = [name.strip(' `') for name in test.split(',') if name.strip(' `')]
    problems = [] if kind in KINDS else [f"the kind '{kind}' is not one of: {', '.join(KINDS)}"]
    problems += source_problems(source, function, clause_ids(functions[function]), c_src)
    problems += [f'no TEST or FUZZ_TEST has the name {name}' for name in names if name not in tests]
    if kind in UNTESTED_KINDS and names:
        problems.append(f"a row of the kind '{kind}' names a test ({', '.join(names)})")
    if kind in TESTED_KINDS and not names and not note.startswith(NOT_COVERED):
        problems.append(f"no test, and the note does not start with '{NOT_COVERED}'")
    return problems


def source_problems(source, function, ids, c_src):
    """The items of the cell `source` that are no clause id of the function (one of `ids`) and no
    lines of a file of the C source."""
    items = [item.strip(' `') for item in re.split(r'[,;]', source) if item.strip(' `')]
    problems = [] if items else ['the source is empty']
    for item in items:
        place = SOURCE_LINES.fullmatch(item)
        if place:
            first, last = int(place.group(2)), int(place.group(3) or place.group(2))
            correct = 1 <= first <= last <= line_count(os.path.join(c_src, place.group(1)))
        else:
            correct = item.startswith('[') and item.endswith(']') and item[1:-1] in ids
        if not correct:
            problems.append(f"the source item '{item}' is no clause id of {function} (as [3] or [1.2]) and no "
                            f'line of a file of the C source (as inputs/c_src/<path>:<line>)')
    return problems


def line_count(path):
    """The number of lines of a file, or 0 for a file that cannot be read."""
    try:
        return len(read(path).splitlines())
    except OSError:
        return 0


# ---- Commands ----

def run_list(args):
    for name, contract in declared_contracts(args.spec, headers(args.c_src)).items():
        print(f"{name}\tclauses: {len(contract['clauses'])}\t{contract['file']}:{contract['line']}")
    return 0


def run_show(args):
    functions = declared_contracts(args.spec, headers(args.c_src))
    print(show(args.function, contract_of(functions, args.function), args.c_src))
    return 0


def run_notes(args):
    all_headers = headers(args.c_src)
    lines = notes(all_headers, declared_contracts(args.spec, all_headers).keys())
    print('\n'.join(lines) if lines else 'No comment block found.')
    return 0


def run_check(args):
    functions = declared_contracts(args.spec, headers(args.c_src))
    try:
        rows = table_rows(read(args.domains))
    except OSError as e:
        raise SpecError(f'cannot read {args.domains}: {e.strerror}')
    texts = read_sources(args.tests)
    problems = check(rows, functions, tests_in(code_texts(texts)), named_in(texts), args.c_src)
    print('\n'.join(problems) if problems else NO_PROBLEMS)
    return 1 if problems else 0


def run_properties(args):
    functions = declared_contracts(args.spec, headers(args.c_src))
    for function in args.functions:
        contract_of(functions, function)  # an error for a function without a contract
    texts = code_texts(read_sources(args.tests))
    lines = properties(texts, list(functions), args.functions)
    if lines:
        print('\n'.join(lines))
    elif args.functions and any(macro.startswith('FUZZ') for macro, *_ in test_definitions(texts)):
        print(f"No FUZZ_TEST reaches {', '.join(args.functions)}.")
    else:
        print('No FUZZ_TEST found.')
    return 0


def parser():
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument('--spec', default=SPEC, metavar='DIR',
                        help=f'the directory of the C files with contracts (default {SPEC})')
    common.add_argument('--c-src', default=C_SRC, metavar='DIR', help=f'the C source (default {C_SRC})')
    common.add_argument('--tests', default=TESTS, metavar='DIR', help=f'the test sources (default {TESTS})')
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = p.add_subparsers(dest='command', required=True, metavar='COMMAND')

    def command(name, run, text):
        sub = commands.add_parser(name, parents=[common], help=text, description=text)
        sub.set_defaults(run=run)
        return sub

    command('list', run_list, 'Print the functions that have a contract and a declaration in a header.')
    sub = command('show', run_show, 'Print the contract of one function: its places in the C source, its '
                  'clauses with an id each, and the build value of each macro in the clauses.')
    sub.add_argument('function', metavar='FUNCTION')
    command('notes', run_notes, 'Print the place and the first line of each header comment that is not directly '
            'above a function declaration.')
    sub = command('check', run_check, 'Compare DOMAINS.md with the contracts, the C source and the tests.')
    sub.add_argument('domains', metavar='DOMAINS.md')
    sub = command('properties', run_properties, 'Print each FUZZ_TEST with its place, its domains, and the '
                  'functions with a contract that it reaches.')
    sub.add_argument('functions', nargs='*', metavar='FUNCTION',
                     help='print only the properties that reach one of these functions')
    return p


def main(argv=None):
    args = parser().parse_args(argv)
    try:
        return args.run(args)
    except SpecError as e:
        print(f'Error: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
