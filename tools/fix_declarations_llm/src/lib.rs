//! `FixDeclarationsLlm`: calls the LLM to repair declarations that have compiler errors,
//! producing an updated `SplitPackage` with fixed declarations and a recomputed line index.

use cargo_metadata::diagnostic::{Diagnostic, DiagnosticLevel};
use full_source::CargoPackage;
use harvest_core::tools::{RunContext, Tool};
use harvest_core::{Id, Representation};
use quantize_rust_spans::RustItemMap;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::ops::{Bound, RangeBounds as _};
use std::path::PathBuf;
use syn::spanned::Spanned;
use tracing::{info, warn};
use try_cargo_build::CargoBuildResult;

mod fix_llm;
// Stub helpers (for interface context)

fn stub_item(item: syn::Item, source: &[u8]) -> String {
    let mut source = Vec::from(source);
    match item {
        syn::Item::Fn(f) => {
            let todo = "{ todo!() }".bytes();
            source.splice(f.block.span().byte_range(), todo);
        }
        syn::Item::Impl(impl_block) => {
            let mut pieces = vec![];
            let mut last_end = 0;
            impl_block.items.iter().for_each(|impl_item| {
                if let syn::ImplItem::Fn(method) = impl_item {
                    let byte_range = method.block.span().byte_range();
                    pieces.push(source[last_end..byte_range.start].to_vec());

                    pieces.push(b"{ todo!() }".to_vec());
                    last_end = byte_range.end;
                }
            });
            pieces.push(source[last_end..].to_vec());
            source = pieces.into_iter().flatten().collect();
        }
        _ => {}
    }
    String::from_utf8(source).unwrap()
}

fn stub_declaration(source: &[u8]) -> Option<String> {
    let sstr = str::from_utf8(source).ok()?;
    let Ok(file) = syn::parse_file(sstr) else {
        return Some(sstr.trim_end_matches('\n').to_string());
    };
    file.items
        .into_iter()
        .map(|item| stub_item(item, source))
        .collect::<Vec<_>>()
        .join("\n")
        .into()
}

/// Calls the LLM to fix each declaration that has compiler errors, and returns a new
/// `SplitPackage` with updated declarations and a freshly computed line index.
pub struct FixDeclarationsLlm;

