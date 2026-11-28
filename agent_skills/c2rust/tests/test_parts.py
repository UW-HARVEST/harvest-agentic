#!/usr/bin/env python3
"""Tests of the parts of c2rust_translate that need no C build and no c2rust: the message classes,
the facts of a CMake build, the check of the options, the crate name, the comparison of files.
For maintainers: python3 tests/test_parts.py
"""
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the skill directory (it is embedded and copied to agents)
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'scripts'))
from c2rust_translate_lib import ToolError, c_build, compare, transpile, translate  # noqa: E402

LOG = '''Transpiling a.c
/usr/include/x86_64-linux-gnu/gnu/stubs-64.h:7:3: error: Applications may not define the macro _LIBC
/src/a.c:3:10: fatal error: 'missing.h' file not found
error: Failed to translate hash_finalize: Unimplemented builtin __builtin_assume
error: Failed to translate hash_finalize: Unimplemented builtin __builtin_assume
error: Skipping declaration Some(Typedef { name: "__m128i_u" }) due to error: Unsupported type Vector
warning: Attempting to clobber reserved register (ebx), dropping clobber! This likely means trouble.
/src/b.c:9:11: warning: c2rust: Encountered unsupported generic selection expression
thread 'main' panicked at src/translator/atomics.rs:334:18:
not implemented: atomic not implemented: __atomic_test_and_set
warning: Skipping duplicate compilation cmd for /src/a.c
warning: rustfmt failed, code may not be well-formatted
warning: Missing child of some node
error: something else
'''


def options(**changed):
    return {'c_src': 'inputs/c_src', 'binary': [], 'library_only': False, 'libc_define': True, 'transpile_args': [],
            'clang_args': []} | changed


class TempDir(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.root = Path(self.dir.name).resolve()

    def write(self, name, text):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text if isinstance(text, bytes) else text.encode())
        return path


class MessageClasses(unittest.TestCase):
    def test_each_line_counts_for_one_class(self):
        classes = transpile.classify(LOG, libc_define=True)
        self.assertEqual({name: len(lines) for name, lines in classes.items()}, {
            'panics': 1, 'functions_not_translated': 2, 'declarations_skipped': 1, 'asm_clobbers_dropped': 1,
            'generic_selections_unsupported': 1, 'duplicate_commands_skipped': 1, 'rustfmt_failures': 1,
            'clang_errors': 1, 'other_errors': 1, 'other_warnings': 1})
        self.assertTrue(classes['panics'][0].endswith('atomic not implemented: __atomic_test_and_set'))

    def test_the_error_of_the_libc_macro_counts_only_without_the_macro(self):
        self.assertEqual(len(transpile.classify(LOG, libc_define=False)['clang_errors']), 2)


class CMakeFacts(TempDir):
    def test_system_libraries_in_link_order_without_project_libraries(self):
        build = Path('/run/scratch/build')
        fragments = ['-lm', '/usr/lib/x86_64-linux-gnu/libz.so', 'libown.a', f'{build}/sub/libown2.so',
                     '-Wl,-rpath,/run/scratch/build', '/opt/x/lib/libfoo-2.so.1.2', '-lm', '-ldl']
        def linking(*fragments):
            return {'commandFragments': [{'fragment': f, 'role': 'libraries'} for f in fragments]
                    + [{'fragment': '-lflag', 'role': 'flags'}]}

        targets = [{'type': 'SHARED_LIBRARY', 'link': linking(*fragments)},
                   {'type': 'STATIC_LIBRARY', 'link': linking('-lnot_linked')},
                   {'type': 'EXECUTABLE', 'link': linking('-lrt', '-lm')}]
        self.assertEqual(c_build.system_libraries(targets, build), ['m', 'z', 'foo-2', 'dl', 'rt'])

    def test_executable_units_are_the_compiled_sources_with_main(self):
        self.write('src/cli.c', 'int main(void) { return 0; }\n')
        self.write('tools/app.c', 'int\nmain (int argc, char **argv) { return run(); }\n')
        self.write('tools/run.c', 'int run(void) { return domain(); }\n')
        compiled = lambda *paths: [{'path': path, 'compileGroupIndex': 0} for path in paths]  # noqa: E731
        targets = [
            {'type': 'SHARED_LIBRARY', 'sources': compiled('src/lib.c')},
            # the objects of another target are sources that the executable does not compile
            {'type': 'EXECUTABLE', 'sources': compiled('src/cli.c') + [{'path': 'build/lib.c.o'}]},
            {'type': 'EXECUTABLE', 'sources': compiled('tools/run.c', 'tools/app.c')},
        ]
        self.assertEqual(c_build.executable_units(targets, self.root), ['cli', 'app'])


