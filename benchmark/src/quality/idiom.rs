//! Idiomaticity: rustc warnings and clippy lints, DARPA TRACTOR's automated
//! idiomaticity metric, and clippy's cognitive complexity per function.
//!
//! DARPA runs clippy once per lint group (correctness, suspicious,
//! complexity, perf, style) with `-A clippy::all` and the group switched on,
//! and counts rustc warnings from the first run. One run with all five
//! groups at warn level yields the same counts per lint (every clippy lint is
//! in exactly one group); correctness stays at warn rather than DARPA's deny
//! so that a hit in the library cannot stop cargo before the binaries.
//!
//! As in DARPA's measurement, lint levels are given on the command line, so
//! the crate's own `#[allow(…)]` attributes still apply.
//!
//! Lint-to-group assignment and the set of rustc lints come from
//! `clippy-driver -W help` of the toolchain in use, not from a table kept
//! here.

use super::scratch::{tool_command, Package, ScratchArea};
use super::MeasureError;
use cargo_metadata::diagnostic::DiagnosticLevel;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// The clippy groups DARPA counts.
const DARPA_GROUPS: &[&str] = &["correctness", "suspicious", "complexity", "perf", "style"];

/// Lint counts of one clippy run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LintCounts {
    pub rustc_warnings: usize,
    pub rustc_by_lint: BTreeMap<String, usize>,
    pub clippy_lints: usize,
    pub clippy_by_group: BTreeMap<String, usize>,
    pub clippy_by_lint: BTreeMap<String, usize>,
}

/// Cognitive complexity of every function, as clippy computes it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ComplexityStats {
    pub functions: usize,
    pub mean: f64,
    pub max: usize,
    /// Complexity value → number of functions.
    pub histogram: BTreeMap<usize, usize>,
}

/// Idiomaticity summary of one package.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IdiomReport {
    pub lints: LintCounts,
    pub cognitive_complexity: ComplexityStats,
}

/// Measures `pkg`, which it compiles but does not modify.
pub fn measure(scratch: &ScratchArea, pkg: &Package) -> Result<IdiomReport, MeasureError> {
    let tables = LintTables::load(pkg)?;
    let mut args = vec!["-A".to_owned(), "clippy::all".to_owned()];
    for group in DARPA_GROUPS {
        args.extend(["-W".to_owned(), format!("clippy::{group}")]);
    }
    Ok(IdiomReport {
        lints: count_lints(pkg, &tables, clippy(pkg, &args, None)?)?,
        cognitive_complexity: cognitive_complexity(pkg, scratch)?,
    })
}

/// Runs `cargo clippy` over the package's default targets.
fn clippy(
    pkg: &Package,
    clippy_args: &[String],
    conf_dir: Option<&Path>,
) -> Result<super::scratch::CargoRun, MeasureError> {
    let mut args = vec!["clippy", "--message-format=json", "--"];
    args.extend(clippy_args.iter().map(String::as_str));
    let mut cmd = pkg.cargo(&args);
    if let Some(dir) = conf_dir {
        cmd.env("CLIPPY_CONF_DIR", dir);
    }
    pkg.run(cmd)
}

fn count_lints(
    pkg: &Package,
    tables: &LintTables,
    run: super::scratch::CargoRun,
) -> Result<LintCounts, MeasureError> {
    let mut counts = LintCounts::default();
    let mut errors = Vec::new();
    for m in &run.messages {
        let d = &m.message;
        let Some(code) = d.code.as_ref().map(|c| c.code.as_str()) else {
            continue;
        };
        match classify(code, d.level == DiagnosticLevel::Error, tables) {
            Finding::Clippy { lint, group } => {
                counts.clippy_lints += 1;
                *counts.clippy_by_lint.entry(lint.to_owned()).or_default() += 1;
                *counts.clippy_by_group.entry(group.to_owned()).or_default() += 1;
            }
            Finding::Rustc => {
                counts.rustc_warnings += 1;
                *counts.rustc_by_lint.entry(code.to_owned()).or_default() += 1;
            }
            Finding::CompileError => {
                errors.push(d.rendered.clone().unwrap_or_else(|| d.message.clone()));
            }
        }
    }
    if !errors.is_empty() {
        return Err(MeasureError::DoesNotCompile(errors));
    }
    if run.messages.is_empty() && !run.success {
        return Err(MeasureError::NotRun(format!(
            "cargo clippy failed in {}:\n{}",
            pkg.root.display(),
            run.stderr
        )));
    }
    Ok(counts)
}

