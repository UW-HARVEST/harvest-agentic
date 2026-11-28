# The translation prompts of ACTOR, unchanged

Source: <https://github.com/UW-HARVEST/ACTOR>, directory `prompts/claude/`, tag
`IEEE-Privacy-and-Security` (commit `337e7a77eee40684f889fb0025316ce68c5b43c1`).

No run gets these files. A test of the crate checks the SHA-256 of each one. The actor arm uses
the files of the same names in the parent directory. `diff -r . ..` shows each difference:

- At the top: the list of the shared files of the study, and the sentence that these files win
  over the prompt. The grader follows the shared files.
- `c_src/` is `inputs/c_src/`. The C source is a read-only input of the run.
- `translate-shared.md`: the words ", using the **exact same name in lowercase**" are removed.
  The shared file `output_spec.md` gives the names of the Cargo features.
- `translate-shared.md`, `translate-no-features-shared.md`: "produces BOTH a shared library AND a
  binary executable" is "a shared library, a binary executable, or both", and the next sentence
  says which target is for which. A HarvestBench program can have either or both.
- `translate-shared.md`, `translate-no-features-shared.md`: the item on the `openssl` crate is
  removed. No program of the benchmark links OpenSSL.
