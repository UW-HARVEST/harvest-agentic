#!/usr/bin/env python3
"""Tests of spec_contracts.py. For maintainers:
python3 agent_skills/spec_contracts/tests/test_spec_contracts.py"""
import io
import shutil
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'scripts'))
import spec_contracts  # noqa: E402

# The annotated C file. foo_pack has a comment between its parameter list and its first clause, a
# clause on two lines with a comment in it, and a loop contract in its body. foo_unpack has a
# macro line above a signature on three lines, a requires clause with one operand of proof setup,
# and the same construct in an ensures clause. helper has a contract and no header declaration.
SPEC = r'''#include "p.h"
#define FOO_GHOST 4  /* only the contracts use it */
#define FOO_ROOM(n) \
    ((n) + FOO_GHOST)

static int helper(int n)
__CPROVER_requires(n > 0)
{ return n; }

int foo_pack(const char* src, char* dst, int srcSize, int dstCapacity)
/* dst holds
 * dstCapacity bytes */
__CPROVER_requires(0 <= srcSize && srcSize <= FOO_MAX && dstCapacity >= 0)
__CPROVER_requires((src = __CPROVER_allocate(srcSize, 0), 1))
__CPROVER_assigns(__CPROVER_object_whole(dst))
__CPROVER_ensures(srcSize == 0 ==> (__CPROVER_return_value == 0  /* empty input */
                  || (FOO_MODE && __CPROVER_return_value == FOO_GHOST)))
__CPROVER_ensures(__CPROVER_forall { int i; (0 <= i && i < srcSize) ==> dst[i] == src[i] })
__CPROVER_ensures(__CPROVER_return_value <= FOO_ROOM(srcSize))  /* the bound */
{
    for (int i = 0; i < srcSize; i++)
    __CPROVER_assigns(i, __CPROVER_object_whole(dst))
    { dst[i] = src[i]; }
    return helper(srcSize);
}

FOO_NOINLINE
int
foo_unpack(const char* src,
           char* dst, int (*visit)(int), int sizes[2], ...)
__CPROVER_requires(src != NULL || dst == NULL && sizes[0] == 0)
__CPROVER_ensures(__CPROVER_return_value != ')')
__CPROVER_requires(dst == NULL || (sizes[0] > 0 && __CPROVER_is_fresh(dst, sizes[0])))
__CPROVER_ensures(__CPROVER_is_fresh(dst, 1))
{
    return '(';
}

#if FOO_MODE
int foo_mode_only(void)
__CPROVER_assigns()
{ return 0; }
#endif
'''

C_SOURCE = r'''#include "p.h"
static int helper(int n) { return n; }
int foo_pack(const char* src, char* dst, int srcSize, int dstCapacity)
{
    return helper(srcSize);
}
FOO_NOINLINE
int
foo_unpack(const char* src,
           char* dst, int (*visit)(int), int sizes[2], ...)
{
    return '(';
}
#if defined(FOO_EXTRA) \
    || 1
#if FOO_MODE  /* 0 in the build */
int foo_mode_only(void) { return 0; }
#endif
#endif
'''

# The header declares foo_pack, foo_unpack (on three lines) and foo_mode_only. The macro, the
# inline definition and the call in its body are no declarations.
HEADER = r'''/* Section: buffers
 * src and dst can be one buffer. */
#define FOO_MAX 100
#define FOO_NOINLINE
#define FOO_API
#define FOO_OLD(text)
#define foo_macro(n) foo_pack(0, 0, n, 0);

/*! foo_pack() :
 *  Copies srcSize bytes. */
FOO_API int foo_pack(const char* src, char* dst, int srcSize, int dstCapacity);

// foo_unpack reverses foo_pack.
// It has two comment lines.

FOO_OLD("use foo_pack(x); instead") FOO_API int
foo_unpack(const char* src,
           char* dst, int (*visit)(int), int sizes[2], ...);
int foo_mode_only(void);  /* a remark at the end of a line */
static inline int foo_inline(int n) { return foo_hidden(n); }
'''

