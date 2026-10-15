//! Cross-checks the unsafe-operation count against its ground truth: DARPA
//! TRACTOR's patched rustc (`unsafe_ops_checker.patch` from the TRACTOR
//! Test-Corpus, `tools/rust_eval/unsafe_ops/`, applied to the rustc version
//! the measurement uses), run exactly as DARPA's `run_unsafeops()` does.
//!
//! The patched compiler is not part of this repository, so these tests are
//! ignored by default. Run them with
//!
//! ```text
//! HARVEST_UNSAFE_OPS_RUSTC=/path/to/patched/bin/rustc \
//!   cargo test -p harvest-benchmark --test unsafe_ops_oracle -- --ignored --nocapture
//! ```
//!
//! and, to check real crates as well, `HARVEST_UNSAFE_OPS_CRATES=<file>`
//! naming one crate directory per line (`#` starts a comment).
//!
//! Every operation is matched by file, start and end position, and kind. The
//! method must report nothing the patched rustc does not, and may miss only
//! operations in compiler-generated unsafe blocks.

mod common;

use harvest_benchmark::quality::measure;
use harvest_benchmark::quality::safety::{OpKind, UnsafeOp};
use harvest_benchmark::quality::Outcome;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Callees that only appear in compiler-generated unsafe blocks.
const COMPILER_GENERATED_CALLEES: &[&str] = &[
    "core::fmt::",
    "core::intrinsics::unreachable",
    "core::pin::",
    "core::future::get_context",
];

type Key = (String, usize, usize, usize, usize, OpKind);

/// One operation as the patched rustc prints it.
struct OracleOp {
    key: Key,
    callee: String,
}

/// What the patched rustc reports for one crate.
struct Oracle {
    /// After DARPA's filter (spans under `library/` or `.cargo` dropped).
    darpa_count: usize,
    /// The subset whose span is a file of the crate itself.
    own: Vec<OracleOp>,
    compiled: bool,
}

fn patched_rustc() -> PathBuf {
    let path = std::env::var_os("HARVEST_UNSAFE_OPS_RUSTC").expect(
        "set HARVEST_UNSAFE_OPS_RUSTC to the rustc built with DARPA's unsafe_ops_checker.patch",
    );
    PathBuf::from(path)
}

/// DARPA's `run_unsafeops()`: clean, `-C codegen-units=1`,
/// `TRACTOR_UNSAFE_OPS=1`, `cargo -vv check`, regex the lines out of stdout.
fn run_oracle(crate_dir: &Path, rustc: &Path) -> Oracle {
    let target = crate_dir.join("target/oracle");
    let out = Command::new("cargo")
        .args(["-vv", "check"])
        .current_dir(crate_dir)
        .env("RUSTC", rustc)
        .env("TRACTOR_UNSAFE_OPS", "1")
        .env("RUSTFLAGS", "-C codegen-units=1")
        .env("RUSTC_BOOTSTRAP", "1")
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .expect("run cargo with the patched rustc");
    let line_re = regex::Regex::new(r"TRACTOR_UNSAFE_OPS (\{.*\})").unwrap();
    let span_re = regex::Regex::new(r"^(.*):(\d+):(\d+): (\d+):(\d+)").unwrap();
    let kind_re = regex::Regex::new(r"^([A-Za-z]+)").unwrap();
    let callee_re = regex::Regex::new(r"~ ([^)]+)\)").unwrap();
    let hash_re = regex::Regex::new(r"\[[0-9a-f]{4}\]").unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut darpa_count = 0;
    let mut own = Vec::new();
    for caps in line_re.captures_iter(&stdout) {
        let e: serde_json::Value = serde_json::from_str(&caps[1]).unwrap();
        let span = e["span"].as_str().unwrap();
        if span.starts_with("library/") || span.contains(".cargo") {
            continue;
        }
        darpa_count += 1;
        let s = span_re.captures(span).expect("span format");
        if s[1].starts_with('/') {
            continue; // a path dependency's file
        }
        let kind = e["kind"].as_str().unwrap();
        let num = |i: usize| s[i].parse::<usize>().unwrap();
        own.push(OracleOp {
            key: (
                s[1].to_owned(),
                num(2),
                num(3),
                num(4),
                num(5),
                // rustc's `UnsafeOpKind` variants the measurement does
                // not distinguish are `Other` on both sides.
                serde_json::from_value(kind_re.captures(kind).unwrap()[1].into())
                    .unwrap_or(OpKind::Other),
            ),
            callee: callee_re
                .captures(kind)
                .map(|c| hash_re.replace_all(&c[1], "").into_owned())
                .unwrap_or_default(),
        });
    }
    Oracle {
        darpa_count,
        own,
        compiled: out.status.success(),
    }
}

fn key(op: &UnsafeOp) -> Key {
    (
        op.file.clone(),
        op.line,
        op.column,
        op.end_line,
        op.end_column,
        op.kind,
    )
}

