#!/usr/bin/env python3
"""Tests of sll_cards.py. For maintainers: python3 tests/test_sll_cards.py"""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
HERE = Path(__file__).resolve().parent
TOOL = HERE.parent / 'scripts' / 'sll_cards.py'
sys.path.insert(0, str(TOOL.parent))
import sll_cards  # noqa: E402

SOURCE_DIR = 'corpus/prog/test_case'
C_FILES = {
    'include/list.h': 'struct node {\n    int value;\n    struct node *next;\n};\n',
    'src/gen.c': '#define DECLARE_LIST(T) struct list_##T { T v; struct list_##T *next; }\nDECLARE_LIST(int);\n',
}


def candidate(file='include/list.h', line=1, verdict='proved', analysed=2, proved=2, violations=0, held_by=()):
    return {
        'defined_at': {'file': file, 'line': line, 'column': 8},
        'functions': {'analysed': analysed, 'proved': proved, 'refuted': analysed - proved,
                      'refuted_by': {'reject_pred_top': analysed - proved - violations,
                                     'reject_pred_violation': violations}},
        'held_by': [{'record': r, 'field': f, 'index': 0, 'pointer': True} for r, f in held_by],
        'self_pointer': {'field': 'next', 'index': 1},
        'verdict': verdict,
    }


def report(candidates, **soundness):
    return {
        'schema_version': 1,
        'run': {'source_dir': SOURCE_DIR},
        'soundness': {'scrape_complete': True, 'incomplete_files': [], 'unsound': False, **soundness},
        'candidates': candidates,
    }


