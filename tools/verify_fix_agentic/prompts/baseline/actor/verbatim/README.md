# The validation prompt of ACTOR, unchanged

Source: <https://github.com/UW-HARVEST/ACTOR>, file `prompts/claude/verify.md`, tag
`IEEE-Privacy-and-Security` (commit `337e7a77eee40684f889fb0025316ce68c5b43c1`).

No run gets this file. A test of the crate checks its SHA-256. The actor arm uses the file of
the same name in the parent directory. `diff -r . ..` shows each difference:

- At the top: the list of the shared files of the study, and the sentence that these files win
  over the prompt. The grader follows the shared files.
- Paths: `CASE_DIR_PLACEHOLDER` is "the current directory (the root of the Rust crate)", the
  prefix `translated_rust/` is removed, and `c_src/` is `inputs/c_src/`. The framework of ACTOR
  ran the agent in a directory above the crate. Here the agent works in the crate, and the C
  source is a read-only input of the run.
- The C build is in `scratch/build`, because the C source is read-only.
- `CMAKE_BUILD_FLAGS` is removed. The framework of ACTOR put the CMake flags of the graded
  configuration there, and nothing for a case without flags. Here no arm is told the graded
  configuration.