/// What a coded diagnostic of a clippy run counts as.
#[derive(Debug, PartialEq, Eq)]
enum Finding<'a> {
    Clippy {
        lint: &'a str,
        group: &'a str,
    },
    /// A rustc lint, keyed by its code as DARPA does. Some lints report an
    /// error code rather than their name (`unsafe_op_in_unsafe_fn` reports
    /// E0133), so any coded diagnostic that is not a hard error counts.
    Rustc,
    /// A hard error: the crate does not compile.
    CompileError,
}

fn classify<'a>(code: &'a str, is_error: bool, tables: &'a LintTables) -> Finding<'a> {
    if let Some(lint) = code.strip_prefix("clippy::") {
        let group = tables
            .clippy_group
            .get(lint)
            .map_or("other", String::as_str);
        Finding::Clippy { lint, group }
    } else if is_error && !tables.rustc_lints.contains(code) {
        Finding::CompileError
    } else {
        Finding::Rustc
    }
}

/// Clippy reports every function's cognitive complexity when its threshold
/// is 0; the configuration is the only way to set it.
fn cognitive_complexity(
    pkg: &Package,
    scratch: &ScratchArea,
) -> Result<ComplexityStats, MeasureError> {
    let conf_dir = scratch.path().join("clippy_conf");
    std::fs::create_dir_all(&conf_dir).map_err(MeasureError::not_run("create clippy config"))?;
    std::fs::write(
        conf_dir.join("clippy.toml"),
        "cognitive-complexity-threshold = 0\n",
    )
    .map_err(MeasureError::not_run("write clippy config"))?;
    let args = ["-A", "clippy::all", "-W", "clippy::cognitive_complexity"].map(str::to_owned);
    let run = clippy(pkg, &args, Some(&conf_dir))?;
    let values: Vec<usize> = run
        .messages
        .iter()
        .filter(|m| {
            m.message
                .code
                .as_ref()
                .is_some_and(|c| c.code == "clippy::cognitive_complexity")
        })
        .filter_map(|m| complexity_value(&m.message.message))
        .collect();
    let mut stats = ComplexityStats {
        functions: values.len(),
        max: values.iter().copied().max().unwrap_or(0),
        ..Default::default()
    };
    if !values.is_empty() {
        stats.mean = values.iter().sum::<usize>() as f64 / values.len() as f64;
    }
    for v in values {
        *stats.histogram.entry(v).or_default() += 1;
    }
    Ok(stats)
}

/// "the function has a cognitive complexity of (12/0)" → 12.
fn complexity_value(message: &str) -> Option<usize> {
    let open = message.rfind('(')?;
    let slash = message[open..].find('/')?;
    message[open + 1..open + slash].parse().ok()
}

/// What the toolchain reports about its lints.
struct LintTables {
    /// Clippy lint (underscored, without `clippy::`) → its group.
    clippy_group: HashMap<String, String>,
    /// Every rustc lint (underscored).
    rustc_lints: HashSet<String>,
}

