#!/usr/bin/env python3
"""Tests of qscan on the fixture crate (known answers), of the tree-sitter parser, and of the gtest
output parser.

Run: python3 tools/quality_agentic/skill/tests/run_tests.py
Needs cargo and clippy of the pinned toolchain. The first run makes the tree-sitter venv in the
repository's target/qscan_venv (QSCAN_VENV overrides it; this needs network). Writes only under a
temporary directory.
"""
import os
import shutil
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'scripts'))

from qscan_lib import pyenv  # noqa: E402

os.environ.setdefault('QSCAN_VENV', os.path.normpath(os.path.join(HERE, '../../../../target/qscan_venv')))
pyenv.ensure(HERE)

from qscan_lib import items, plan, report, scan, unsafe_ops  # noqa: E402
from regress_lib import gtest  # noqa: E402


def scan_source(src):
    with tempfile.TemporaryDirectory() as d:
        with open(os.path.join(d, 'a.rs'), 'w') as fh:
            fh.write(src)
        return items.scan_sources(d, ['a.rs'])[0]['a.rs']


class StripTest(unittest.TestCase):
    def test_strip_keeps_offsets_and_range_end(self):
        src = 'for i in 0..unsafe { *n } { let x = unsafe { *p }; } // \u00e9 unsafe { }\nassert!(unsafe { *q } > 0);'
        out, blocks = unsafe_ops.strip_unsafe_blocks(src)
        self.assertEqual(len(out), len(src))
        self.assertEqual(blocks, 3)
        self.assertIn("0..'z:    { *n }", out)
        self.assertIn('let x =        { *p }', out)
        self.assertIn('// \u00e9 unsafe { }', out)  # a comment is not code
        self.assertIn('assert!(       { *q } > 0)', out)  # a macro body is code


class ItemsTest(unittest.TestCase):
    def test_signature_facts(self):
        it = scan_source('''
#[unsafe(no_mangle)]
/// Doc.
pub unsafe extern "C" fn f(a: *mut u8, n: usize) -> *const u8 { a }
impl Foo { pub(crate) fn m(&self, p: &mut Bar) {} }
impl Tr for &mut Baz<T> { fn t(&self) {} }
macro_rules! g { ($n:ident) => { fn $n() {} } }
extern "C" { fn ext(x: i32); }
fn h<T: Into<u8>>(x: T) where T: Copy { }
''')
        fns = {f.qual: f for f in it.fns}
        self.assertEqual(sorted(fns), ['Baz::t', 'Foo::m', 'f', 'h'])
        f = fns['f']
        self.assertTrue(f.exported and f.is_unsafe)
        self.assertEqual((f.first_line, f.last_line), (2, 4))
        self.assertEqual(f.raw_ptr_params, ['a'])
        self.assertTrue(fns['Baz::t'].in_trait_impl and not fns['Foo::m'].in_trait_impl)
        self.assertEqual(it.extern_decls, ['ext'])

    def test_macro_bodies_and_pointer_uses(self):
        it = scan_source('''
type P = *mut u8;
fn f(p: P, q: *const u8, n: usize) {
    assert!(n as u32 > 0, "{}", n as u64);
    let a = *(q as *const u16).add(n);
    g(p, (q, n));
    unsafe { *p = 0 };
}
''')
        f = it.fns[0]
        self.assertEqual(f.raw_ptr_params, ['p', 'q'])
        self.assertEqual(f.facts['as_casts'], 3)
        self.assertEqual(f.facts['ptr_arith'], 1)
        self.assertEqual(f.uses['p'], dict(deref=1, arg=1, block=0))
        self.assertEqual(f.uses['q'], dict(deref=0, arg=1, block=1))  # arithmetic, then a dereference
        self.assertTrue(f.raw_in_unsafe)
        self.assertEqual(it.calls, {'g', 'add'})


class GtestParseTest(unittest.TestCase):
    def test_crash_is_the_unfinished_test(self):
        out = '\n'.join(['[ RUN      ] A.B', '[       OK ] A.B (0 ms)', '[ RUN      ] A.C',
                         '[  FAILED  ] A.C (1 ms)', '[ RUN      ] A.D', 'Segmentation fault'])
        self.assertEqual(gtest.parse(out), {'A.B': 'PASS', 'A.C': 'FAIL', 'A.D': 'CRASH'})


class FuzzPropertyTest(unittest.TestCase):
    def test_source_fallback_reads_any_text_file(self):
        with tempfile.TemporaryDirectory() as d:
            env = os.path.join(d, 'verify_env')
            os.makedirs(os.path.join(env, 'build-fuzz'))
            with open(os.path.join(env, 'tests_block.inc'), 'w') as fh:
                fh.write('FUZZ_TEST(BlockFuzz, Compress).WithDomains(Arbitrary<int>());\n')
            with open(os.path.join(env, 'build-fuzz', 'x.inc'), 'w') as fh:
                fh.write('FUZZ_TEST(Ignored, InBuildDir);\n')
            with open(os.path.join(env, 'blob.a'), 'wb') as fh:
                fh.write(bytes([0xff, 0xfe, 0x00]))
            self.assertEqual(gtest.fuzz_properties(d, os.path.join(env, 'missing-binary')),
                             ['BlockFuzz.Compress'])


