"""The working directory of the run: the fixed paths, the files that are the result of the run,
the record of the tool calls, and the git commits of the tool."""
import json
import os
import subprocess
from pathlib import Path

from . import ToolError, compare

# All paths are relative to the working directory, which is the root of the package.
C_SRC = Path('inputs/c_src')
# The tool makes this directory again in each translation.
WORK_DIR = Path('scratch/c2rust')
RECORD_DIR = Path('c2rust_record')
CALLS_FILE = RECORD_DIR / 'calls.jsonl'
C_EDITS_FILE = RECORD_DIR / 'c_edits.patch'

# What the run removes from the working directory at its end: these top-level directories, the
# top-level links to the inputs, and each hidden entry except a git ignore file.
REMOVED_DIRECTORIES = ('target', 'scratch')
REMOVED_LINKS = ('inputs', 'shared')
KEPT_HIDDEN = '.gitignore'
TOOL_IDENTITY = ('-c', 'user.name=c2rust_translate', '-c', 'user.email=c2rust_translate@invalid')


def require_workspace():
    if not C_SRC.is_dir() or not Path('.git').exists():
        raise ToolError(f'Run the tool from the root of the working directory: it has {C_SRC} and is a git repository.')


def result_files():
    """The files of the working directory that are the result of the run: also the files that git
    does not track or ignores, and not what the run removes at its end."""
    def removed(relative):
        name = relative.name
        if name.startswith('.') and name != KEPT_HIDDEN:
            return True
        return len(relative.parts) == 1 and (name in REMOVED_DIRECTORIES
                                             or (name in REMOVED_LINKS and relative.is_symlink()))

    return compare.files('.', removed)


def read_calls():
    """The record of the tool calls, oldest first."""
    try:
        text = CALLS_FILE.read_text() if CALLS_FILE.is_file() else ''
        calls = [json.loads(line) for line in text.splitlines() if line.strip()]
        if not all(isinstance(call, dict) and isinstance(call.get('options'), dict) for call in calls):
            raise ValueError('an entry is not a call')
    except ValueError as error:
        raise ToolError(f'{CALLS_FILE} is not a record of the tool: {error}') from None
    return calls


def git(*args, ok=(0,)):
    """Runs git in the working directory and returns its output. An exit status outside `ok` is an
    error. The configuration of the user and of the system, hooks and replaced objects do not
    apply. The git commands of the tool are a help for the agent: no statement of the tool about
    its output depends on them."""
    env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1', GIT_NO_REPLACE_OBJECTS='1')
    done = subprocess.run(['git', '-c', f'core.hooksPath={os.devnull}', *args], env=env, capture_output=True,
                          text=True, errors='replace')
    if done.returncode not in ok:
        raise ToolError(f'git {" ".join(args)} failed:\n{done.stderr.strip()}')
    return done.stdout


def commit_subject(call):
    return f'c2rust_translate: call {call}'


def commit_of_call(call):
    """The newest commit in the history of HEAD with the subject of call number `call`, or None."""
    subject = commit_subject(call)
    for line in git('log', '--format=%H %s').splitlines():
        commit, _, text = line.partition(' ')
        if text == subject:
            return commit
    return None


def changes_since(commit, paths):
    """The changes of `paths` in the working directory after `commit`, as a patch. New files count."""
    git('add', '-A', '-f', '--', *(p for p in paths if Path(p).exists()))
    return git('diff', '--cached', commit, '--', *paths)


def commit_call(call, paths, message):
    """Commits the state of `paths` in the working directory, and nothing else, as the commit of a
    tool call. A path that is gone from the working directory is gone from the commit."""
    paths = sorted(map(str, paths))
    git('add', '-A', '-f', '--', *(p for p in paths if Path(p).exists()))
    indexed = git('ls-files', '--', *paths).splitlines()
    known = [p for p in paths if any(f == p or f.startswith(f'{p}/') for f in indexed)]
    git(*TOOL_IDENTITY, 'commit', '-q', '--no-verify', '-m', f'{commit_subject(call)}\n\n{message}', '--', *known)
    return git('rev-parse', 'HEAD').strip()
