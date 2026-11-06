//! The syntax items: one walk over a parsed file finds the casts, chains,
//! raw pointer types, internal `extern "C"` functions and `current_block`
//! identifiers, and collects the type aliases; [`crate_hits`] then finds the
//! suffixed aliases across the files of the crate.
//!
//! The walk sees the items and expressions of the file, and the arguments of
//! each macro call that parse as a comma-separated list of expressions
//! (`assert!(…)`, `format!(…)`). It does not see the body of a `macro_rules!`
//! definition, nor macro arguments of another shape. It does not evaluate
//! `cfg`: code under `#[cfg(test)]` counts too.

use crate::{Hit, Item};
use proc_macro2::{Span, TokenTree};
use std::collections::BTreeSet;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};

/// What one file contributes: its hits, and its type aliases for
/// [`Item::SuffixedTypeAliases`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileScan {
    hits: Vec<Hit>,
    /// The name and the position of each type alias definition.
    type_aliases: Vec<(String, Hit)>,
}

/// The syntax items of `file`, the file at `rel_path`.
pub fn scan_file(rel_path: &str, file: &syn::File) -> FileScan {
    let mut scan = Scan {
        file: rel_path,
        out: FileScan::default(),
        in_signature: false,
        operand_of_cast: false,
    };
    scan.visit_file(file);
    scan.out
}

/// The hits of all syntax items of a crate, sorted by position: the hits of
/// each file, and the suffixed type aliases, which need the aliases of the
/// whole crate.
pub fn crate_hits<'a>(files: impl IntoIterator<Item = &'a FileScan>) -> Vec<Hit> {
    let files: Vec<&FileScan> = files.into_iter().collect();
    let names: BTreeSet<&str> = files
        .iter()
        .flat_map(|f| &f.type_aliases)
        .map(|(name, _)| name.as_str())
        .collect();
    let mut aliases: Vec<&(String, Hit)> = files
        .iter()
        .flat_map(|f| &f.type_aliases)
        .filter(|(name, _)| suffix_base(name).is_some_and(|base| names.contains(base)))
        .collect();
    aliases.sort_by(|a, b| a.1.cmp(&b.1));
    let mut seen = BTreeSet::new();
    let mut hits: Vec<Hit> = files.iter().flat_map(|f| f.hits.iter().cloned()).collect();
    for (name, at) in aliases {
        if seen.insert(name) {
            hits.push(Hit {
                item: Item::SuffixedTypeAliases,
                ..at.clone()
            });
        }
    }
    hits.sort();
    hits
}

/// `B` for a name `B_N`, where N is one or more digits and B is not empty.
fn suffix_base(name: &str) -> Option<&str> {
    let (base, digits) = name.rsplit_once('_')?;
    (!base.is_empty() && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then_some(base)
}

/// `#[no_mangle]`, `#[export_name = "…"]`, `#[unsafe(no_mangle)]`,
/// `#[unsafe(export_name = "…")]`: the function is exported to C by name.
pub fn is_export_attr(attr: &syn::Attribute) -> bool {
    let is_export = |path: &syn::Path| path.is_ident("no_mangle") || path.is_ident("export_name");
    match &attr.meta {
        syn::Meta::Path(p) => is_export(p),
        syn::Meta::NameValue(nv) => is_export(&nv.path),
        syn::Meta::List(list) if list.path.is_ident("unsafe") => {
            matches!(list.tokens.clone().into_iter().next(),
                     Some(TokenTree::Ident(id)) if id == "no_mangle" || id == "export_name")
        }
        syn::Meta::List(_) => false,
    }
}

struct Scan<'a> {
    file: &'a str,
    out: FileScan,
    /// True while the walk is in a function signature.
    in_signature: bool,
    /// True from a cast whose operand (without parentheses) is a cast, to that
    /// operand: the operand cast is then not the top of a chain.
    operand_of_cast: bool,
}

impl Scan<'_> {
    fn at(&self, item: Item, span: Span) -> Hit {
        let start = span.start();
        Hit {
            file: self.file.to_owned(),
            line: start.line,
            column: start.column + 1,
            item,
        }
    }

    fn hit(&mut self, item: Item, span: Span) {
        let hit = self.at(item, span);
        self.out.hits.push(hit);
    }

    /// A function item: `body` is None for a trait method without a default
    /// body and for a foreign function, which are not definitions.
    fn function(
        &mut self,
        attrs: &[syn::Attribute],
        sig: &syn::Signature,
        body: Option<&syn::Block>,
    ) {
        for attr in attrs {
            self.visit_attribute(attr);
        }
        let foreign_abi = sig
            .abi
            .as_ref()
            .is_some_and(|abi| abi.name.as_ref().is_none_or(|name| name.value() != "Rust"));
        if body.is_some() && foreign_abi && !attrs.iter().any(is_export_attr) {
            self.hit(Item::InternalExternCFunctions, sig.ident.span());
        }
        let outer = std::mem::replace(&mut self.in_signature, true);
        self.visit_signature(sig);
        self.in_signature = false;
        if let Some(body) = body {
            self.visit_block(body);
        }
        self.in_signature = outer;
    }
}

