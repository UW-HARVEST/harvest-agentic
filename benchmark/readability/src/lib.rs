//! Counts of formal redundancy in Rust source: `as` casts and chains of
//! them, raw pointer types, the C calling convention on functions that C
//! cannot call by name, `current_block` state variables, type aliases
//! repeated with a numeric suffix, C integer type names, and raw pointer
//! arithmetic.
//!
//! Each count is the number of [`Hit`]s of one [`Item`]. A hit records where
//! the shape is, so that every count can be traced back to the source. A
//! count is a number of shapes, not a score: it is meant for comparing
//! translations of the same program, and no count has a limit.
//!
//! Seven items need only the syntax tree ([`Item::SYNTAX`]): [`scan_file`]
//! finds them in one parsed file and [`crate_hits`] joins the files of a
//! crate. They are counted even when the crate does not compile. The other two
//! need the types ([`Item::TYPES`]); they come from a clippy run with the
//! configuration and arguments in [`clippy`].

pub mod clippy;
mod scan;

pub use scan::{FileScan, crate_hits, is_export_attr, scan_file};

use serde::Serialize;
use std::collections::BTreeMap;

/// One kind of shape. The serialized name is the item's key in reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Item {
    /// An `as` cast.
    AsCasts,
    /// A chain of two or more casts, once per longest chain: `x as u32 as
    /// u64` and `(x as u32) as u64` are one chain each. Each cast of a chain
    /// is also an [`Item::AsCasts`] hit.
    CastChains,
    /// A `*const T` or `*mut T` type in the signature of a function
    /// definition, a trait method or a foreign function (`*mut *mut u8` is
    /// two).
    RawPointerTypesInSignatures,
    /// A raw pointer type anywhere else: function bodies (casts, closures,
    /// items inside a body), fields, statics, constants, type aliases.
    RawPointerTypesOutsideSignatures,
    /// A function definition with a foreign ABI (`extern "C" fn`) and
    /// without `#[no_mangle]` or `#[export_name]`: C cannot call it by name.
    /// A function whose address is given to C (a callback) is counted too.
    InternalExternCFunctions,
    /// An identifier that starts with `current_block`, a state variable
    /// that selects the next block, as a `goto` would. Lifetimes and
    /// labels are not identifiers here.
    CurrentBlockUses,
    /// A type alias `B_N` (N digits) when the crate also has a type alias
    /// `B`: the same type under one more name. Each distinct
    /// name counts once, at its first definition.
    SuffixedTypeAliases,
    /// A use of a C integer type (`c_int`, `size_t`, `uint32_t`, …) by any
    /// path, also through a renaming `use`; from clippy's `disallowed_types`.
    CIntegerTypeUses,
    /// A call of a raw pointer arithmetic method (`add`, `offset`,
    /// `wrapping_add`, `byte_add`, …) on a raw pointer, resolved by type;
    /// from clippy's `disallowed_methods`.
    RawPointerArithmeticCalls,
}

impl Item {
    /// The items that [`scan_file`] and [`crate_hits`] find.
    pub const SYNTAX: [Item; 7] = [
        Item::AsCasts,
        Item::CastChains,
        Item::RawPointerTypesInSignatures,
        Item::RawPointerTypesOutsideSignatures,
        Item::InternalExternCFunctions,
        Item::CurrentBlockUses,
        Item::SuffixedTypeAliases,
    ];

    /// The items that the clippy run finds ([`clippy::item_of`]).
    pub const TYPES: [Item; 2] = [Item::CIntegerTypeUses, Item::RawPointerArithmeticCalls];
}

/// Where one shape is. Lines and character columns are 1-based, as in
/// rustc's diagnostics. Hits sort by position.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Hit {
    /// Path relative to the package root, `/`-separated.
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub item: Item,
}

/// The number of hits of each of `items`, with 0 for an item without hits.
/// Hits of other items are ignored.
pub fn count(hits: &[Hit], items: &[Item]) -> BTreeMap<Item, usize> {
    let mut counts: BTreeMap<Item, usize> = items.iter().map(|&item| (item, 0)).collect();
    for hit in hits {
        if let Some(n) = counts.get_mut(&hit.item) {
            *n += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_is_in_one_list_and_has_a_distinct_key() {
        let all: Vec<Item> = Item::SYNTAX.iter().chain(&Item::TYPES).copied().collect();
        let keys: std::collections::BTreeSet<String> = all
            .iter()
            .map(|i| {
                serde_json::to_value(i)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(keys.len(), 9);
        assert!(keys.contains("internal_extern_c_functions"));
        assert!(keys.contains("c_integer_type_uses"));
    }

    #[test]
    fn counts_have_every_requested_item() {
        let hit = |item| Hit {
            file: "src/lib.rs".into(),
            line: 1,
            column: 1,
            item,
        };
        let hits = [
            hit(Item::AsCasts),
            hit(Item::AsCasts),
            hit(Item::CIntegerTypeUses),
        ];
        let c = count(&hits, &Item::SYNTAX);
        assert_eq!(c.len(), 7);
        assert_eq!(c[&Item::AsCasts], 2);
        assert_eq!(c[&Item::CastChains], 0);
        assert!(!c.contains_key(&Item::CIntegerTypeUses));
    }
}