impl Tool for FixDeclarationsLlm {
    fn name(&self) -> &'static str {
        "fix_declarations_llm"
    }

    fn run(
        self: Box<Self>,
        context: RunContext,
        inputs: Vec<Id>,
    ) -> Result<Box<dyn Representation>, Box<dyn std::error::Error>> {
        let config =
            fix_llm::Config::deserialize(context.config.tools.get("fix_declarations_llm").ok_or(
                "No fix_declarations_llm config found in config.toml. \
                     Please add a [tools.fix_declarations_llm] section.",
            )?)?;
        config.validate();

        let fix_llm = fix_llm::FixLlm::new(&config.llm)?;

        let item_map = context
            .ir_snapshot
            .get::<RustItemMap>(inputs[0])
            .ok_or("DiagnosticAttributor: no RustItemMap found in IR")?;

        let mut cargo_package = context
            .ir_snapshot
            .get::<CargoPackage>(item_map.cargo_pkg_idx)
            .ok_or("DiagnosticAttributor: no CargoPackage found in IR")?
            .clone();

        let build_result = context
            .ir_snapshot
            .get::<CargoBuildResult>(inputs[1])
            .ok_or("DiagnosticAttributor: no CargoBuildResult found in IR")?;

        let mut decl_errors: HashMap<(PathBuf, Bound<usize>, Bound<usize>), Vec<Diagnostic>> =
            HashMap::new();

        let error_diagnostics = build_result
            .diagnostics
            .iter()
            .map(|d| &d.message)
            .filter(|m| m.level == DiagnosticLevel::Error);

        for msg in error_diagnostics {
            for span in msg.spans.iter() {
                let file_name = PathBuf::new().join(&span.file_name);
                cargo_package.dir.get_file(&file_name)?;

                let (decl_start, decl_end) = item_map
                    .items
                    .get(&file_name)
                    .ok_or("")?
                    .iter()
                    .find_map(|v| {
                        if v.contains(&(span.byte_start as usize))
                            && v.contains(&(span.byte_end as usize))
                        {
                            Some((v.start_bound().cloned(), v.end_bound().cloned()))
                        } else {
                            None
                        }
                    })
                    .unwrap_or((Bound::Unbounded, Bound::Unbounded));
                decl_errors
                    .entry((file_name, decl_start, decl_end))
                    .or_default()
                    .push(msg.clone());
            }
        }

        // Build an interface context string: all declarations with
        // function bodies replaced by `{ todo!() }`. Used as
        // reference context for the LLM in each fix call.
        let interface_ctx = item_map
            .items
            .iter()
            .flat_map(|(file_name, irs)| Some((cargo_package.dir.get_file(file_name).ok()?, irs)))
            .flat_map(|(src, item_ranges)| {
                item_ranges
                    .iter()
                    .flat_map(|r| stub_declaration(&src[r.clone()]))
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let mut fixed_count = 0usize;

        let mut fixes: HashMap<PathBuf, BTreeMap<(usize, usize), String>> = HashMap::new();

        for ((file_name, start, end), diagnostics) in &decl_errors {
            let source = cargo_package.dir.get_file(file_name)?;
            let decl_source = str::from_utf8(&source[(*start, *end)])?;
            let errors_text = diagnostics
                .iter()
                .map(|d| d.message.clone())
                .collect::<Vec<_>>()
                .join("\n\n");

            match fix_llm.fix_declaration(decl_source, &errors_text, &interface_ctx) {
                Ok(fixed) if !fixed.is_empty() => {
                    let start = match start {
                        Bound::Included(i) => *i,
                        Bound::Excluded(i) => i + 1,
                        Bound::Unbounded => 0,
                    };
                    let end = match end {
                        Bound::Included(i) => i + 1,
                        Bound::Excluded(i) => *i,
                        Bound::Unbounded => 0,
                    };
                    fixes
                        .entry(file_name.clone())
                        .or_default()
                        .insert((start, end), fixed);
                    fixed_count += 1;
                }
                Ok(_) => {
                    warn!("FixDeclarationsLlm: LLM returned empty response for declaration",);
                }
                Err(e) => {
                    warn!("FixDeclarationsLlm: LLM fix failed for declaration: {}", e);
                }
            }
        }

        for (file_name, patches) in fixes.drain() {
            let source = cargo_package.dir.get_file_mut(&file_name)?;
            let mut offset: isize = 0;
            // patches are sorted by start bound since they are stored
            // in a BTree, and start bound is first part of key
            for ((orig_start, orig_end), new_entry) in patches.into_iter() {
                let diff = new_entry.len().cast_signed() - (orig_end - orig_start).cast_signed();
                source.splice(
                    (orig_start.strict_add_signed(offset))..(orig_end.strict_add_signed(offset)),
                    new_entry.bytes(),
                );

                offset += diff;
            }
        }

        info!(
            "FixDeclarationsLlm: fixed {}/{} declarations",
            fixed_count,
            decl_errors.len()
        );

        Ok(Box::new(cargo_package))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_stub_declaration() {
        assert_eq!(
            Some(
                r"
fn foo() { todo!() }
"
                .into()
            ),
            super::stub_declaration(
                br"
fn foo() { println!() }
"
            )
        );

        assert_eq!(
            Some(
                r"

impl Foo {
    fn foo() { todo!() }

    fn bar() { todo!() }
}

"
                .into()
            ),
            super::stub_declaration(
                br"

impl Foo {
    fn foo() { println!() }

    fn bar() {
      scanf!()
    }
}

"
            )
        );
    }
}