class GatePathTest(unittest.TestCase):
    def test_baseline_paths_do_not_go_through_the_shared_link(self):
        from regress_lib import gate
        with tempfile.TemporaryDirectory() as d:
            real = os.path.join(d, 'case_shared')
            os.makedirs(real)
            crate = os.path.join(d, 'wt', 'base')
            os.makedirs(crate)
            os.symlink(real, os.path.join(crate, 'shared'))
            self.assertTrue(gate._paths(crate)['base'].startswith(os.path.realpath(real) + os.sep))

    def test_missing_baseline_library_stops_the_check(self):
        import json
        from regress_lib import gate
        with tempfile.TemporaryDirectory() as d:
            base = os.path.join(d, 'shared', 'regress', 'baseline')
            os.makedirs(base)
            with open(os.path.join(base, 'info.json'), 'w') as fh:
                json.dump(dict(lib=os.path.join(d, 'gone', 'lib.so'), unit=True, fuzz=False, props=[],
                               cargo_tests=False, exports=0, notes=[]), fh)
            with self.assertRaises(gate.GateError):
                gate.check(d, log=lambda m: None, skip_build=True)


class FixtureScanTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix='qscan-test-')
        crate = os.path.join(cls.tmp, 'crate')
        shutil.copytree(os.path.join(HERE, 'fixture'), crate)
        cls.data = scan.run(crate, os.path.join(cls.tmp, 'work'), log=lambda m: None)
        cls.rows = {r['fn']: r for r in cls.data['rows']}
        report.write(cls.data, os.path.join(cls.tmp, 'scan'))

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp)

    def test_unsafe_ops_per_function(self):
        t = self.data['totals']
        self.assertEqual(t['measure_errors'], [])
        expect = {'reset': 2, 'add': 0, 'peek': 1, 'q_reset': 2, 'noisy': 0, 'sum_to': 1,
                  'other_caller': 1, 'super_reset': 1, 'call_reset_twice': 1}
        self.assertEqual({k: self.rows[k]['ops'] for k in expect}, expect)
        self.assertEqual(t['unsafe_ops'], 9)
        self.assertEqual(self.rows['q_reset']['static_mut'], 1)
        self.assertEqual(t['unsafe_ops_exported'], 2)

    def test_pseudo_safe_and_aliases(self):
        pseudo = sorted(k for k, r in self.rows.items() if r['pseudo_safe'] == 'y')
        self.assertEqual(pseudo, ['call_reset_twice', 'peek', 'sum_to'])
        self.assertEqual(self.data['totals']['raw_pointer_aliases'], ['ctx_ptr'])
        self.assertEqual(self.rows['q_reset']['raw_params'], 1)

    def test_recipes(self):
        self.assertIn('R2', self.rows['add']['recipes'])
        self.assertEqual(self.rows['peek']['recipes'].split()[0], 'R12')
        self.assertIn('R3', self.rows['reset']['recipes'])  # in-body form: callers elsewhere keep working
        self.assertEqual(self.rows['reset']['r3_vars'], 'ctx(2)')
        self.assertIn('R7', self.rows['q_reset']['recipes'])
        self.assertIn('R1', self.rows['noisy']['recipes'])

    def test_roots_groups_by_pointee_type(self):
        from qscan_lib import roots
        loaded = report.load(os.path.join(self.tmp, 'scan'))
        text = roots.report(loaded)
        self.assertIn('| Ctx | 3 |', text)  # reset (2) and super_reset (1)
        self.assertIn('static_mut', text)
        ops = {(o['fn'], o['var']) for o in loaded['ops'] if o['kind'] == 'deref'}
        self.assertIn(('peek', 'p'), ops)

    def test_lints_ignore_allows(self):
        t = self.data['totals']
        self.assertIn('needless_return', t['lints_by_name'])
        self.assertEqual([a.split(' ')[1] for a in t['blanket_allows']], ['clippy::all'])
        self.assertEqual(t['static_mut_items'], 1)

    def test_views_and_compare(self):
        loaded = report.load(os.path.join(self.tmp, 'scan'))
        view = report.file_view(loaded, 'src/lib.rs', [])
        self.assertIn('PSEUDO-SAFE', view)
        text, problems = report.compare(loaded, loaded)
        self.assertEqual(problems, [])
        units = plan.units(loaded, budget_lines=10, min_score=1)
        self.assertTrue(all(len({f for f, _, _ in u['files']}) >= 1 for u in units))
        waves = plan.waves(units, parallel=5)
        for w in waves:
            files = [f for u in w for f, _, _ in u['files']]
            self.assertEqual(len(files), len(set(files)))


if __name__ == '__main__':
    unittest.main()
