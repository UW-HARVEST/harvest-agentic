//! Readability: counts of formal redundancy, per item, by
//! [`harvest_readability`]. The seven
//! syntax items come from the files the crate compiles and need no compiler,
//! so they are counted also when the crate does not compile. The two items
//! that need types come from one more clippy run.

use super::scratch::{Package, ScratchArea};
use super::source::AnalyzedFile;
use super::MeasureError;
use cargo_metadata::diagnostic::DiagnosticLevel;
use harvest_readability::{Hit, Item};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Readability counts of one package.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadabilityReport {
    pub syntax: super::Outcome<ItemCounts>,
    pub types: super::Outcome<ItemCounts>,
}

/// The counts of a group of items, from their hits.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemCounts {
    /// Item → its number of hits, 0 included.
    pub items: BTreeMap<Item, usize>,
    /// Compiled files that do not parse, so that their syntax items are
    /// missing from the counts (syntax items only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files_not_parsed: Vec<String>,
    /// Every hit, sorted by position; written to the hit list, not to the
    /// report.
    #[serde(skip)]
    pub hits: Vec<Hit>,
}

impl ItemCounts {
    fn new(items: &[Item], hits: Vec<Hit>) -> Self {
        Self {
            items: harvest_readability::count(&hits, items),
            files_not_parsed: Vec::new(),
            hits,
        }
    }
}

/// The syntax items of `files`.
pub(super) fn syntax(files: &[AnalyzedFile]) -> ItemCounts {
    let scans = files.iter().filter_map(|f| f.readability.as_ref());
    let mut counts = ItemCounts::new(&Item::SYNTAX, harvest_readability::crate_hits(scans));
    counts.files_not_parsed = files
        .iter()
        .filter(|f| f.readability.is_none())
        .map(|f| f.rel_path.clone())
        .collect();
    counts
}

/// The items that need types: a clippy run over the package's
/// default targets with the readability configuration. A source position
/// counts once, also when two targets compile its file or a macro expands
/// there several times, as the syntax items count it.
pub(super) fn types(scratch: &ScratchArea, pkg: &Package) -> Result<ItemCounts, MeasureError> {
    let conf_dir = scratch.path().join("readability_clippy_conf");
    std::fs::create_dir_all(&conf_dir).map_err(MeasureError::not_run("create clippy config"))?;
    std::fs::write(
        conf_dir.join("clippy.toml"),
        harvest_readability::clippy::CONFIG,
    )
    .map_err(MeasureError::not_run("write clippy config"))?;
    let args = harvest_readability::clippy::ARGS.map(str::to_owned);
    let run = super::clippy::run(pkg, &args, Some(&conf_dir))?;
    let mut hits = Vec::new();
    let mut errors = Vec::new();
    for m in &run.messages {
        let d = &m.message;
        if matches!(d.level, DiagnosticLevel::Error | DiagnosticLevel::Ice) {
            errors.push(d.rendered.clone().unwrap_or_else(|| d.message.clone()));
            continue;
        }
        let Some(item) = d
            .code
            .as_ref()
            .and_then(|c| harvest_readability::clippy::item_of(&c.code))
        else {
            continue;
        };
        if let Some(span) = d.spans.iter().find(|s| s.is_primary) {
            if pkg.owns_file(&span.file_name) {
                let file = Path::new(&span.file_name);
                hits.push(Hit {
                    file: file
                        .strip_prefix(&pkg.root)
                        .unwrap_or(file)
                        .to_string_lossy()
                        .into_owned(),
                    line: span.line_start,
                    column: span.column_start,
                    item,
                });
            }
        }
    }
    super::clippy::check_run(pkg, &run, errors)?;
    hits.sort();
    hits.dedup();
    Ok(ItemCounts::new(&Item::TYPES, hits))
}
