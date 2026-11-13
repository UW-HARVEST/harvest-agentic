#!/usr/bin/env python3
"""Lists the candidate list types of one analysis report, and writes a card stub for each list type.

    sll_cards.py REPORT.json C_SRC_DIR [--cards OUT_DIR]

Prints one line for each candidate type of the report. With --cards, also writes one Markdown card
stub for each type that is defined in C_SRC_DIR. A card that exists stays as it is. The class of a
type comes only from the report:

  outside     the type is not defined in C_SRC_DIR (for example a system header): no card
  broken      the analysis found a function that breaks the chain shape
  proved      verdict "proved", one function or more analysed, and a complete source scan
  not-proved  all other cases ("refuted" without a found violation, "unknown", incomplete scan)

See ../SKILL.md.
"""
import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

SCHEMA_VERSION = 1
NO_TYPES = 'No list types.'

KIND_NAMES = {str: 'a string', int: 'an integer', bool: 'a boolean', list: 'a list', dict: 'an object'}

CARD = """# {type}

## From the analysis
- Defined at: `inputs/c_src/{defined_at}`{macro}
- Link field: `{link_field}`
- Known holders: {holders}
- Shape: {shape}

## From the source (the list scout fills this part)
- Functions that use the type or the link field:
- Functions that write the link field:
- Read-only functions:
- Single-node functions:
- Nodes are allocated at:
- Nodes are freed at:
- Node pointers in exported signatures or public structs:
- Other pointers into the chain that stay after a call:
- Node layout: fixed size / variable size

## Decision
- Representation: OWNED-SEQUENCE / OWNED-CHAIN / BORROWED / C-LAYOUT
- Reason:
- Views: YES / NO (C-LAYOUT only), and the reason
- Rust types:
"""
SHAPE = {
    'proved': 'PROVED. The analysis proved the chain shape for {proved} functions{optimistic}.',
    'not-proved': 'NOT PROVED. {cause} This does not show that the type is not a list.',
    'broken': 'BROKEN. The analysis found a function that breaks the chain shape.',
}
# Why a type that the analysis did not break is not proved.
NOT_PROVED_CAUSE = {
    'no-function': 'The analysis examined no function for this type.',
    'incomplete-scan': 'The analysis proved the chain shape for {proved} functions, but it did not scan '
                       'all source files.',
    'some-functions': 'The analysis proved the chain shape for {proved} functions and has no result for '
                      'the others.',
    'no-verdict': 'The analysis proved the chain shape for {proved} functions, but it has no result for '
                  'the type.',
}


class ReportError(Exception):
    """The report cannot be used. The message tells why."""


@dataclass
class Record:
    """One candidate type of the report."""
    type: str
    cls: str            # outside, broken, proved or not-proved
    defined_at: str     # path in C_SRC_DIR and line, or the path of the report for an outside type
    from_macro: bool    # the definition line does not contain the type name
    link_field: str
    holders: list       # struct fields that point to a node, as "record.field"
    proved: str         # "<proved> of <analysed>"
    shape: str          # the sentence of the card about the chain shape, empty for an outside type


def field(obj, path, kind):
    """The value at the dotted `path` of the JSON object `obj`, which must be a `kind`."""
    value = obj
    for key in path.split('.'):
        if not isinstance(value, dict) or key not in value:
            raise ReportError(f'the field {path} is missing')
        value = value[key]
    if not isinstance(value, kind) or (kind is int and isinstance(value, bool)):
        raise ReportError(f'the field {path} is not {KIND_NAMES[kind]}')
    return value


def load_report(path):
    """The report as a JSON object, after the check of its schema version."""
    try:
        report = json.loads(path.read_text())
    except OSError as e:
        raise ReportError(f'cannot read {path}: {e.strerror}') from None
    except ValueError as e:
        raise ReportError(f'{path} is not JSON: {e}') from None
    if not isinstance(report, dict):
        raise ReportError(f'{path} is not a JSON object')
    version = report.get('schema_version')
    if version != SCHEMA_VERSION:
        raise ReportError(f'{path} has schema_version {version!r}. This tool reads only version {SCHEMA_VERSION}.')
    return report


def source_path(file, source_dir, c_src):
    """The path of `file` relative to C_SRC_DIR, or None when the file is not in C_SRC_DIR.

    A relative `file` is relative to the source directory of the analysis. An absolute `file` must
    contain `source_dir`, the source directory that the report records.
    """
    rel = file
    if file.startswith('/'):
        marker = '/' + source_dir.strip('/') + '/'
        if marker == '//' or marker not in file:
            return None
        rel = file.split(marker, 1)[1]
    root = c_src.resolve()
    path = (root / rel).resolve()
    if not path.is_file() or not path.is_relative_to(root):
        return None
    return path.relative_to(root).as_posix()