impl LintTables {
    fn load(pkg: &Package) -> Result<Self, MeasureError> {
        let out = tool_command("clippy-driver", &pkg.root)
            .args(["-W", "help"])
            .output()
            .map_err(MeasureError::not_run("run clippy-driver"))?;
        if !out.status.success() {
            return Err(MeasureError::NotRun(format!(
                "clippy-driver -W help failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(Self::parse(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Parses `-W help`: rows `name  level  description` for lints and
    /// `group  member, member, …` for groups; names use `-`.
    fn parse(help: &str) -> Self {
        let mut tables = Self {
            clippy_group: HashMap::new(),
            rustc_lints: HashSet::new(),
        };
        for line in help.lines() {
            let mut words = line.split_whitespace();
            let (Some(name), Some(second)) = (words.next(), words.next()) else {
                continue;
            };
            let name = name.replace('-', "_");
            if let Some(group) = name.strip_prefix("clippy::") {
                if second.starts_with("clippy::") && group != "all" {
                    let members = std::iter::once(second).chain(words);
                    for member in members {
                        let member = member.trim_end_matches(',').replace('-', "_");
                        if let Some(lint) = member.strip_prefix("clippy::") {
                            tables
                                .clippy_group
                                .entry(lint.to_owned())
                                .or_insert_with(|| group.to_owned());
                        }
                    }
                }
                continue;
            }
            if !matches!(second, "allow" | "warn" | "deny" | "forbid") || name == "name" {
                continue;
            }
            tables.rustc_lints.insert(name);
        }
        tables
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELP: &str = "
Available lint options:

Lint checks provided by rustc:

                              name  default  meaning
                              ----  -------  -------
                      absolute-paths-not-starting-with-crate  allow    fully qualified paths
                         dead-code  warn     detect unused, unexported items
                  unused-variables  warn     detect variables which are not used in any way
                    unconditional-panic  deny     operation will cause a panic at runtime

Lint groups provided by rustc:

                              name  sub-lints
                              ----  ---------
                            unused  dead-code, unused-variables

Lint checks provided by plugins loaded by this crate:

                              name  default  meaning
                  clippy::needless-return  warn  using a return statement like `return expr;`

Lint groups provided by plugins loaded by this crate:

                              name  sub-lints
                       clippy::all  clippy::needless-return, clippy::approx-constant
                clippy::complexity  clippy::bool-comparison
               clippy::correctness  clippy::approx-constant
                     clippy::style  clippy::needless-return, clippy::let-and-return
";

    #[test]
    fn lint_help_is_parsed_into_groups_and_lints() {
        let t = LintTables::parse(HELP);
        assert_eq!(t.clippy_group["needless_return"], "style");
        assert_eq!(t.clippy_group["approx_constant"], "correctness");
        assert_eq!(t.clippy_group["let_and_return"], "style");
        assert!(t.rustc_lints.contains("dead_code"));
        assert!(t.rustc_lints.contains("unconditional_panic"));
        assert!(t
            .rustc_lints
            .contains("absolute_paths_not_starting_with_crate"));
        assert!(!t.rustc_lints.contains("unused"), "groups are not lints");
        assert!(!t.rustc_lints.contains("clippy::needless_return"));
    }

    #[test]
    fn coded_diagnostics_are_classified_like_darpa() {
        let t = LintTables::parse(HELP);
        assert_eq!(
            classify("clippy::needless_return", false, &t),
            Finding::Clippy {
                lint: "needless_return",
                group: "style"
            }
        );
        assert_eq!(
            classify("clippy::approx_constant", true, &t),
            Finding::Clippy {
                lint: "approx_constant",
                group: "correctness"
            }
        );
        assert_eq!(classify("dead_code", false, &t), Finding::Rustc);
        // A lint reported under an error code, at warning level.
        assert_eq!(classify("E0133", false, &t), Finding::Rustc);
        // A lint the crate denies is still a lint.
        assert_eq!(classify("unconditional_panic", true, &t), Finding::Rustc);
        assert_eq!(classify("E0425", true, &t), Finding::CompileError);
    }

    #[test]
    fn complexity_value_is_read_from_the_message() {
        assert_eq!(
            complexity_value("the function has a cognitive complexity of (12/0)"),
            Some(12)
        );
        assert_eq!(complexity_value("unrelated"), None);
    }
}