CMAKE = 'add_library(p SHARED src/p.c)\ntarget_compile_definitions(p PRIVATE FOO_MODE=0 FOO_EXTRA)\n'

# Suite.Prop and Suite.Fixed are tests. The macro and the comment define no test.
TESTS = '''#define MY_PROPERTY(name) \\
  FUZZ_TEST(Macro, name)
// TEST(Comment, Old) {}
void Helper(int n) { foo_pack(0, 0, n, 0); Deep(n); }
void Deep(int n) { foo_mode_only(); Helper(n); }
void Prop(int n) { /* foo_unpack(n) is not called here */ Sym("foo_unpack"); Helper(n); }
FUZZ_TEST(Suite, Prop)
    .WithDomains(fuzztest::OneOf(fuzztest::InRange(0, 9),
                                 fuzztest::Just(Size(")"))))
    .WithSeeds({{0}});
void Plain(int n) { Apply(foo_unpack, n); Shared(n); }
FUZZ_TEST(Suite, Plain);
TEST(Suite, Fixed) { foo_pack(0, 0, 1, 0); }
'''
# Another file: a different function with the name Helper, and a function that only this file has.
COMMON = 'void Helper(int n) { foo_unpack(0, 0, 0, 0); }\nvoid Shared(int n) { foo_mode_only(); }\n'

NEEDS_GCC = unittest.skipUnless(shutil.which('gcc'), 'gcc is not installed')


def row(function='foo_pack', kind='must cover', source='[1.1]', test='Suite.Prop', note=''):
    """One DOMAINS.md row for a class of srcSize, without spaces at the pipes."""
    return f'|{function}|srcSize|0 .. FOO_MAX|{kind}|the size|{source}|{test}|{note}|\n'


# One row for each kind, and one or more rows for each function that the tests name.
COMPLETE = [row(source='[1.1], [4]; inputs/c_src/src/p.c:3', test='Suite.Prop, `Suite.Fixed`'),
            row(kind='C rejects', test='', note='not covered: no time'),
            row(kind='never pass', test=''),
            row(function='`foo_unpack`', kind='unsure', source='[1]', test=''),
            row(function='foo_mode_only', source='inputs/c_src/include/p.h:19', test='Suite.Plain')]


def table(*rows):
    return spec_contracts.HEADER + '\n|---|---|---|---|---|---|---|---|\n' + ''.join(rows)