/// `expr` without the parentheses and invisible groups around it.
fn strip_parens(mut expr: &syn::Expr) -> &syn::Expr {
    loop {
        match expr {
            syn::Expr::Paren(paren) => expr = &paren.expr,
            syn::Expr::Group(group) => expr = &group.expr,
            other => return other,
        }
    }
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.function(&node.attrs, &node.sig, Some(&node.block));
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.function(&node.attrs, &node.sig, Some(&node.block));
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.function(&node.attrs, &node.sig, node.default.as_ref());
    }

    fn visit_foreign_item_fn(&mut self, node: &'ast syn::ForeignItemFn) {
        self.function(&node.attrs, &node.sig, None);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        let at = self.at(Item::SuffixedTypeAliases, node.ident.span());
        self.out.type_aliases.push((node.ident.to_string(), at));
        visit::visit_item_type(self, node);
    }

    fn visit_expr_cast(&mut self, node: &'ast syn::ExprCast) {
        let top = !std::mem::take(&mut self.operand_of_cast);
        self.hit(Item::AsCasts, node.as_token.span);
        let operand_is_cast = matches!(strip_parens(&node.expr), syn::Expr::Cast(_));
        if top && operand_is_cast {
            self.hit(Item::CastChains, node.as_token.span);
        }
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        self.operand_of_cast = operand_is_cast;
        self.visit_expr(&node.expr);
        self.operand_of_cast = false;
        self.visit_type(&node.ty);
    }

    fn visit_type_ptr(&mut self, node: &'ast syn::TypePtr) {
        let item = if self.in_signature {
            Item::RawPointerTypesInSignatures
        } else {
            Item::RawPointerTypesOutsideSignatures
        };
        self.hit(item, node.star_token.span);
        visit::visit_type_ptr(self, node);
    }

    /// A lifetime or a label is not an identifier of a variable.
    fn visit_lifetime(&mut self, _: &'ast syn::Lifetime) {}

    fn visit_ident(&mut self, node: &'ast proc_macro2::Ident) {
        if node.to_string().starts_with("current_block") {
            self.hit(Item::CurrentBlockUses, node.span());
        }
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        visit::visit_macro(self, node);
        if node.path.is_ident("macro_rules") {
            return;
        }
        let parser = Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
        if let Ok(args) = node.parse_body_with(parser) {
            for arg in &args {
                self.visit_expr(arg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::count;
    use std::collections::BTreeMap;

    fn hits(text: &str) -> Vec<Hit> {
        let file: syn::File = syn::parse_str(text).expect("test input parses");
        crate_hits([&scan_file("src/lib.rs", &file)])
    }

    /// The items with a nonzero count.
    fn counts(text: &str) -> BTreeMap<Item, usize> {
        let mut c = count(&hits(text), &Item::SYNTAX);
        c.retain(|_, n| *n > 0);
        c
    }

    fn only(item: Item, n: usize) -> BTreeMap<Item, usize> {
        BTreeMap::from([(item, n)])
    }

    #[test]
    fn each_item_has_a_hit_and_a_miss() {
        use Item::*;
        let pairs = [
            (
                AsCasts,
                "fn f(x: u8) -> u32 { x as u32 }",
                "fn f(x: u8) -> u32 { u32::from(x) }",
            ),
            (
                InternalExternCFunctions,
                r#"extern "C" fn cmp(a: i32, b: i32) -> i32 { a - b }"#,
                "#[unsafe(no_mangle)]\npub extern \"C\" fn cmp(a: i32, b: i32) -> i32 { a - b }",
            ),
            (
                RawPointerTypesInSignatures,
                "fn len(p: *const u8) {}",
                "fn len(p: &[u8]) {}",
            ),
            (
                RawPointerTypesOutsideSignatures,
                "struct Node { next: *mut Node }",
                "struct Node { next: Option<Box<Node>> }",
            ),
            (
                CurrentBlockUses,
                "fn f() { let current_block: u64 = 0; }",
                "fn f() { let state: u64 = 0; }",
            ),
            (
                SuffixedTypeAliases,
                "type U32 = u32;\ntype U32_1 = u32;",
                "type U32 = u32;",
            ),
        ];
        for (item, hit, miss) in pairs {
            assert_eq!(counts(hit), only(item, 1), "{hit}");
            assert_eq!(counts(miss), BTreeMap::new(), "{miss}");
        }
    }

    #[test]
    fn a_chain_is_counted_once_and_each_of_its_casts_once() {
        let text = "fn f(x: u8, p: *const u8) -> u64 {
            let a = x as u32;
            let b = x as u32 as u64;
            let c = (x as u32) as u64;
            let d = x as i8 as i32 as u64;
            let e = (x as u32 + 1) as u64;
            let g = (x as u32).count_ones() as u64;
            0
        }";
        // Casts: 1 + 2 + 2 + 3 + 2 + 2. Chains: b, c, d; not e or g, where
        // a sum or a method call is between the casts.
        let c = counts(text);
        assert_eq!(c[&Item::AsCasts], 12);
        assert_eq!(c[&Item::CastChains], 3);
    }

    #[test]
    fn raw_pointer_types_are_split_by_place() {
        let text = r#"
pub struct S { p: *mut *mut u8, f: Option<unsafe extern "C" fn(*const u8)> }
static G: usize = 0;
type P = *mut u8;
fn f(a: *mut u8) -> *const u8 {
    fn nested(q: *const u8) {}
    let g = |r: *mut u8| r as *const u8;
    a as *const u8
}
trait T { fn m(&self, p: *const u8); }
unsafe extern "C" { fn memcpy(d: *mut u8, s: *const u8, n: usize) -> *mut u8; }
"#;
        // Signatures: f 2, nested 1, T::m 1, memcpy 3. Outside: S 3 (the
        // function pointer type of a field is not a signature), P 1, the
        // closure 2, the last cast 1.
        let c = counts(text);
        assert_eq!(c[&Item::RawPointerTypesInSignatures], 7);
        assert_eq!(c[&Item::RawPointerTypesOutsideSignatures], 7);
    }

    #[test]
    fn only_definitions_without_an_export_attribute_are_internal() {
        let text = r#"
#[no_mangle]
pub extern "C" fn exported() {}
#[export_name = "e2"]
pub extern "C" fn exported_too() {}
extern "C" fn internal() {}
extern fn default_abi() {}
extern "Rust" fn rust_abi() {}
fn plain() {}
struct S;
impl S { extern "C" fn method() {} }
trait T { extern "C" fn declared(); }
unsafe extern "C" { fn foreign(); }
"#;
        assert_eq!(counts(text), only(Item::InternalExternCFunctions, 3));
    }

    #[test]
    fn lifetimes_and_labels_are_not_identifiers() {
        let text = "fn g<'current_block>(x: &'current_block u8) -> u8 {
            let current_block_7 = 1;
            'current_block_label: { break 'current_block_label current_block_7 }
        }";
        assert_eq!(counts(text), only(Item::CurrentBlockUses, 2));
    }

    #[test]
    fn a_suffixed_alias_needs_its_base_somewhere_in_the_crate() {
        let parse = |text| syn::parse_str::<syn::File>(text).unwrap();
        let a = scan_file(
            "src/a.rs",
            &parse("pub type U32_1 = u32; pub type BYTE_2 = u8;"),
        );
        let b = scan_file(
            "src/b.rs",
            &parse(
                "pub type U32 = u32; pub type BYTE = u8; pub type U32_1 = u32; pub type X_1 = u8; pub type V_ = u8;",
            ),
        );
        let hits = crate_hits([&a, &b]);
        let aliases: Vec<(&str, usize)> = hits
            .iter()
            .filter(|h| h.item == Item::SuffixedTypeAliases)
            .map(|h| (h.file.as_str(), h.column))
            .collect();
        // U32_1 once, at its first definition, and BYTE_2. Not X_1 (no X)
        // or V_ (no digits).
        assert_eq!(aliases, [("src/a.rs", 10), ("src/a.rs", 32)]);
    }

    #[test]
    fn macro_arguments_count_when_they_are_expressions() {
        let text = r#"
fn f(x: u8) {
    assert!(x as u32 == 1, "{}", x as u64);
    m!(struct X; x as u32);
}
macro_rules! cast { ($x:expr) => { $x as u32 as u64 } }
"#;
        // The two casts in assert!. Not m! (not expressions) or the
        // macro_rules! body.
        assert_eq!(counts(text), only(Item::AsCasts, 2));
    }

    #[test]
    fn comments_are_not_code() {
        let text = "// x as u32 as u64; current_block\n/* *mut u8 */\n/// p as usize\nfn f() {}";
        assert_eq!(counts(text), BTreeMap::new());
    }

    #[test]
    fn hits_point_at_the_shape() {
        let h = hits("fn f(x: u8) -> u64 {\n    x as u32 as u64\n}");
        let at: Vec<(Item, usize, usize)> = h.iter().map(|h| (h.item, h.line, h.column)).collect();
        // The chain is reported at the cast that ends it.
        assert_eq!(
            at,
            [
                (Item::AsCasts, 2, 7),
                (Item::AsCasts, 2, 14),
                (Item::CastChains, 2, 14),
            ]
        );
    }
}
