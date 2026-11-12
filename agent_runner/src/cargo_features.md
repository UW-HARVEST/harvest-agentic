### Cargo features
- Each build option of the C project becomes Cargo features. A string or
  integer option `NAME` with value `v` is the feature `NAME_v`. A boolean
  option `NAME` is the one feature `NAME`. The case does not change: `DEBUG`
  is not `debug`. When `inputs/c_src/configuration.json` exists, the build
  options and their values are its `configurable_variables`, and only those:
  the Rust code follows the CMake default of every other option.
- Do not type feature names. The skill `inputs/agent_skills/cargo_features/`
  prints them (`names`) and checks `Cargo.toml` (`check`). Read its
  `SKILL.md`. Give the printed names to each sub-agent that writes
  `cfg(feature = "...")` code.
- The `default` features select the CMake default of each option. The
  harness can replace these option features in `default`. Every other
  configuration is built with
  `cargo build --release --no-default-features --features <list>`, one
  feature for each option that the configuration sets. Test a configuration
  with the same command. The translation must compile and be correct in every
  configuration: every value of every option.
- The warning ``unexpected `cfg` condition value`` (lint `unexpected_cfgs`)
  is an error: a `cfg(feature = "...")` names a feature that `Cargo.toml`
  does not declare.
- When the crate has a `Cargo.toml`, run `check` before you end your task.