class Options(TempDir):
    def test_the_accepted_forms(self):
        translate.check_options(options(
            clang_args=['-DA', '-DF(x)=((void)0)', '-DS="a b"', '-UA', '-std=gnu11', '-fno-strict-aliasing',
                        '-fvisibility=hidden', '-Wno-error=implicit-function-declaration', '-msse4.2', '-march=x86-64-v2'],
            transpile_args=['--preserve-unused-functions', '--translate-const-macros=none', '--invalid-code=panic'],
            binary=['main']), self.root)

    def test_an_option_that_can_bring_in_a_file_or_leave_code_out_is_refused(self):
        refused = [
            {'clang_args': [arg]} for arg in (
                '-include', '/x.h', '-include/x.h', '--include=/x.h', '-I/x', '-isystem', '-iwithprefix', '-isysroot',
                '--sysroot=/x', '-F/x', '--config', '@/x', '-ivfsoverlay', '-Xclang', '-Wp,-include,x.h',
                '-fmodule-map-file=/x/m', '-fplugin=../x.so', '-D', '-DHEADER="/x/h.h"', '-DR=a/b', '--')
        ] + [
            {'transpile_args': [arg]} for arg in (
                '--', '--filter', '--filter=a', '-X', '--cross-check-config=x', '--output-dir=/x', '--binary=a',
                '--translate-const-macros', '--translate-const-macros=/x', '--fail-on-error=1')
        ] + [{'binary': ['../a']}, {'c_src': 7}, {'unknown': 1}]
        for change in refused:
            with self.assertRaises(ToolError, msg=change):
                translate.check_options(options(**change), self.root)

    def test_a_copy_of_the_c_source_with_a_symbolic_link_is_refused(self):
        self.write('copy/a.c', 'int a;\n')
        translate.check_options(options(c_src=str(self.root / 'copy')), self.root / 'original')
        (self.root / 'copy/b.h').symlink_to('/etc/hostname')
        with self.assertRaises(ToolError):
            translate.check_options(options(c_src=str(self.root / 'copy')), self.root / 'original')
        # The C source of the run itself is not a copy.
        translate.check_options(options(c_src=str(self.root / 'copy')), self.root / 'copy')


class CrateName(TempDir):
    def test_name_from_the_project_that_is_not_in_the_c_source(self):
        self.write('a.c', 'int sqlite3_c2rust;\nint other;\n')
        self.assertEqual(translate.crate_name('SQLite3', self.root), 'sqlite3_c2rust_')
        self.assertEqual(translate.crate_name('lib-x', self.root), 'lib_x_c2rust')
        self.assertEqual(translate.crate_name('7zip', self.root), 'c_7zip_c2rust')


class Comparison(TempDir):
    def test_changed_added_removed_binary_and_linked_files(self):
        for name, old, new in (('same.rs', 'a\n', 'a\n'), ('src/changed.rs', 'a\nb\nc\n', 'a\nB\nc\nd'),
                               ('removed.rs', 'x\ny\n', None), ('added.rs', None, 'z\n'),
                               ('binary', b'\0a', b'\0b'), ('same_binary', b'\0a', b'\0a'),
                               ('.hidden/x', None, 'h\n'), ('target/x', None, 't\n')):
            for side, text in (('old', old), ('new', new)):
                if text is not None:
                    self.write(f'{side}/{name}', text)
        (self.root / 'old/link').symlink_to('same.rs')
        (self.root / 'new/link').symlink_to('/etc/hostname')
        (self.root / 'new/dir_link').symlink_to('/etc')  # not followed
        skip = lambda relative: relative.parts[0] in ('.hidden', 'target')  # noqa: E731
        changes, text = compare.difference(compare.files(self.root / 'old'), compare.files(self.root / 'new', skip))
        self.assertEqual(changes, [('added.rs', 1, 0), ('binary', None, None), ('dir_link', 1, 0), ('link', 1, 1),
                                   ('removed.rs', 0, 2), ('src/changed.rs', 2, 1)])
        self.assertEqual(compare.totals(changes) | {'list': None},
                         {'files': 6, 'added': 5, 'removed': 4, 'files_not_compared_by_lines': 1, 'list': None})
        for part in ('--- /dev/null\n+++ b/added.rs\n@@ -0,0 +1 @@\n+z\n',
                     'Not compared by lines: a/binary: binary file and b/binary: binary file\n',
                     '-symbolic link to same.rs\n+symbolic link to /etc/hostname\n', '--- a/removed.rs\n+++ /dev/null\n',
                     '--- a/src/changed.rs\n+++ b/src/changed.rs\n@@ -1,3 +1,4 @@\n a\n-b\n+B\n c\n+d\n\\ No newline at end of file\n'):
            self.assertIn(part, text)

    def test_a_file_that_cannot_be_read_and_an_entry_that_is_not_a_file_are_changes(self):
        self.write('old/a.rs', 'a\n')
        unreadable = self.write('new/a.rs', 'hidden\n')
        unreadable.chmod(0)
        self.addCleanup(unreadable.chmod, 0o644)
        os.mkfifo(self.root / 'new/pipe')
        closed = self.root / 'new/closed'
        closed.mkdir()
        (closed / 'inside.rs').write_text('x\n')
        closed.chmod(0)
        self.addCleanup(closed.chmod, 0o755)
        changes, text = compare.difference(compare.files(self.root / 'old'), compare.files(self.root / 'new'))
        self.assertEqual(changes, [('a.rs', None, None), ('closed', None, None), ('pipe', None, None)])
        self.assertIn('Not compared by lines: b/a.rs: cannot be read\n', text)
        self.assertIn('Not compared by lines: b/closed: not a regular file\n', text)
        self.assertIn('Not compared by lines: b/pipe: not a regular file\n', text)


if __name__ == '__main__':
    unittest.main()
