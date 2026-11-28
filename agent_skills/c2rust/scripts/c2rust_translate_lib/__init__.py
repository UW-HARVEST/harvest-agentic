"""The parts of c2rust_translate: the C build, the c2rust call, the adaptation of its output, the
comparison of file trees, and the record in the working directory. See ../../SKILL.md."""
import os
import subprocess

# The environment variables that the commands of the tool get. No other variable of the caller
# reaches CMake, the C compiler or c2rust, so the options of a call and the C source decide the
# translation.
KEPT_VARIABLES = ('PATH', 'HOME', 'TMPDIR', 'CARGO_HOME', 'RUSTUP_HOME')


class ToolError(Exception):
    """A problem that stops the tool. The message is for the agent."""


def run(command, toolchain=None):
    """Runs a command with the kept environment variables, and with `toolchain` as its
    RUSTUP_TOOLCHAIN. Returns its exit status and its output (stdout and stderr together): the
    command does not write to the output of the tool."""
    env = {name: os.environ[name] for name in KEPT_VARIABLES if name in os.environ}
    if toolchain:
        env['RUSTUP_TOOLCHAIN'] = toolchain
    done = subprocess.run(command, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, text=True, errors='replace')
    return done.returncode, done.stdout


def rust_toolchain():
    """The Rust toolchain whose rustfmt formats the output of c2rust: RUSTUP_TOOLCHAIN, or else
    the toolchain that rustup selects in the working directory. None without rustup. The tool
    gives it to c2rust, which starts rustfmt in its output directory: the toolchain file that
    c2rust writes there would select another toolchain."""
    if os.environ.get('RUSTUP_TOOLCHAIN'):
        return os.environ['RUSTUP_TOOLCHAIN']
    try:
        status, output = run(['rustup', 'show', 'active-toolchain'])
    except FileNotFoundError:
        return None
    return output.split()[0] if status == 0 and output.split() else None
