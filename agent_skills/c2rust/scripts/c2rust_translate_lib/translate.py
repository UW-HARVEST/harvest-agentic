"""One translation: the check of the options, the C build, the c2rust call and the adaptation, in
the scratch directory of the tool. The options and the C source decide the result: a later
translation with the same options and the same C source gives the same files."""
import re
import shutil
from dataclasses import dataclass
from pathlib import Path

from . import ToolError, adapt, c_build, compare, transpile
from .workspace import C_SRC, WORK_DIR

# The forms of a clang argument that the tool accepts: a macro definition without `/`, the
# removal of one, the language standard, and a -f, -W or -m option without a path and without a
# list of arguments for another program. No accepted form names a file to read: a macro with a
# path could do so through an `#include` of a macro in the C source.
CLANG_ARGUMENT = re.compile(r'-D[^/]+|-U\w+|-std=[\w+:.-]+|-[fWm][\w+.=-]*', re.S)
# The options of `c2rust transpile` that the tool accepts. None names a file or leaves code out.
TRANSPILE_FLAGS = ('--preserve-unused-functions', '--no-simplify-structures', '--no-incremental-relooper',
                   '--ignore-c-loop-info', '--ignore-c-multiple-info', '--disable-refactoring',
                   '--reduce-type-annotations', '--fail-on-error', '--fail-on-multiple')
TRANSPILE_VALUES = {'--translate-const-macros': ('none', 'conservative', 'experimental'),
                    '--translate-fn-macros': ('none', 'conservative', 'experimental'),
                    '--invalid-code': ('panic', 'compile_error')}
UNIT_NAME = re.compile(r'[\w.+-]+')


@dataclass
class Translation:
    cmake_command: list
    c2rust_command: list
    exit_status: int
    log: str
    crate: str
    binaries: list
    link_libraries: list
    messages: dict             # the lines of each message class of c2rust (see transpile.classify)
    package: Path | None       # the adapted package, or None when c2rust wrote no package
    adaptation: dict | None    # the number of changes of each adaptation step
    c_edits: dict | None       # the changes of the C source against inputs/c_src: counts and files
    c_edits_file: Path | None  # these changes as a patch, when there are changes


def translate(options, c2rust, toolchain, original=C_SRC):
    """Translates the C source of `options` (the `options` of a record entry) with the program
    `c2rust`, in the scratch directory of the tool, which it makes again. `toolchain` is the Rust
    toolchain of the rustfmt that formats the output (see `rust_toolchain`). `original` is the C
    source of the run: a C source at another place is a copy with edits."""
    original = Path(original).resolve()
    check_options(options, original)
    c_src = Path(options['c_src']).resolve()
    if not (c_src / 'CMakeLists.txt').is_file():
        raise ToolError(f'{options["c_src"]} has no CMakeLists.txt')
    if WORK_DIR.is_symlink():
        WORK_DIR.unlink()
    shutil.rmtree(WORK_DIR, ignore_errors=True)
    work = WORK_DIR.resolve()
    work.mkdir(parents=True)
    edits, edits_file = c_edits(c_src, original, work / 'c_edits.patch')

    build = c_build.configure_and_build(c_src, work / 'build')
    binaries = [] if options['library_only'] else options['binary'] or build.executable_units
    if len(binaries) > 1 and not options['binary']:
        raise ToolError('More than one translation unit has the `main` of an executable: ' + ', '.join(binaries)
                        + '. Name the unit of the executable with --binary UNIT, or give --library-only.')
    crate = crate_name(build.project, c_src)
    output = work / crate   # c2rust names the package after its output directory
    clang_args = ([transpile.LIBC_DEFINE] if options['libc_define'] else []) + options['clang_args']
    line = transpile.command(c2rust, build.compile_commands, output, binaries, options['transpile_args'], clang_args)
    status, log = transpile.transpile(line, toolchain)
    (work / 'transpile.log').write_text(log)

    package = adaptation = None
    if status == 0 and (output / 'Cargo.toml').is_file():
        adaptation = adapt.adapt_package(output, build.link_libraries)
        package = output
    messages = transpile.classify(log, options['libc_define'])
    return Translation(build.command, line, status, log, crate, binaries, build.link_libraries, messages, package,
                       adaptation, edits, edits_file)


def check_options(options, original):
    """Refuses options that do not have the form that the tool records, an option that the tool
    does not know, and an option that brings a file into the translation that is not in the C
    source of the call."""
    lists = ('binary', 'transpile_args', 'clang_args')
    if (set(options) != {'c_src', 'library_only', 'libc_define', *lists} or not isinstance(options['c_src'], str)
            or not all(isinstance(options[name], list) and all(isinstance(item, str) for item in options[name])
                       for name in lists)):
        raise ToolError('the options of the call do not have the form that the tool records')
    for arg in options['clang_args']:
        if not CLANG_ARGUMENT.fullmatch(arg):
            raise ToolError(f'clang argument "{arg}": the tool accepts -DNAME and -DNAME=VALUE without `/`, -UNAME, '
                            '-std=NAME, and -f, -W and -m options without a path. To add or change a file, or to '
                            'define a macro with `/`, edit a copy of the C source and translate the copy (--c-src).')
    for arg in options['transpile_args']:
        name, _, value = arg.partition('=')
        if arg not in TRANSPILE_FLAGS and value not in TRANSPILE_VALUES.get(name, ()):
            raise ToolError(f'c2rust option "{arg}": the tool accepts ' + ', '.join(TRANSPILE_FLAGS)
                            + ''.join(f', {name}=<{"|".join(values)}>' for name, values in TRANSPILE_VALUES.items()))
    for unit in options['binary']:
        if not UNIT_NAME.fullmatch(unit):
            raise ToolError(f'--binary {unit}: give the file name of the translation unit without directory and suffix')
    c_src = Path(options['c_src'])
    if c_src.resolve() != original:
        links = [str(path) for path in c_src.rglob('*') if path.is_symlink()] if c_src.is_dir() else []
        if c_src.is_symlink() or links:
            raise ToolError(f'The copy of the C source must not have a symbolic link: {", ".join(links[:5]) or c_src}')


def c_edits(c_src, original, patch_file):
    """The changes of the C source `c_src` against `original`, the C source of the run: the counts
    with the changed files, and `patch_file`, which gets the changes as a patch. (None, None) when
    `c_src` is `original`. No patch file when nothing differs."""
    if c_src == original:
        return None, None
    changes, text = compare.difference(compare.files(original), compare.files(c_src))
    if text:
        patch_file.write_text(text, errors='surrogateescape')
    return compare.totals(changes), patch_file if text else None


def crate_name(project, c_src):
    """A crate name from the CMake project name that is not an identifier of the C source: an item
    of the translated code with the name of the crate would stop the build of an executable."""
    name = re.sub(r'\W', '_', project).lower() + '_c2rust'
    if name[0].isdigit():
        name = f'c_{name}'
    texts = [f.read_bytes() for f in c_src.rglob('*') if f.is_file()]
    while any(re.search(rb'\b%b\b' % name.encode(), text) for text in texts):
        name += '_'
    return name
