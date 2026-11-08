//! The measurement's outputs: each snapshot's `.harvest/quality.json`,
//! `.harvest/unsafe_ops.tsv` and `.harvest/readability_hits.tsv`, the run's
//! `quality.csv` with one headline row per program, and the three summary
//! lines logged per program.

use super::readability::ItemCounts;
use super::{Outcome, QualityReport, READABILITY_HITS_FILE, REPORT_FILE, UNSAFE_OPS_FILE};
use harvest_core::stage_manifest::meta_dir;
use harvest_readability::{Hit, Item};
use serde::Serialize;
use std::path::Path;

/// The status an [`Outcome`] records, by itself (for the summary CSV). Its
/// serialized names follow the same convention as the `status` tag of
/// `Outcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Measured,
    DoesNotCompile,
    NotRun,
}

impl<T> Outcome<T> {
    fn status(&self) -> Status {
        match self {
            Outcome::Measured { .. } => Status::Measured,
            Outcome::DoesNotCompile { .. } => Status::DoesNotCompile,
            Outcome::NotRun { .. } => Status::NotRun,
        }
    }
}

/// Writes the report, the operation list and the readability hits into the
/// snapshot's `.harvest/`.
pub(super) fn write(program_dir: &Path, report: &QualityReport) -> std::io::Result<()> {
    let dir = meta_dir(program_dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(REPORT_FILE), serde_json::to_string_pretty(report)?)?;
    let tsv = |name| {
        csv::WriterBuilder::new()
            .delimiter(b'\t')
            .from_path(dir.join(name))
    };
    let ops = report.safety.result().map_or(&[][..], |s| &s.ops);
    write_table(tsv(UNSAFE_OPS_FILE)?, ops, &UNSAFE_OPS_COLUMNS)?;
    let r = &report.readability;
    let mut hits: Vec<&Hit> = [&r.syntax, &r.types]
        .into_iter()
        .filter_map(Outcome::result)
        .flat_map(|c| &c.hits)
        .collect();
    hits.sort();
    write_table(tsv(READABILITY_HITS_FILE)?, hits, &READABILITY_HITS_COLUMNS)
}

/// Writes `rows` with a header of `columns`, the field names of `T`.
fn write_table<T: Serialize>(
    mut w: csv::Writer<std::fs::File>,
    rows: impl IntoIterator<Item = T>,
    columns: &[&str],
) -> std::io::Result<()> {
    let mut empty = true;
    for row in rows {
        w.serialize(row)?;
        empty = false;
    }
    if empty {
        // Serialization writes the header with the first row; keep it for an
        // empty table too.
        w.write_record(columns)?;
    }
    w.flush()
}

/// Columns of [`UNSAFE_OPS_FILE`]: the fields of [`super::safety::UnsafeOp`], in
/// order.
const UNSAFE_OPS_COLUMNS: [&str; 11] = [
    "file",
    "line",
    "column",
    "end_line",
    "end_column",
    "kind",
    "callee",
    "in_unsafe_fn",
    "scope",
    "function",
    "target",
];

/// Columns of [`READABILITY_HITS_FILE`]: the fields of [`Hit`], in order.
const READABILITY_HITS_COLUMNS: [&str; 4] = ["file", "line", "column", "item"];

pub(super) fn log_report(r: &QualityReport) {
    match r.safety.as_result() {
        Ok(s) => log::info!(
            "  Safety: {} unsafe operations ({}; {} in exported functions, {} internal, {} \
             outside functions), {} more in compiler-generated code",
            s.unsafe_ops,
            rate(r.per_kloc(s.unsafe_ops)),
            s.in_exported_functions,
            s.in_internal_functions,
            s.outside_functions,
            s.compiler_generated.total(),
        ),
        Err(why) => log::info!("  Safety: {why}"),
    }
    match r.clippy.as_result() {
        Ok(i) => log::info!(
            "  Clippy: {} clippy lints ({}), {} rustc warnings; cognitive complexity \
             mean {:.2}, max {}",
            i.lints.clippy_lints,
            rate(r.per_kloc(i.lints.clippy_lints)),
            i.lints.rustc_warnings,
            i.cognitive_complexity.mean,
            i.cognitive_complexity.max,
        ),
        Err(why) => log::info!("  Clippy: {why}"),
    }
    let line = |o: &Outcome<ItemCounts>| match o.as_result() {
        Ok(c) => c
            .items
            .iter()
            .map(|(item, n)| format!("{} {n}", item_key(*item)))
            .collect::<Vec<_>>()
            .join(", "),
        Err(why) => why,
    };
    let groups = [&r.readability.syntax, &r.readability.types];
    let measured: Vec<usize> = groups
        .iter()
        .filter_map(|o| o.as_result().ok())
        .map(|c| c.items.values().sum())
        .collect();
    let total = match measured.len() {
        0 => String::new(),
        n => {
            let total = measured.iter().sum();
            let partial = if n < groups.len() {
                ", items not measured excluded"
            } else {
                ""
            };
            format!("{total} in total ({}{partial}); ", rate(r.per_kloc(total)))
        }
    };
    log::info!(
        "  Readability: {total}{}; needing types: {}",
        line(&r.readability.syntax),
        line(&r.readability.types)
    );
}