class InDirectory(unittest.TestCase):
    """The annotated C file, a C source and a test directory in a temporary directory."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        for name, text in {'spec/p.c': SPEC, 'spec/p.h': HEADER, 'spec/stubs/stub.c': SPEC,
                           'c_src/src/p.c': C_SOURCE, 'c_src/include/p.h': HEADER, 'c_src/CMakeLists.txt': CMAKE,
                           'verify_env/t.cc': TESTS, 'verify_env/common.h': COMMON,
                           'verify_env/build-test/copy.cc': 'TEST(Build, Copy) {}\n'}.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.spec = str(self.root / 'spec')
        self.c_src = str(self.root / 'c_src')
        self.tests = self.root / 'verify_env'
        self.domains = self.root / 'DOMAINS.md'

    def tearDown(self):
        self.dir.cleanup()

    def run_tool(self, *argv):
        """(exit status, stdout, stderr) of one command, with the files of the directory."""
        out, err = io.StringIO(), io.StringIO()
        paths = ['--spec', self.spec, '--tests', str(self.tests), '--c-src', self.c_src]
        with redirect_stdout(out), redirect_stderr(err):
            status = spec_contracts.main([*argv, *paths])
        return status, out.getvalue(), err.getvalue()

    def contracts(self):
        return spec_contracts.contracts(self.spec)

    def show(self, function):
        return spec_contracts.show(function, self.contracts()[function], self.c_src)

    def texts(self):
        """The code of the test sources."""
        return spec_contracts.code_texts(spec_contracts.read_sources(str(self.tests)))


class Contracts(InDirectory):
    def test_a_contract_is_the_clauses_between_the_parameter_list_and_the_body(self):
        contract = self.contracts()['foo_pack']
        self.assertEqual((contract['file'], contract['line']), (f'{self.spec}/p.c', 10))
        self.assertEqual(contract['parameters'], ['src', 'dst', 'srcSize', 'dstCapacity'])
        self.assertEqual([(kind, line) for kind, line, _ in contract['clauses']],
                         [('requires', 13), ('requires', 14), ('assigns', 15), ('ensures', 16), ('ensures', 18),
                          ('ensures', 19)])

    def test_a_signature_on_several_lines_below_a_macro_line(self):
        contract = self.contracts()['foo_unpack']
        self.assertEqual(contract['line'], 29)
        self.assertEqual(contract['parameters'], ['src', 'dst', 'visit', 'sizes'])

    def test_a_clause_is_one_line_with_its_literals_and_without_its_comments(self):
        self.assertEqual(self.contracts()['foo_pack']['clauses'][3][2],
                         'srcSize == 0 ==> (__CPROVER_return_value == 0 || (FOO_MODE && '
                         '__CPROVER_return_value == FOO_GHOST))')
        self.assertEqual(self.contracts()['foo_unpack']['clauses'][1][2], "__CPROVER_return_value != ')'")

    def test_the_comments_of_a_contract_have_their_lines(self):
        self.assertEqual(self.contracts()['foo_pack']['comments'],
                         [(11, 'dst holds dstCapacity bytes'), (16, 'empty input'), (19, 'the bound')])

    def test_a_loop_contract_is_no_function_contract(self):
        self.assertEqual(list(self.contracts()), ['helper', 'foo_pack', 'foo_unpack', 'foo_mode_only'])

    def test_only_the_c_files_directly_in_the_directory_are_read(self):
        self.assertEqual(spec_contracts.spec_files(self.spec), [f'{self.spec}/p.c'])

    def test_a_directory_without_a_c_file_is_an_error_line(self):
        self.spec = str(self.root / 'spec/stubs/none')
        self.assertEqual(self.run_tool('list'), (2, '', f'Error: no C file with contracts in {self.spec}\n'))

    def test_a_missing_c_source_is_an_error_line(self):
        self.c_src = str(self.root / 'no_such')
        self.assertEqual(self.run_tool('list'), (2, '', f'Error: {self.c_src} is not a directory\n'))

    def test_a_requires_clause_has_one_part_for_each_operand_of_and(self):
        parts = spec_contracts.clause_parts
        self.assertEqual(parts('requires', 'a >= 0 && (b || c) && d[i && j]'), ['a >= 0', '(b || c)', 'd[i && j]'])
        self.assertEqual(parts('ensures', 'a && b'), ['a && b'])

    def test_a_requires_clause_with_a_weaker_operator_is_one_part(self):
        for text in ['a || b && c', 'a && b ==> c', 'a && b ? c : d', 'a = b, c && d']:
            self.assertEqual(spec_contracts.clause_parts('requires', text), [text])

    def test_parts_have_the_number_of_their_clause(self):
        ids = [part_id for part_id, *_ in spec_contracts.numbered_parts(self.contracts()['foo_pack'])]
        self.assertEqual(ids, ['1.1', '1.2', '1.3', '2', '3', '4', '5', '6'])


class Headers(InDirectory):
    def test_a_declaration_is_a_name_after_a_type_with_a_semicolon_after_its_parameters(self):
        (_, _, _, names), = spec_contracts.headers(self.c_src)
        self.assertEqual(list(names), ['foo_pack', 'foo_unpack', 'foo_mode_only'])

    def test_list_gives_the_functions_with_a_contract_that_a_header_declares(self):
        status, out, _ = self.run_tool('list')
        self.assertEqual(status, 0)
        self.assertEqual(out.splitlines(), [f'foo_pack\tclauses: 6\t{self.spec}/p.c:10',
                                            f'foo_unpack\tclauses: 4\t{self.spec}/p.c:29',
                                            f'foo_mode_only\tclauses: 1\t{self.spec}/p.c:40'])

    def test_adjacent_line_comments_are_one_block(self):
        blocks = spec_contracts.comment_blocks(HEADER)
        self.assertEqual([(first, last) for first, last, _ in blocks], [(1, 2), (9, 10), (13, 14)])

    def test_notes_gives_the_comment_blocks_that_are_not_above_a_declaration(self):
        header = f'{self.c_src}/include/p.h'
        self.assertEqual(self.run_tool('notes'), (0, f'{header}:1-2: Section: buffers\n'
                                                     f'{header}:13-14: foo_unpack reverses foo_pack.\n', ''))


class Show(InDirectory):
    def test_the_places_of_a_function(self):
        text = self.show('foo_unpack')
        self.assertTrue(text.startswith('# foo_unpack(src, dst, visit, sizes)\n\n'
                                        f'contract: {self.spec}/p.c:29\n'
                                        f'C definition: {self.c_src}/src/p.c:9\n'))
        self.assertIn(f'\nheader declaration: {self.c_src}/include/p.h:17\n', text)

    def test_the_header_comments_that_name_the_function(self):
        header = f'{self.c_src}/include/p.h'
        self.assertIn('\nheader comments that name foo_pack:\n'
                      f'  {header}:9-10 (directly above the declaration): foo_pack() :\n'
                      f'  {header}:13-14: foo_unpack reverses foo_pack.\n\n', self.show('foo_pack'))
        self.assertIn('\nheader comments that name foo_mode_only:\n  none\n', self.show('foo_mode_only'))

    def test_a_clause_with_a_condition_has_its_condition_and_its_result(self):
        text = self.show('foo_pack')
        self.assertIn('        if: srcSize == 0\n        then: (__CPROVER_return_value == 0 || ', text)
        # The ==> in the braces of a forall is not the operator of the clause.
        self.assertIn('  [5] ensures (spec line 18): __CPROVER_forall { int i; (0 <= i && i < srcSize) ==> '
                      'dst[i] == src[i] }\n  [6] ', text)

    def test_the_operands_of_a_condition(self):
        self.assertEqual(spec_contracts.operands('a ==> (b || (c && d(e || f)))'), ['a', 'b', 'c', 'd(e || f)'])
        for text in ['a ? b && c : d', 'a = b, c || d']:
            self.assertEqual(spec_contracts.operands(text), [text])

    def test_proof_setup_in_one_operand_has_a_mark_with_the_operand(self):
        self.assertIn('  [3] requires (spec line 33): dst == NULL || (sizes[0] > 0 && __CPROVER_is_fresh(dst, '
                      'sizes[0]))\n'
                      '        SETUP in part: __CPROVER_is_fresh(dst, sizes[0])\n'
                      '              proof setup: the proof gives the pointer a new object of this size.',
                      self.show('foo_unpack'))

    def test_a_setup_construct_in_an_ensures_clause_is_a_result(self):
        text = self.show('foo_unpack')
        self.assertIn('  [4] ensures (spec line 34): __CPROVER_is_fresh(dst, 1)\n\n', text)
        self.assertTrue(text.endswith('\n  __CPROVER_is_fresh(p, n) in an ensures clause: when the function '
                                      'returns, p points to a new object of n bytes.'))

    def test_proof_setup_has_a_mark(self):
        text = self.show('foo_pack')
        self.assertIn('  [2] requires (spec line 14): (src = __CPROVER_allocate(srcSize, 0), 1)\n'
                      '        SETUP proof setup: the contract makes this buffer itself.', text)
        self.assertIn('\n        SETUP the clause assigns a value to src. The contract makes this value itself.\n',
                      text)
        self.assertEqual(text.count('SETUP'), 2)

    def test_a_macro_of_the_contracts_only_is_expanded_or_named(self):
        text = self.show('foo_pack')
        self.assertIn('(FOO_MODE && __CPROVER_return_value == (4))', text)
        # The body of FOO_ROOM names FOO_GHOST: the expansion goes on until no such macro is left.
        self.assertIn('  [6] ensures (spec line 19): __CPROVER_return_value <= FOO_ROOM(srcSize)\n'
                      '        note: FOO_ROOM(n) is a macro of the contracts only: ((n) + (4))\n', text)

    def test_each_parameter_has_the_ids_of_its_clauses(self):
        self.assertIn('\nclauses by parameter:\n  src: 2, 5\n  dst: 3, 5\n  srcSize: 1.1, 1.2, 2, 4, 5, 6\n'
                      '  dstCapacity: 1.3\n', self.show('foo_pack'))
        self.assertIn('\n  visit: no clause\n', self.show('foo_unpack'))

    def test_the_constructs_of_the_clauses_have_their_meaning(self):
        self.assertIn('\nconstructs in the clauses:\n  __CPROVER_return_value is the result of the function.\n',
                      self.show('foo_unpack'))

    @NEEDS_GCC
    def test_a_macro_of_the_c_source_has_its_value_in_the_build(self):
        self.assertIn('\nmacros in the clauses, value in this build (gcc -dM -E -DFOO_MODE=0 -DFOO_EXTRA):\n'
                      '  FOO_MAX = 100\n  FOO_MODE = 0\n\n', self.show('foo_pack'))

    @NEEDS_GCC
    def test_a_definition_is_in_the_build_or_not(self):
        self.assertIn(f'\nC definition: {self.c_src}/src/p.c:3\nin the build: yes\n', self.show('foo_pack'))
        self.assertIn(f'\nC definition: {self.c_src}/src/p.c:17\nin the build: no\n', self.show('foo_mode_only'))

    def test_without_the_compiler_the_answer_is_unknown_with_the_conditions(self):
        source = f'{self.c_src}/src/p.c'
        with mock.patch.object(spec_contracts, 'COMPILER', 'no-such-compiler'):
            text = self.show('foo_mode_only')
            self.assertIn('\n  FOO_MODE = unknown. The compiler did not read the file.\n', self.show('foo_pack'))
            self.assertIn('\nin the build: unknown. The compiler did not read the file. The definition is in no '
                          '#if condition.\n', self.show('foo_pack'))
        self.assertIn('\nin the build: unknown. The compiler did not read the file. The definition is in these '
                      f'conditions:\n  {source}:14: #if defined(FOO_EXTRA) || 1\n  {source}:16: #if FOO_MODE\n',
                      text)

    def test_the_lines_of_a_branch_that_is_not_selected_are_not_built(self):
        output = '# 1 "src/p.c"\nint a;\n\n# 1 "include/p.h" 1\nint b;\nint c;\n# 9 "src/p.c" 2\n\nint d;\n'
        self.assertEqual(spec_contracts.built_lines(output, 'src/p.c'), {1, 10})

    def test_a_function_that_list_does_not_print_is_an_error_line(self):
        # helper has a contract, but no header declares it.
        for command in ['show', 'properties']:
            self.assertEqual(self.run_tool(command, 'helper'),
                             (2, '', 'Error: no contract for the function helper. The list command prints the '
                                     'functions with a contract.\n'))


class TestSources(InDirectory):
    def test_macros_comments_and_build_directories_define_no_test(self):
        self.assertEqual(spec_contracts.tests_in(self.texts()), {'Suite.Prop', 'Suite.Plain', 'Suite.Fixed'})

    def test_a_missing_test_directory_is_an_error_line(self):
        self.tests = self.root / 'no_such'
        status, _, err = self.run_tool('properties')
        self.assertEqual((status, err), (2, f'Error: {self.tests} is not a directory\n'))


class Properties(InDirectory):
    def property_lines(self, *wanted):
        functions = ['foo_pack', 'foo_unpack', 'foo_mode_only']
        return [line.split('\t') for line in spec_contracts.properties(self.texts(), functions, wanted)]

    def test_a_property_has_its_place_and_its_domains(self):
        prop, plain = self.property_lines()
        self.assertEqual(prop[:3], [
            'Suite.Prop', f'{self.tests}/t.cc:7',
            'domains: fuzztest::OneOf(fuzztest::InRange(0, 9), fuzztest::Just(Size(")")))'])
        self.assertEqual(plain[:3], ['Suite.Plain', f'{self.tests}/t.cc:12', 'domains: (default domains)'])

    def test_seeds_are_in_the_chain_or_on_a_domain(self):
        code = ('FUZZ_TEST(S, Chain).WithDomains(fuzztest::Arbitrary<int>()).WithSeeds({{0}});\n'
                'FUZZ_TEST(S, Domain).WithDomains(fuzztest::VectorOf(fuzztest::Arbitrary<int>()).WithSeeds(Seeds));\n'
                'FUZZ_TEST(S, None).WithDomains(fuzztest::Arbitrary<int>());\n')
        seeds = [line.split('\t')[3] for line in spec_contracts.properties({'t.cc': code}, [])]
        self.assertEqual(seeds, ['seeds: yes', 'seeds: yes', 'seeds: no'])

    def test_a_property_reaches_the_functions_that_it_and_its_helpers_name(self):
        # Prop calls Helper of its file, which calls foo_pack and Deep. Deep calls foo_mode_only, and
        # Helper again. Helper of the other file calls foo_unpack, and Prop has foo_unpack in a string
        # literal: Prop does not reach foo_unpack. Plain gives foo_unpack to a function as an argument,
        # and calls Shared of the other file, which calls foo_mode_only.
        prop, plain = self.property_lines()
        self.assertEqual((prop[4], plain[4]),
                         ('reaches: foo_pack, foo_mode_only', 'reaches: foo_unpack, foo_mode_only'))

    def test_function_arguments_select_the_properties(self):
        status, out, _ = self.run_tool('properties', 'foo_unpack')
        self.assertEqual(status, 0)
        self.assertEqual([line.split('\t')[0] for line in out.splitlines()], ['Suite.Plain'])

    def test_no_property_is_one_sentence(self):
        (self.tests / 't.cc').write_text('void P(int n) { foo_pack(0, 0, n, 0); }\nFUZZ_TEST(Suite, P);\n')
        self.assertEqual(self.run_tool('properties', 'foo_unpack'), (0, 'No FUZZ_TEST reaches foo_unpack.\n', ''))
        (self.tests / 't.cc').write_text('TEST(Suite, Fixed) {}\n')
        self.assertEqual(self.run_tool('properties', 'foo_unpack'), (0, 'No FUZZ_TEST found.\n', ''))


class Check(InDirectory):
    def problems(self, *rows):
        """The problems of a table with the rows, without the line numbers."""
        self.domains.write_text(table(*rows))
        status, out, _ = self.run_tool('check', str(self.domains))
        self.assertEqual(status, 1)
        return [line.split(': ', 1)[1] if line.startswith('line ') else line for line in out.splitlines()]

    def test_a_complete_table_has_no_problem(self):
        self.domains.write_text(table(*COMPLETE))
        self.assertEqual(self.run_tool('check', str(self.domains)), (0, 'No problems found.\n', ''))

    def test_a_second_header_row_is_no_class(self):
        self.domains.write_text(table(*COMPLETE[:2]) + table(*COMPLETE[2:]))
        self.assertEqual(self.run_tool('check', str(self.domains))[0], 0)

    def test_a_missing_or_different_header_row(self):
        problem = f'the first table row is not the header row: {spec_contracts.HEADER}\n'
        for text in ['No table.\n', ''.join(COMPLETE), table(*COMPLETE).replace('C result', 'expect')]:
            self.domains.write_text(text)
            self.assertEqual(self.run_tool('check', str(self.domains)), (1, problem, ''))

    def test_a_row_with_a_missing_cell(self):
        self.assertEqual(self.problems(*COMPLETE, '|foo_pack|srcSize|0|must cover|0|[4]|Suite.Prop|\n'),
                         ['the row has 7 cells, not 8'])

    def test_a_kind_that_is_no_kind(self):
        self.assertEqual(self.problems(*COMPLETE, row(kind='legal')),
                         ["the kind 'legal' is not one of: must cover, C rejects, never pass, unsure"])

    def test_a_function_without_a_contract(self):
        # helper has a contract, but no header declares it.
        self.assertEqual(self.problems(*COMPLETE, row(function='helper'), row(function='foo_inline')),
                         ['no contract for the function helper', 'no contract for the function foo_inline'])

    def test_a_source_item_is_a_clause_id_or_a_line_of_the_c_source(self):
        correct = '[1], [1.3]; `[6]`, inputs/c_src/src/p.c:19, inputs/c_src/include/p.h:9-10'
        wrong = ['[7]', '[1.4]', 'the header', 'src/p.c:3', 'inputs/c_src/src/p.c:20', 'inputs/c_src/src/p.c:5-3',
                 'inputs/c_src/src/q.c:1']
        problems = self.problems(*COMPLETE, row(source=correct), row(source=', '.join(wrong)))
        self.assertEqual(problems, [f"the source item '{item}' is no clause id of foo_pack (as [3] or [1.2]) and "
                                    'no line of a file of the C source (as inputs/c_src/<path>:<line>)'
                                    for item in wrong])

    def test_a_test_name_that_is_no_test(self):
        self.assertEqual(self.problems(*COMPLETE, row(test='Suite.Fixed, Suite.Other')),
                         ['no TEST or FUZZ_TEST has the name Suite.Other'])

    def test_a_never_pass_or_unsure_row_with_a_test(self):
        self.assertEqual(self.problems(*COMPLETE, row(kind='never pass'), row(kind='unsure', test='Suite.Fixed')),
                         ["a row of the kind 'never pass' names a test (Suite.Prop)",
                          "a row of the kind 'unsure' names a test (Suite.Fixed)"])

    def test_a_tested_kind_without_a_test_and_without_a_reason(self):
        problem = "no test, and the note does not start with 'not covered:'"
        self.assertEqual(self.problems(*COMPLETE, row(test='', note='later'), row(kind='C rejects', test='')),
                         [problem, problem])

    def test_a_function_that_the_tests_name_without_a_row(self):
        self.assertEqual(self.problems(*COMPLETE[:3]),
                         ['foo_unpack: the tests name this function, but it has no row',
                          'foo_mode_only: the tests name this function, but it has no row'])

    def test_a_function_in_the_body_of_a_macro_of_the_tests_needs_a_row(self):
        (self.tests / 'common.h').write_text('#define EACH_FUNCTION(X) \\\n  X(foo_pack) X(foo_unpack)\n')
        (self.tests / 't.cc').write_text('TEST(Suite, Prop) {}\n')
        self.assertEqual(self.problems(row()), ['foo_unpack: the tests name this function, but it has no row'])

    def test_a_row_without_a_source(self):
        self.assertEqual(self.problems(*COMPLETE, row(source=' ')), ['the source is empty'])

    def test_a_problem_has_the_line_of_its_row(self):
        self.domains.write_text(table(*COMPLETE, row(kind='legal')))
        out = self.run_tool('check', str(self.domains))[1]
        self.assertTrue(out.startswith('line 8: the kind '), out)

    def test_a_missing_table_is_an_error_line(self):
        status, _, err = self.run_tool('check', str(self.domains))
        self.assertEqual(status, 2)
        self.assertEqual(err, f'Error: cannot read {self.domains}: No such file or directory\n')


if __name__ == '__main__':
    unittest.main()