/// Op-by-op comparison of one crate.
struct Comparison {
    name: String,
    oracle_darpa: usize,
    oracle_own: usize,
    measured: usize,
    compiler_generated_estimate: usize,
    matched: usize,
    only_oracle_compiler_generated: usize,
    only_oracle_unexplained: Vec<Key>,
    only_measured: Vec<Key>,
}

fn compare(name: &str, crate_dir: &Path, rustc: &Path) -> Option<Comparison> {
    let oracle = run_oracle(crate_dir, rustc);
    let report = measure(crate_dir);
    let Outcome::Measured { result } = &report.safety else {
        // A crate the toolchain cannot compile has no ground truth either.
        assert!(
            !oracle.compiled,
            "{name}: patched rustc compiles it but the measurement failed: {:?}",
            report.safety
        );
        eprintln!("{name}: does not compile with this toolchain; skipped");
        return None;
    };
    let mut measured: BTreeMap<Key, usize> = BTreeMap::new();
    for op in &result.ops {
        *measured.entry(key(op)).or_default() += 1;
    }
    let mut matched = 0;
    let mut only_oracle_compiler_generated = 0;
    let mut only_oracle_unexplained = Vec::new();
    for op in &oracle.own {
        match measured.get_mut(&op.key) {
            Some(n) if *n > 0 => {
                *n -= 1;
                matched += 1;
            }
            _ if COMPILER_GENERATED_CALLEES
                .iter()
                .any(|c| op.callee.starts_with(c)) =>
            {
                only_oracle_compiler_generated += 1
            }
            _ => only_oracle_unexplained.push(op.key.clone()),
        }
    }
    let only_measured = measured
        .into_iter()
        .flat_map(|(k, n)| std::iter::repeat_n(k, n))
        .collect();
    Some(Comparison {
        name: name.to_owned(),
        oracle_darpa: oracle.darpa_count,
        oracle_own: oracle.own.len(),
        measured: result.unsafe_ops,
        compiler_generated_estimate: result.compiler_generated.total(),
        matched,
        only_oracle_compiler_generated,
        only_oracle_unexplained,
        only_measured,
    })
}

fn print_table(rows: &[Comparison]) {
    eprintln!(
        "| crate | patched rustc (DARPA filter) | patched rustc (own files) | measured | matched \
         | only patched: compiler-generated | compiler-generated estimate | only patched: other | only measured |"
    );
    eprintln!("|---|---|---|---|---|---|---|---|---|");
    for r in rows {
        eprintln!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.name,
            r.oracle_darpa,
            r.oracle_own,
            r.measured,
            r.matched,
            r.only_oracle_compiler_generated,
            r.compiler_generated_estimate,
            r.only_oracle_unexplained.len(),
            r.only_measured.len(),
        );
    }
}

fn assert_agreement(r: &Comparison) {
    assert!(
        r.only_measured.is_empty(),
        "{}: counted operations the patched rustc does not report: {:?}",
        r.name,
        &r.only_measured[..r.only_measured.len().min(10)]
    );
    assert!(
        r.only_oracle_unexplained.is_empty(),
        "{}: missed operations outside compiler-generated code: {:?}",
        r.name,
        &r.only_oracle_unexplained[..r.only_oracle_unexplained.len().min(10)]
    );
}

#[test]
#[ignore = "needs HARVEST_UNSAFE_OPS_RUSTC (DARPA's patched rustc)"]
fn fixtures_match_the_patched_rustc() {
    let rustc = patched_rustc();
    let copy = common::fixtures_copy();
    let mut rows = Vec::new();
    for name in [
        "known_ops",
        "edition2024",
        "lib_and_bin",
        "lib_bin_build",
        "deny_in_lib",
        "external_macro",
        "compiler_generated",
        "edition2024_extern",
    ] {
        rows.push(compare(name, &copy.path().join(name), &rustc).expect("fixtures compile"));
    }
    print_table(&rows);
    for r in &rows {
        assert_agreement(r);
        // On the fixtures the source estimate is exact.
        assert_eq!(
            r.only_oracle_compiler_generated, r.compiler_generated_estimate,
            "{}: compiler-generated estimate",
            r.name
        );
    }
}

#[test]
#[ignore = "needs HARVEST_UNSAFE_OPS_RUSTC and HARVEST_UNSAFE_OPS_CRATES"]
fn listed_crates_match_the_patched_rustc() {
    let rustc = patched_rustc();
    let list = std::env::var_os("HARVEST_UNSAFE_OPS_CRATES")
        .expect("set HARVEST_UNSAFE_OPS_CRATES to a file listing crate directories");
    let list = std::fs::read_to_string(&list).expect("read crate list");
    let mut rows = Vec::new();
    for line in list.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let src = Path::new(line);
        let name = src
            .components()
            .rev()
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        // Work on a copy: both sides write build output into the crate.
        let dir = tempfile::tempdir().unwrap();
        let crate_dir = dir.path().join("crate");
        common::copy_tree(src, &crate_dir);
        if let Some(r) = compare(&name, &crate_dir, &rustc) {
            rows.push(r);
        }
    }
    print_table(&rows);
    for r in &rows {
        assert_agreement(r);
    }
}