/// The report key of `item`.
fn item_key(item: Item) -> String {
    serde_json::to_value(item)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn rate(per_kloc: Option<f64>) -> String {
    per_kloc.map_or("no code lines".to_owned(), |v| {
        format!("{v:.1} per 1000 code lines")
    })
}

/// One row of [`super::SUMMARY_CSV`]; the field names are the column names.
/// Measurements that were not made leave their columns empty.
#[derive(Serialize)]
struct SummaryRow<'a> {
    program_name: &'a str,
    rustc: Option<&'a str>,
    rust_code_lines: usize,
    safety_status: Status,
    unsafe_ops: Option<usize>,
    unsafe_ops_per_kloc: Option<String>,
    unsafe_ops_in_exported_functions: Option<usize>,
    unsafe_ops_in_internal_functions: Option<usize>,
    unsafe_ops_outside_functions: Option<usize>,
    unsafe_ops_in_unsafe_fn_bodies: Option<usize>,
    compiler_generated_ops_estimate: Option<usize>,
    unsafe_blocks: Option<usize>,
    clippy_status: Status,
    clippy_lints: Option<usize>,
    clippy_lints_per_kloc: Option<String>,
    rustc_warnings: Option<usize>,
    cognitive_complexity_mean: Option<String>,
    cognitive_complexity_max: Option<usize>,
    readability_syntax_status: Status,
    as_casts_per_kloc: Option<String>,
    cast_chains_per_kloc: Option<String>,
    raw_pointer_types_in_signatures_per_kloc: Option<String>,
    raw_pointer_types_outside_signatures_per_kloc: Option<String>,
    internal_extern_c_functions: Option<usize>,
    current_block_uses: Option<usize>,
    suffixed_type_aliases: Option<usize>,
    readability_types_status: Status,
    c_integer_type_uses_per_kloc: Option<String>,
    raw_pointer_arithmetic_calls_per_kloc: Option<String>,
}

impl<'a> SummaryRow<'a> {
    fn new(name: &'a str, r: &'a QualityReport) -> Self {
        let two_places = |v: f64| format!("{v:.2}");
        let s = r.safety.result();
        let i = r.clippy.result();
        let syntax = r.readability.syntax.result();
        let types = r.readability.types.result();
        let count = |c: Option<&ItemCounts>, item| c.map(|c| c.items[&item]);
        let rate = |c: Option<&ItemCounts>, item| {
            count(c, item).and_then(|n| r.per_kloc(n)).map(two_places)
        };
        Self {
            program_name: name,
            rustc: r.rustc.as_deref(),
            rust_code_lines: r.rust_code_lines,
            safety_status: r.safety.status(),
            unsafe_ops: s.map(|s| s.unsafe_ops),
            unsafe_ops_per_kloc: s.and_then(|s| r.per_kloc(s.unsafe_ops)).map(two_places),
            unsafe_ops_in_exported_functions: s.map(|s| s.in_exported_functions),
            unsafe_ops_in_internal_functions: s.map(|s| s.in_internal_functions),
            unsafe_ops_outside_functions: s.map(|s| s.outside_functions),
            unsafe_ops_in_unsafe_fn_bodies: s.map(|s| s.in_unsafe_fn_bodies),
            compiler_generated_ops_estimate: s.map(|s| s.compiler_generated.total()),
            unsafe_blocks: s.map(|s| s.unsafe_blocks),
            clippy_status: r.clippy.status(),
            clippy_lints: i.map(|i| i.lints.clippy_lints),
            clippy_lints_per_kloc: i
                .and_then(|i| r.per_kloc(i.lints.clippy_lints))
                .map(two_places),
            rustc_warnings: i.map(|i| i.lints.rustc_warnings),
            cognitive_complexity_mean: i.map(|i| two_places(i.cognitive_complexity.mean)),
            cognitive_complexity_max: i.map(|i| i.cognitive_complexity.max),
            readability_syntax_status: r.readability.syntax.status(),
            as_casts_per_kloc: rate(syntax, Item::AsCasts),
            cast_chains_per_kloc: rate(syntax, Item::CastChains),
            raw_pointer_types_in_signatures_per_kloc: rate(
                syntax,
                Item::RawPointerTypesInSignatures,
            ),
            raw_pointer_types_outside_signatures_per_kloc: rate(
                syntax,
                Item::RawPointerTypesOutsideSignatures,
            ),
            internal_extern_c_functions: count(syntax, Item::InternalExternCFunctions),
            current_block_uses: count(syntax, Item::CurrentBlockUses),
            suffixed_type_aliases: count(syntax, Item::SuffixedTypeAliases),
            readability_types_status: r.readability.types.status(),
            c_integer_type_uses_per_kloc: rate(types, Item::CIntegerTypeUses),
            raw_pointer_arithmetic_calls_per_kloc: rate(types, Item::RawPointerArithmeticCalls),
        }
    }
}

