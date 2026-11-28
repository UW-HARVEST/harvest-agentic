"""The call of `c2rust transpile`, and the classes of the messages in its output."""
import re
import shutil

from . import ToolError, run

# With this macro the headers of the C library declare the string functions as plain functions.
# Without it they are C11 generic selections under _GNU_SOURCE, which c2rust cannot translate.
LIBC_DEFINE = '-D_LIBC'
# The C library answers the macro with this error in each translation unit. c2rust goes on.
LIBC_DEFINE_ERROR = 'Applications may not define the macro _LIBC'

ANSI_COLOR = re.compile(r'\x1b\[[0-9;]*m')
# The message classes of the record: name, and what a line of the class matches. A line counts
# for the first class that matches.
MESSAGE_CLASSES = (
    ('panics', re.compile(r'panicked at ')),
    ('functions_not_translated', re.compile(r'^error: Failed to translate ')),
    ('declarations_skipped', re.compile(r'^error: Skipping declaration ')),
    ('asm_clobbers_dropped', re.compile(r'^warning: .*dropping clobber')),
    ('generic_selections_unsupported', re.compile(r'unsupported generic selection')),
    ('duplicate_commands_skipped', re.compile(r'^warning: Skipping duplicate compilation cmd')),
    ('rustfmt_failures', re.compile(r'^warning: rustfmt failed')),
    ('clang_errors', re.compile(r'^\S+:\d+:\d+: (?:fatal )?error: ')),
    ('other_errors', re.compile(r'^error: ')),
    ('other_warnings', re.compile(r'^warning: ')),
)


def find():
    """The path of the c2rust program on PATH."""
    path = shutil.which('c2rust')
    if path is None:
        raise ToolError('c2rust is not installed: no `c2rust` on PATH')
    return path


def version(c2rust):
    status, output = run([c2rust, '--version'])
    if status != 0:
        raise ToolError(f'`{c2rust} --version` failed:\n{output}')
    return output.strip()


def rustfmt_version(toolchain):
    """The version of the rustfmt that formats the output of c2rust, or None when there is none."""
    try:
        status, output = run(['rustfmt', '--version'], toolchain)
    except FileNotFoundError:
        return None
    return output.strip() if status == 0 else None


def command(c2rust, compile_commands, output_dir, binaries, transpile_args, clang_args):
    line = [c2rust, 'transpile', str(compile_commands), '--emit-build-files', '--output-dir', str(output_dir)]
    for unit in binaries:
        line += ['--binary', unit]
    line += transpile_args
    if clang_args:
        line += ['--', *clang_args]
    return line


def transpile(line, toolchain):
    """Runs c2rust. Returns its exit status and its output without color codes."""
    status, output = run(line, toolchain)
    return status, ANSI_COLOR.sub('', output)


def classify(log, libc_define):
    """The lines of each message class in the c2rust output: class name -> lines, in order. With
    `libc_define`, the error that the macro of the tool causes is not a message."""
    classes = {name: [] for name, _ in MESSAGE_CLASSES}
    lines = log.splitlines()
    for number, line in enumerate(lines):
        if libc_define and LIBC_DEFINE_ERROR in line:
            continue
        for name, pattern in MESSAGE_CLASSES:
            if pattern.search(line):
                # The cause of a panic is on the line after it.
                cause = lines[number + 1:number + 2] if name == 'panics' else []
                classes[name].append(' '.join([line, *cause]))
                break
    return classes