class WithCSource(unittest.TestCase):
    """A small C source tree in a temporary directory."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        self.c_src = self.root / 'c_src'
        for name, text in C_FILES.items():
            path = self.c_src / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)

    def tearDown(self):
        self.dir.cleanup()

    def record(self, cand, **soundness):
        """The record of the one candidate `struct node` of a report."""
        (result,) = sll_cards.records(report({'struct node': cand}, **soundness), self.c_src)
        return result


class Classes(WithCSource):
    def test_proved_with_a_complete_scan(self):
        self.assertEqual(self.record(candidate()).cls, 'proved')

    def test_refuted_without_a_violation_is_not_proved(self):
        result = self.record(candidate(verdict='refuted', analysed=7, proved=4))
        self.assertEqual(result.cls, 'not-proved')
        self.assertIn('for 4 of 7 functions and has no result for the others', result.shape)

    def test_a_violation_is_broken_also_with_the_verdict_proved(self):
        for verdict in ('refuted', 'proved'):
            cand = candidate(verdict=verdict, analysed=7, proved=4, violations=1)
            self.assertEqual(self.record(cand).cls, 'broken', verdict)

    def test_an_incomplete_scan_makes_proved_not_proved(self):
        for soundness in ({'scrape_complete': False}, {'incomplete_files': ['src/a.c']}):
            result = self.record(candidate(), **soundness)
            self.assertEqual(result.cls, 'not-proved', soundness)
            self.assertIn('for 2 of 2 functions, but it did not scan all source files', result.shape)

    def test_proved_without_an_analysed_function_is_not_proved(self):
        result = self.record(candidate(analysed=0, proved=0))
        self.assertEqual(result.cls, 'not-proved')
        self.assertIn('examined no function', result.shape)
        self.assertNotIn('0 of 0', result.shape)

    def test_all_functions_proved_without_the_verdict_proved_is_not_proved(self):
        result = self.record(candidate(verdict='unknown', analysed=3, proved=3))
        self.assertEqual(result.cls, 'not-proved')
        self.assertIn('for 3 of 3 functions, but it has no result for the type', result.shape)

    def test_unknown_in_a_system_header_is_outside(self):
        cand = candidate(file='/usr/include/bits/types.h', line=57, verdict='unknown', analysed=0, proved=0)
        result = self.record(cand)
        self.assertEqual((result.cls, result.defined_at), ('outside', '/usr/include/bits/types.h'))


class DefinitionPlace(WithCSource):
    def test_relative_path(self):
        self.assertEqual(self.record(candidate(line=1)).defined_at, 'include/list.h:1')

    def test_absolute_path_that_contains_the_source_dir(self):
        cand = candidate(file=f'/home/someone/{SOURCE_DIR}/include/list.h')
        result = self.record(cand)
        self.assertEqual((result.cls, result.defined_at), ('proved', 'include/list.h:1'))

    def test_absolute_path_without_the_source_dir_is_outside(self):
        cand = candidate(file=str(self.c_src / 'include/list.h'))
        self.assertEqual(self.record(cand).cls, 'outside')

    def test_source_dir_that_matches_a_part_of_a_directory_name_is_outside(self):
        for source_dir in (f'{SOURCE_DIR}_old', f'my{SOURCE_DIR}'):
            file = f'/home/someone/{source_dir}/include/list.h'
            self.assertEqual(self.record(candidate(file=file)).cls, 'outside', file)

    def test_file_that_is_not_in_the_source_is_outside(self):
        self.assertEqual(self.record(candidate(file='include/other.h')).cls, 'outside')

    def test_path_that_leaves_the_source_is_outside(self):
        (self.root / 'secret.h').write_text('struct node;\n')
        for file in ('../secret.h', f'/home/someone/{SOURCE_DIR}/../secret.h'):
            self.assertEqual(self.record(candidate(file=file)).cls, 'outside', file)

    def test_line_without_the_type_name_is_from_a_macro(self):
        cands = {'struct node': candidate(), 'struct list_int': candidate(file='src/gen.c', line=2)}
        by_type = {r.type: r.from_macro for r in sll_cards.records(report(cands), self.c_src)}
        self.assertEqual(by_type, {'struct node': False, 'struct list_int': True})

    def test_line_that_the_file_does_not_have_is_an_error(self):
        with self.assertRaisesRegex(sll_cards.ReportError, "candidate 'struct node'.*has no line 99"):
            self.record(candidate(line=99))


class CommandLine(WithCSource):
    def run_tool(self, report_json, *args):
        path = self.root / 'report.json'
        path.write_text(report_json if isinstance(report_json, str) else json.dumps(report_json))
        return subprocess.run([sys.executable, str(TOOL), str(path), str(self.c_src), *args],
                              capture_output=True, text=True)

    def test_prints_a_line_for_each_type_and_writes_the_cards(self):
        cands = {
            'struct node': candidate(held_by=[('struct holder::(anonymous)', 'head')]),
            'struct list_int': candidate(file='src/gen.c', line=2, verdict='refuted', analysed=7, proved=4),
            'struct sys': candidate(file='/usr/include/sys.h', verdict='unknown', analysed=0, proved=0),
        }
        cards = self.root / 'out' / 'types'
        out = self.run_tool(report(cands, unsound=True), '--cards', str(cards))
        self.assertEqual(out.returncode, 0, out.stderr)
        lines = out.stdout.splitlines()
        self.assertEqual([line.split()[0] for line in lines[:3]], ['not-proved', 'proved', 'outside'])
        self.assertNotIn(sll_cards.NO_TYPES, lines)
        self.assertEqual(sorted(p.name for p in cards.iterdir()), ['struct_list_int.md', 'struct_node.md'])
        node = (cards / 'struct_node.md').read_text()
        for fact in ('# struct node\n', '`inputs/c_src/include/list.h:1`\n', 'Link field: `next`',
                     'Known holders: `struct holder.head`', 'Shape: PROVED.', '2 of 2 functions',
                     'with optimistic assumptions'):
            self.assertIn(fact, node)
        macro = (cards / 'struct_list_int.md').read_text()
        for fact in ('`inputs/c_src/src/gen.c:2` (a macro makes the type at this line)',
                     'Known holders: none listed', 'Shape: NOT PROVED.', '4 of 7 functions',
                     '\n- Views: YES / NO'):
            self.assertIn(fact, macro)

    def test_a_card_that_exists_stays(self):
        cards = self.root / 'cards'
        cards.mkdir()
        (cards / 'struct_node.md').write_text('filled by the list scout\n')
        cands = {'struct node': candidate(), 'struct list_int': candidate(file='src/gen.c', line=2)}
        out = self.run_tool(report(cands), '--cards', str(cards))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual((cards / 'struct_node.md').read_text(), 'filled by the list scout\n')
        self.assertIn(f"Kept the card that exists: {cards / 'struct_node.md'}", out.stdout.splitlines())
        self.assertTrue((cards / 'struct_list_int.md').is_file())

    def test_a_broken_type_gets_a_card(self):
        cards = self.root / 'cards'
        cand = candidate(verdict='refuted', analysed=3, proved=1, violations=2)
        out = self.run_tool(report({'struct node': cand}), '--cards', str(cards))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn('Shape: BROKEN.', (cards / 'struct_node.md').read_text())

    def test_no_type_in_the_source_writes_no_card(self):
        cards = self.root / 'cards'
        for cands in ({}, {'struct sys': candidate(file='/usr/include/sys.h')}):
            out = self.run_tool(report(cands), '--cards', str(cards))
            self.assertEqual(out.returncode, 0, out.stderr)
            self.assertEqual(out.stdout.splitlines()[-1], sll_cards.NO_TYPES)
            self.assertFalse(cards.exists())

    def test_unsupported_schema_version(self):
        out = self.run_tool({**report({}), 'schema_version': 2})
        self.assertEqual(out.returncode, 1)
        self.assertIn('schema_version 2', out.stderr)
        self.assertEqual(out.stdout, '')

    def test_malformed_reports(self):
        no_verdict = candidate()
        del no_verdict['verdict']
        bad_count = candidate()
        bad_count['functions']['analysed'] = 'two'
        no_soundness = report({'struct node': candidate()})
        del no_soundness['soundness']
        for text, message in [
            ('{"schema_version": 1', 'is not JSON'),
            ('[]', 'is not a JSON object'),
            ({'schema_version': 1}, 'the field candidates is missing'),
            (report({'struct node': no_verdict}), "candidate 'struct node': the field verdict is missing"),
            (report({'struct node': bad_count}), 'the field functions.analysed is not an integer'),
            (no_soundness, 'the field soundness is missing'),
        ]:
            out = self.run_tool(text)
            self.assertEqual(out.returncode, 1, text)
            self.assertIn(message, out.stderr)
            self.assertNotIn('Traceback', out.stderr)

    def test_missing_report_or_source(self):
        missing = subprocess.run([sys.executable, str(TOOL), str(self.root / 'none.json'), str(self.c_src)],
                                 capture_output=True, text=True)
        self.assertEqual(missing.returncode, 1)
        self.assertIn('cannot read', missing.stderr)
        no_source = subprocess.run([sys.executable, str(TOOL), str(self.root / 'none.json'), str(self.root / 'x')],
                                   capture_output=True, text=True)
        self.assertEqual(no_source.returncode, 1)
        self.assertIn('is not a directory', no_source.stderr)


if __name__ == '__main__':
    unittest.main()