/// Columns of [`super::SUMMARY_CSV`]: the fields of [`SummaryRow`], in order.
const SUMMARY_COLUMNS: [&str; 29] = [
    "program_name",
    "rustc",
    "rust_code_lines",
    "safety_status",
    "unsafe_ops",
    "unsafe_ops_per_kloc",
    "unsafe_ops_in_exported_functions",
    "unsafe_ops_in_internal_functions",
    "unsafe_ops_outside_functions",
    "unsafe_ops_in_unsafe_fn_bodies",
    "compiler_generated_ops_estimate",
    "unsafe_blocks",
    "clippy_status",
    "clippy_lints",
    "clippy_lints_per_kloc",
    "rustc_warnings",
    "cognitive_complexity_mean",
    "cognitive_complexity_max",
    "readability_syntax_status",
    "as_casts_per_kloc",
    "cast_chains_per_kloc",
    "raw_pointer_types_in_signatures_per_kloc",
    "raw_pointer_types_outside_signatures_per_kloc",
    "internal_extern_c_functions",
    "current_block_uses",
    "suffixed_type_aliases",
    "readability_types_status",
    "c_integer_type_uses_per_kloc",
    "raw_pointer_arithmetic_calls_per_kloc",
];

/// One headline row per program.
pub(super) fn write_summary_csv(
    path: &Path,
    rows: &[(String, QualityReport)],
) -> std::io::Result<()> {
    let w = csv::Writer::from_path(path)?;
    let rows = rows
        .iter()
        .map(|(name, report)| SummaryRow::new(name, report));
    write_table(w, rows, &SUMMARY_COLUMNS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::readability::ReadabilityReport;
    use crate::quality::{safety, SUMMARY_CSV};

    #[test]
    fn status_names_match_the_outcome_tags() {
        let outcomes: [Outcome<u8>; 3] = [
            Outcome::Measured { result: 1 },
            Outcome::DoesNotCompile { errors: vec![] },
            Outcome::NotRun {
                reason: String::new(),
            },
        ];
        for o in outcomes {
            assert_eq!(
                serde_json::to_value(o.status()).unwrap(),
                serde_json::to_value(&o).unwrap()["status"]
            );
        }
    }

    #[test]
    fn unsafe_ops_columns_are_the_fields_serde_writes() {
        let op = safety::UnsafeOp {
            file: "src/lib.rs".into(),
            line: 1,
            column: 2,
            end_line: 1,
            end_column: 5,
            kind: safety::OpKind::DerefOfRawPointer,
            callee: None,
            in_unsafe_fn: false,
            scope: safety::OpScope::Internal,
            function: Some("f".into()),
            target: "x".into(),
        };
        let mut w = csv::WriterBuilder::new()
            .delimiter(b'\t')
            .from_writer(vec![]);
        w.serialize(&op).unwrap();
        let text = String::from_utf8(w.into_inner().unwrap()).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(UNSAFE_OPS_COLUMNS.join("\t").as_str()));
        assert_eq!(
            lines.next(),
            Some("src/lib.rs\t1\t2\t1\t5\tDerefOfRawPointer\t\tfalse\tinternal\tf\tx")
        );
    }

    #[test]
    fn readability_hits_columns_are_the_fields_serde_writes() {
        let hit = Hit {
            file: "src/lib.rs".into(),
            line: 3,
            column: 7,
            item: Item::AsCasts,
        };
        let mut w = csv::WriterBuilder::new()
            .delimiter(b'\t')
            .from_writer(vec![]);
        w.serialize(&hit).unwrap();
        let text = String::from_utf8(w.into_inner().unwrap()).unwrap();
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some(READABILITY_HITS_COLUMNS.join("\t").as_str())
        );
        assert_eq!(lines.next(), Some("src/lib.rs\t3\t7\tas_casts"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn summary_csv_has_its_header_with_or_without_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SUMMARY_CSV);
        let header = SUMMARY_COLUMNS.join(",");
        write_summary_csv(&path, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), header + "\n");
        let report = QualityReport {
            schema_version: 2,
            rustc: None,
            rust_code_lines: 0,
            safety: Outcome::NotRun { reason: "x".into() },
            clippy: Outcome::NotRun { reason: "x".into() },
            readability: ReadabilityReport {
                syntax: Outcome::NotRun { reason: "x".into() },
                types: Outcome::NotRun { reason: "x".into() },
            },
        };
        write_summary_csv(&path, &[("p".into(), report)]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(SUMMARY_COLUMNS.join(",").as_str()));
        assert_eq!(
            lines.next(),
            Some("p,,0,not_run,,,,,,,,,not_run,,,,,,not_run,,,,,,,,not_run,,")
        );
    }
}
