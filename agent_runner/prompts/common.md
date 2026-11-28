# Rules for every agent

Read these files first:

- `inputs/prompts/environment.md`
- `inputs/prompts/anti_cheating.md`
- `inputs/prompts/run.md`
- `inputs/prompts/runtime.md`

## Directories of this run

All paths in the prompt files are relative to the root of your checkout: the working directory of
the main agent, or the git worktree of a sub-agent. `inputs/prompts/environment.md` describes the
directories. This section adds to it.

- `inputs/` also holds skills and test suites. Never write, delete, move or
  change the permissions of anything in it.
- `shared/` is for the large files that several agents must use, for example
  a C build that a tool made once for all sub-agents.
  Git does not control who writes in it. The main agent decides, for each
  sub-agent, which files in `shared/` it may write and which it may read, and
  tells this in the sub-agent prompt.
- `scratch/` is your private temporary space: C builds, test programs, large
  dumps.
- Put reports and anything that a later agent needs in the working directory
  (git keeps them), not in `shared/` or `scratch/`.
- Each git worktree has the links `inputs` and `shared`. Do not commit changes
  to them.
- Build the C source out of tree: `cmake -S inputs/c_src -B scratch/<build name>`.
- Read and write only in your checkout, `inputs/`, `shared/` and `scratch/`,
  and the files that your tools use (compilers, system headers, the Rust
  toolchain, the Cargo registry).

## Command environment

`inputs/prompts/environment.md` describes the file allowlist and the memory limit. This section
adds to it.

- After "Permission denied", do not try again in another place outside the run.
- Do not write `/tmp`. Put temporary files in `scratch/`. `mktemp` and the temporary files of
  compilers work, because of `TMPDIR`.
- Under the allowlist, `HOME` is not the home directory of a user. The Rust toolchain and the
  Cargo home are available to `cargo` and `rustc` all the same.
- You do not have to find crate files on the host. The prompt files of your stage can limit
  which crates you use.
- After a line that starts with `[harvest] killed`, use less memory, or run fewer memory-heavy
  commands at the same time. A line that starts with `[harvest] note` tells that the command was
  slow because the memory was full.

## Rust toolchain

- `inputs/prompts/run.md` gives the required Rust toolchain. Use this exact toolchain for every
  build and every test: run Cargo as `RUSTUP_TOOLCHAIN=<required version> cargo ...`, or make sure
  that `rustc --version` reports the required version first.
- If a different Rust version is active, stop and report a toolchain problem. Do not treat test
  failures as translation bugs then.

## Long commands

- Give every command a time limit: the timeout of your shell tool (10 minutes, 600000 ms, when the
  tool takes one), or `timeout 600 <command>`.
- Run a command that needs more than 10 minutes in the background, with its output in a log file
  in `scratch/`. Then wait for it in steps:

  ```
  nohup sh -c '<command> > scratch/run.log 2>&1; echo "EXIT $?" >> scratch/run.log' > /dev/null 2>&1 &
  timeout 500 sh -c 'until grep -q "^EXIT" scratch/run.log; do sleep 15; done'; tail -5 scratch/run.log
  ```

  Repeat the second line until the log has the `EXIT` line. Without a 10-minute tool timeout, use
  `timeout 100` in place of `timeout 500`.
- Find and wait for a process by its PID too: when you start it in the background, write
  `$!` to a file in `scratch/` (the next command runs in a new shell). Wait with
  `while kill -0 <pid>` or for a marker file as above, and stop it with `kill <pid>`. A pattern
  of `pgrep -f`, `pkill -f` or `killall` can stop the work of other agents.
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
