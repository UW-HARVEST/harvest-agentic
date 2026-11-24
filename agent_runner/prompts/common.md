# Rules for every agent

Reader: every agent of this run, main agent and sub-agent. Read this file before you start your
task.

Read these files first:

- `inputs/prompts/run.md`
- `inputs/prompts/runtime.md`

## Directories of this run

All paths in the prompt files are relative to the root of your checkout: the working directory of
the main agent, or the git worktree of a sub-agent.

- `inputs/` holds the inputs of the run: the C source `inputs/c_src/`, the prompt files
  `inputs/prompts/`, skills and test suites. It is read-only. Never write, delete, move or
  change the permissions of anything in it.
- `shared/` is for large files that several agents must use but that must not
  go into git, for example a C build that a tool made once for all sub-agents.
  Git does not control who writes in it. The main agent decides, for each
  sub-agent, which files in `shared/` it may write and which it may read, and
  tells this in the sub-agent prompt. `shared/` is deleted when the run ends.
- `scratch/` is your private temporary space: C builds, test programs, large
  dumps. Git ignores it. It is deleted when the run ends.
- Put reports and anything that a later agent needs in the working directory
  (git keeps them), not in `shared/` or `scratch/`.
- `inputs` and `shared` are symbolic links that git tracks, so each git
  worktree has them. Do not delete, change or commit changes to them.
- Build the C source out of tree: `cmake -S inputs/c_src -B scratch/<build name>`.
- Read and write only in your checkout, `inputs/`, `shared/` and `scratch/`,
  and the files that your tools use (compilers, system headers, the Rust
  toolchain, the Cargo registry). Do not look elsewhere on the host for test
  suites, graders, results or other translations.

## Command environment

- Your commands normally run under a file allowlist. Under the allowlist, they can read the system
  files, the compilers and the Rust toolchain. They can write only the directories of this run and
  the Cargo home. A command that reads or writes another place fails with "Permission denied". Do
  not try again in another place outside the run.
- Do not write `/tmp`. The allowlist does not include it. Put temporary files in `scratch/`.
  `TMPDIR` is a directory of this run, so `mktemp` and the temporary files of compilers work.
- Under the allowlist, `HOME` is an empty directory of this run. It is not the home directory of a
  user. The Rust toolchain and the Cargo home are available to `cargo` and `rustc` all the same.
- Cargo downloads the crates that a `Cargo.toml` names. You do not have to find crate files on the
  host. The prompt files of your stage can limit which crates you use.
- All agent commands on this machine share one memory limit. When the output of a command has a
  line that starts with `[harvest] killed`, the commands used up this memory and the system
  stopped this one. Use less memory, or run fewer memory-heavy commands at the same time. A line
  that starts with `[harvest] note` tells that the command was slow because the memory was full.

## Rust toolchain

- `inputs/prompts/run.md` gives the required Rust toolchain. Use this exact toolchain for every
  build and every test: run Cargo as `RUSTUP_TOOLCHAIN=<required version> cargo ...`, or make sure
  that `rustc --version` reports the required version first.
- If a different Rust version is active, stop and report a toolchain problem. Do not treat test
  failures as translation bugs then.

## Long commands

- Give every command a time limit: the timeout of your shell tool (10 minutes, 600000 ms, when the
  tool takes one), or `timeout 600 <command>`. A shell tool usually stops a command after 2
  minutes when you give no timeout.
- Run a command that needs more than 10 minutes in the background, with its output in a log file
  in `scratch/`. Then wait for it in steps:

  ```
  nohup sh -c '<command> > scratch/run.log 2>&1; echo "EXIT $?" >> scratch/run.log' > /dev/null 2>&1 &
  timeout 500 sh -c 'until grep -q "^EXIT" scratch/run.log; do sleep 15; done'; tail -5 scratch/run.log
  ```

  Repeat the second line until the log has the `EXIT` line. Without a 10-minute tool timeout, use
  `timeout 100` in place of `timeout 500`.
- Do not get stuck on one command. When a command does not end in its time limit, do not run it
  again unchanged. Find the cause first.

## Reading and writing files

- Read a whole file only when it is short. For a long file, use `grep`, `head`,
  `sed -n 'A,Bp'` and `wc -l`, and read the part that you need.
- Write a long file in parts, with one write for each part. Each part must stay well below the
  output limit that `inputs/prompts/run.md` gives. When a response is cut off, continue from the
  last complete item. Do not start the file again.

## Responses

- Keep each response and each thinking block short. Plan one function or one step at a time, and
  call a tool at least every 2000 tokens.