def classify(cand, soundness):
    """The class of a candidate that is defined in C_SRC_DIR, and the cause when it is not-proved."""
    if field(cand, 'functions.refuted_by.reject_pred_violation', int) > 0:
        return 'broken', None
    analysed = field(cand, 'functions.analysed', int)
    proved = field(cand, 'functions.proved', int)
    complete = field(soundness, 'scrape_complete', bool) and not field(soundness, 'incomplete_files', list)
    if analysed == 0:
        return 'not-proved', 'no-function'
    if field(cand, 'verdict', str) == 'proved':
        return ('proved', None) if complete else ('not-proved', 'incomplete-scan')
    return 'not-proved', ('some-functions' if proved < analysed else 'no-verdict')


def shape_sentence(cls, cause, proved, optimistic):
    """The sentence of a card about the chain shape."""
    cause = NOT_PROVED_CAUSE[cause].format(proved=proved) if cause else ''
    return SHAPE[cls].format(
        proved=proved, cause=cause, optimistic=', with optimistic assumptions' if optimistic else '')


def made_by_macro(type_name, file, line):
    """Whether line `line` of `file` does not contain the name of the type: a macro makes the type there."""
    lines = file.read_text(errors='replace').splitlines()
    if not 1 <= line <= len(lines):
        raise ReportError(f'{file} has no line {line}. The report is not for this C source.')
    name = type_name.split()[-1]
    return re.search(rf'\b{re.escape(name)}\b', lines[line - 1]) is None


def record(name, cand, report, c_src):
    """The record of the candidate `name`."""
    soundness = field(report, 'soundness', dict)
    file = field(cand, 'defined_at.file', str)
    line = field(cand, 'defined_at.line', int)
    path = source_path(file, field(report, 'run.source_dir', str), c_src)
    holders = [f"{re.sub(r'::.*', '', field(h, 'record', str))}.{field(h, 'field', str)}"
               for h in field(cand, 'held_by', list)]
    proved = f"{field(cand, 'functions.proved', int)} of {field(cand, 'functions.analysed', int)}"
    cls, cause = classify(cand, soundness) if path else ('outside', None)
    return Record(
        type=name,
        cls=cls,
        defined_at=f'{path}:{line}' if path else file,
        from_macro=bool(path) and made_by_macro(name, c_src / path, line),
        link_field=field(cand, 'self_pointer.field', str),
        holders=holders,
        proved=proved,
        shape=shape_sentence(cls, cause, proved, field(soundness, 'unsound', bool)) if path else '',
    )


def records(report, c_src):
    """The records of all candidates, in the order of the type names."""
    result = []
    for name, cand in sorted(field(report, 'candidates', dict).items()):
        try:
            result.append(record(name, cand, report, c_src))
        except ReportError as e:
            raise ReportError(f'candidate {name!r}: {e}') from None
    return result


def table_line(r):
    """The output line of one record."""
    return (f'{r.cls:<11} {r.type:<34} {r.defined_at:<34} link={r.link_field:<7} '
            f"proved={r.proved:<10} holders={','.join(r.holders) or '-'}")


def card_name(type_name):
    """The file name of the card of a type: `struct foo` gives `struct_foo.md`."""
    return re.sub(r'\W+', '_', type_name).strip('_') + '.md'


def card_text(r):
    """The card stub of one record. The list scout fills the other parts."""
    return CARD.format(
        type=r.type,
        defined_at=r.defined_at,
        macro=' (a macro makes the type at this line)' if r.from_macro else '',
        link_field=r.link_field,
        holders=', '.join(f'`{h}`' for h in r.holders) or 'none listed',
        shape=r.shape,
    )


def write_cards(list_types, out_dir):
    """Writes the card stubs. A card that exists can have the work of the list scout: it stays."""
    out_dir.mkdir(parents=True, exist_ok=True)
    for r in list_types:
        card = out_dir / card_name(r.type)
        if card.exists():
            print(f'Kept the card that exists: {card}')
        else:
            card.write_text(card_text(r))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('report', type=Path, metavar='REPORT.json', help='the report of the list analysis')
    parser.add_argument('c_src', type=Path, metavar='C_SRC_DIR', help='the C source that the report is for')
    parser.add_argument('--cards', type=Path, metavar='OUT_DIR', help='write one card stub for each list type here')
    args = parser.parse_args()
    if not args.c_src.is_dir():
        sys.exit(f'sll_cards.py: {args.c_src} is not a directory')
    try:
        all_records = records(load_report(args.report), args.c_src)
    except ReportError as e:
        sys.exit(f'sll_cards.py: {e}')
    for r in all_records:
        print(table_line(r))
    list_types = [r for r in all_records if r.cls != 'outside']
    if not list_types:
        print(NO_TYPES)
    elif args.cards:
        write_cards(list_types, args.cards)
        print(f'Number of cards in {args.cards}: {len(list_types)}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
