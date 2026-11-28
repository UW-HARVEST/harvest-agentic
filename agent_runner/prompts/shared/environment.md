# The environment of this run

- `inputs/` holds what the framework gives you: the C source `inputs/c_src/`, the prompt files
  `inputs/prompts/`, and the other files of this run. It is read-only. If you build the C code,
  build it out of tree in `scratch/`.
- `scratch/` is for temporary files. Git ignores it. It is deleted when the run ends.
- `shared/` is for large files that git must not record. It is deleted when the run ends.
- Only the working directory is kept. It is a git repository. `inputs` and `shared` in it are
  symbolic links that git tracks: do not delete or change them.
- Your commands normally run under a file allowlist. They can read the system files, the
  compilers and the Rust toolchain. They can write only the directories of this run and the Cargo
  home. A command that reads or writes another place fails with "Permission denied".
- `/tmp` is not writable: `TMPDIR` is a directory of this run. `HOME` is an empty directory.
- Cargo downloads the crates that `Cargo.toml` names.
- The environment sets `RUSTUP_TOOLCHAIN` to the required Rust toolchain.
- All agent commands on this machine share one memory limit. A line `[harvest] killed` in the
  output of a command tells that the system stopped the command because the memory was full.
- A shell tool usually stops a command after 2 minutes when you give no timeout. It takes a
  timeout of at most 10 minutes.
- Stop a process only by its PID. Never use `pgrep -f`, `pkill -f` or `killall`: the pattern also
  matches your own shell and the processes of other agents on this machine.
