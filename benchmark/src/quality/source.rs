//! Static reading of a crate's Rust source: how many code lines each file
//! holds, the rewrite that exposes their unsafe operations, where
//! each function body lies, and the unsafe operations the compiler generates
//! that no source rewrite can expose.
//!
//! Everything here is lexical or syntactic (`proc-macro2` tokens, `syn`
//! items); nothing compiles. [`analyze`] lexes each file once and parses it
//! once, and derives everything from those. Positions follow rustc's
//! diagnostic convention (1-based lines, 1-based character columns) so they
//! can be matched against compiler messages directly.

use super::strip::{strip_unsafe_blocks, Stripped};
use proc_macro2::{TokenStream, TokenTree};
use std::borrow::Cow;
use std::collections::BTreeSet;
use syn::visit::Visit;

/// What one source file contributes to the measurement.
pub struct AnalyzedFile {
    /// Path relative to the package root, `/`-separated, as rustc reports it.
    pub rel_path: String,
    /// Lines that carry at least one token: comments and blank lines are
    /// excluded by construction. 0 when the file does not lex.
    pub code_lines: usize,
    /// The file with its `unsafe` blocks blanked, or the lexer error.
    pub stripped: Result<Stripped, String>,
    /// Function bodies; empty when the file does not parse (its positions
    /// stay unattributed rather than being guessed).
    pub fn_index: FnIndex,
    pub compiler_generated: CompilerGenerated,
}

/// Lexes `text`, the file at `rel_path`, once, parses the tokens once, and
/// derives everything the measurement needs from them.
pub fn analyze(rel_path: String, text: &str) -> AnalyzedFile {
    let without_shebang = blank_shebang(text);
    let tokens: TokenStream = match without_shebang.parse() {
        Ok(tokens) => tokens,
        Err(e) => {
            return AnalyzedFile {
                rel_path,
                code_lines: 0,
                stripped: Err(e.to_string()),
                fn_index: FnIndex::default(),
                compiler_generated: CompilerGenerated::default(),
            }
        }
    };
    let mut lines = BTreeSet::new();
    token_lines(tokens.clone(), &without_shebang, &mut lines);
    let mut compiler_generated = CompilerGenerated::default();
    scan_tokens(tokens.clone(), &mut compiler_generated);
    // The rewrite edits the original text: blanking keeps every offset.
    let stripped = strip_unsafe_blocks(text, tokens.clone());
    let mut fn_index = FnIndex::default();
    if let Ok(ast) = syn::parse2::<syn::File>(tokens) {
        ItemScan {
            fn_index: &mut fn_index,
            found: &mut compiler_generated,
        }
        .visit_file(&ast);
    }
    AnalyzedFile {
        rel_path,
        code_lines: lines.len(),
        stripped: Ok(stripped),
        fn_index,
        compiler_generated,
    }
}

/// `text` with its shebang line, if any, blanked to spaces of the same byte
/// length. rustc skips a first line that starts with `#!` unless an inner
/// attribute (`#![…]`) starts there; the lexer would read it as tokens, and
/// the file would then fail to parse.
fn blank_shebang(text: &str) -> Cow<'_, str> {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    match body.strip_prefix("#!") {
        Some(rest) if !rest.trim_start().starts_with('[') => {
            let start = text.len() - body.len();
            let end = start + body.find('\n').unwrap_or(body.len());
            let mut blanked = text.to_owned();
            blanked.replace_range(start..end, &" ".repeat(end - start));
            Cow::Owned(blanked)
        }
        _ => Cow::Borrowed(text),
    }
}

/// Collects the lines that hold code tokens. Doc comments reach the token
/// stream as `#[doc = "…"]` attributes whose tokens all point into the
/// comment; they are comments, so they are skipped.
fn token_lines(stream: TokenStream, text: &str, lines: &mut BTreeSet<usize>) {
    for tt in stream {
        let start = text.get(tt.span().byte_range().start..).unwrap_or("");
        if start.starts_with("//") || start.starts_with("/*") {
            continue;
        }
        match tt {
            TokenTree::Group(g) => {
                lines.insert(g.span_open().start().line);
                lines.insert(g.span_close().start().line);
                token_lines(g.stream(), text, lines);
            }
            other => {
                let span = other.span();
                lines.extend(span.start().line..=span.end().line);
            }
        }
    }
}

/// A function body, located by its braces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnBody {
    pub name: String,
    /// Exported to C: `#[no_mangle]` or `#[export_name]`, in either the plain
    /// or the `#[unsafe(...)]` spelling.
    pub exported: bool,
    /// Declared `unsafe fn`: its body is an unsafe context.
    pub unsafe_sig: bool,
    /// 1-based line and character column of the opening and closing brace.
    pub open: (usize, usize),
    pub close: (usize, usize),
}

/// The function bodies of one file — free functions, inherent and
/// trait-impl methods, provided trait methods, nested functions — for
/// attributing a source position to the innermost function that contains it.
#[derive(Debug, Default)]
pub struct FnIndex {
    bodies: Vec<FnBody>,
}

impl FnIndex {
    /// The innermost function whose body contains (line, column).
    pub fn innermost(&self, line: usize, column: usize) -> Option<&FnBody> {
        let pos = (line, column);
        self.bodies
            .iter()
            .filter(|b| b.open <= pos && pos <= b.close)
            .max_by_key(|b| b.open)
    }
}

/// One walk over a file's items: function bodies for [`FnIndex`], and the
/// derives that [`CompilerGenerated`] counts.
struct ItemScan<'a> {
    fn_index: &'a mut FnIndex,
    found: &'a mut CompilerGenerated,
}

impl ItemScan<'_> {
    fn push(&mut self, attrs: &[syn::Attribute], sig: &syn::Signature, block: &syn::Block) {
        let open = block.brace_token.span.open().start();
        let close = block.brace_token.span.close().start();
        self.fn_index.bodies.push(FnBody {
            name: sig.ident.to_string(),
            exported: attrs.iter().any(is_export_attr),
            unsafe_sig: sig.unsafety.is_some(),
            open: (open.line, open.column + 1),
            close: (close.line, close.column + 1),
        });
    }
}

impl<'ast> Visit<'ast> for ItemScan<'_> {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.push(&f.attrs, &f.sig, &f.block);
        syn::visit::visit_item_fn(self, f);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.push(&f.attrs, &f.sig, &f.block);
        syn::visit::visit_impl_item_fn(self, f);
    }

    fn visit_trait_item_fn(&mut self, f: &'ast syn::TraitItemFn) {
        if let Some(block) = &f.default {
            self.push(&f.attrs, &f.sig, block);
        }
        syn::visit::visit_trait_item_fn(self, f);
    }

    fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
        let all_carry_data = e.variants.len() > 1
            && e.variants
                .iter()
                .all(|v| !matches!(v.fields, syn::Fields::Unit) && !v.fields.is_empty());
        if all_carry_data && derives(&e.attrs, "PartialEq") {
            self.found.derived_partial_eq += 1;
        }
        syn::visit::visit_item_enum(self, e);
    }
}

/// `#[no_mangle]`, `#[export_name = "…"]`, `#[unsafe(no_mangle)]`,
/// `#[unsafe(export_name = "…")]`.
fn is_export_attr(attr: &syn::Attribute) -> bool {
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

/// Unsafe operations the compiler wraps in its own unsafe blocks. DARPA's
/// patched rustc counts them, but they never appear in the source, so the
/// strip-and-recompile method cannot see them; they are counted here from
/// the constructs that generate them (rustc 1.94):
///
/// * `format_args!` with arguments, written in the crate (directly or in a
///   crate-local macro): one call to the unsafe `fmt::Arguments::new`. The
///   standard library's formatting macros (`format!`, `println!`, `write!`)
///   generate the same call, but its span lies in the standard library and
///   DARPA's filter drops it, so they are not counted.
/// * `.await`: `Pin::new_unchecked` and `get_context`, two operations.
/// * `#[derive(PartialEq)]` on an enum whose variants all carry data:
///   one call to `intrinsics::unreachable`.
///
/// An estimate: a crate-local macro that expands `format_args!` several
/// times counts once, and code under `#[cfg(test)]` is included.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CompilerGenerated {
    pub format_args: usize,
    pub await_points: usize,
    pub derived_partial_eq: usize,
}

impl CompilerGenerated {
    /// Operations these constructs generate.
    pub fn total(&self) -> usize {
        self.format_args + 2 * self.await_points + self.derived_partial_eq
    }

    pub fn add(&mut self, other: &Self) {
        self.format_args += other.format_args;
        self.await_points += other.await_points;
        self.derived_partial_eq += other.derived_partial_eq;
    }
}

fn scan_tokens(stream: TokenStream, found: &mut CompilerGenerated) {
    let tts: Vec<TokenTree> = stream.into_iter().collect();
    for (i, tt) in tts.iter().enumerate() {
        match tt {
            TokenTree::Group(g) => scan_tokens(g.stream(), found),
            TokenTree::Ident(id) if id == "format_args" => {
                let bang =
                    matches!(tts.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!');
                if let (true, Some(TokenTree::Group(args))) = (bang, tts.get(i + 2)) {
                    if format_args_has_arguments(args.stream()) {
                        found.format_args += 1;
                    }
                }
            }
            TokenTree::Ident(id) if id == "await" => {
                if matches!(i.checked_sub(1).and_then(|j| tts.get(j)),
                            Some(TokenTree::Punct(p)) if p.as_char() == '.')
                {
                    found.await_points += 1;
                }
            }
            _ => {}
        }
    }
}

/// `format_args!("literal")` lowers to the safe `Arguments::from_str`;
/// anything with a placeholder or an argument lowers to `Arguments::new`.
/// Input that is not a string literal (a macro body forwarding `$($t)*`) is
/// counted, since it is forwarded arguments.
fn format_args_has_arguments(args: TokenStream) -> bool {
    let mut tts = args.into_iter();
    let Some(TokenTree::Literal(lit)) = tts.next() else {
        return true;
    };
    if tts.next().is_some() {
        return true;
    }
    let text = lit.to_string();
    let unescaped = text.replace("{{", "");
    unescaped.contains('{')
}

fn derives(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("derive"))
        .any(|a| {
            let mut hit = false;
            let _ = a.parse_nested_meta(|meta| {
                hit |= meta.path.segments.last().is_some_and(|s| s.ident == name);
                Ok(())
            });
            hit
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_text(text: &str) -> AnalyzedFile {
        analyze("src/lib.rs".into(), text)
    }

    #[test]
    fn code_lines_skip_comments_and_blank_lines() {
        let text =
            "// header\n\nfn f() {\n    /* block\n       comment */\n    let s = \"a\nb\";\n}\n";
        // `fn f() {`, `let s = "a`, `b";`, `}`
        assert_eq!(analyze_text(text).code_lines, 4);
    }

    #[test]
    fn doc_comments_are_not_code_lines() {
        let text = "//! crate doc\n/// item doc\n/** block\n    doc */\nfn f() {}\n#[doc = \"written\"]\nfn g() {}\n";
        // `fn f() {}`, the hand-written attribute, `fn g() {}`
        assert_eq!(analyze_text(text).code_lines, 3);
    }

    #[test]
    fn a_shebang_is_skipped_but_kept_in_the_rewrite() {
        let text = "#!/usr/bin/env run-cargo-script\nfn main() { unsafe { g() } }\n";
        let a = analyze_text(text);
        assert_eq!(a.code_lines, 1);
        assert_eq!(a.fn_index.innermost(2, 22).unwrap().name, "main");
        let stripped = a.stripped.unwrap();
        assert!(stripped
            .text
            .starts_with("#!/usr/bin/env run-cargo-script\n"));
        assert_eq!(stripped.blocks, 1);
        // An inner attribute on the first line is code, not a shebang.
        let attr = analyze_text("#![allow(dead_code)]\nfn f() {}\n");
        assert_eq!(attr.code_lines, 2);
        assert!(attr.fn_index.innermost(2, 9).is_some());
    }

    #[test]
    fn one_pass_yields_the_rewrite_and_its_block_count() {
        let a = analyze_text("fn f(p: *const u8) -> u8 { unsafe { *p } }");
        let stripped = a.stripped.unwrap();
        assert_eq!(stripped.blocks, 1);
        assert!(!stripped.text.contains("unsafe"));
    }

    #[test]
    fn a_file_that_does_not_lex_is_reported_not_guessed() {
        let a = analyze_text("fn f() { \"unterminated }");
        assert!(a.stripped.is_err());
        assert_eq!(a.code_lines, 0);
        assert!(a.fn_index.innermost(1, 10).is_none());
    }

    #[test]
    fn a_file_that_lexes_but_does_not_parse_keeps_its_token_counts() {
        let a = analyze_text("fn fn format_args!(\"{}\", 1)");
        assert!(a.stripped.is_ok());
        assert_eq!(a.code_lines, 1);
        assert!(a.fn_index.innermost(1, 1).is_none());
        assert_eq!(a.compiler_generated.format_args, 1);
    }

    #[test]
    fn innermost_function_wins_and_exports_are_recognized() {
        let text = r#"
#[no_mangle]
pub extern "C" fn outer() {
    fn inner() {
        let x = 1;
    }
}
#[unsafe(export_name = "e")]
pub unsafe extern "C" fn two() {}
impl S { fn m(&self) { } }
"#;
        let idx = analyze_text(text).fn_index;
        assert_eq!(idx.innermost(5, 9).unwrap().name, "inner");
        assert!(!idx.innermost(5, 9).unwrap().exported);
        // Line 6 is `    }`: column 5 is inner's closing brace, column 6 is
        // back in outer.
        assert_eq!(idx.innermost(6, 5).unwrap().name, "inner");
        assert_eq!(idx.innermost(6, 6).unwrap().name, "outer");
        assert!(idx.innermost(6, 6).unwrap().exported);
        assert!(idx.innermost(9, 33).unwrap().exported);
        assert_eq!(idx.innermost(10, 23).unwrap().name, "m");
        assert!(
            idx.innermost(10, 21).is_none(),
            "between impl and fn bodies"
        );
        assert!(idx.innermost(1, 1).is_none());
    }

    #[test]
    fn compiler_generated_constructs_are_counted() {
        let text = r#"
#[derive(PartialEq, PartialOrd)]
enum Data { A(u8), B { x: u16 } }
#[derive(PartialEq)]
enum Mixed { A(u8), C }
#[derive(Debug)]
enum NoEq { A(u8), B(u8) }
fn f(x: u8) -> String { std::fmt::format(format_args!("{}", x)) }
fn g() -> String { std::fmt::format(format_args!("plain {{}}")) }
fn h(x: u8) -> String { std::fmt::format(format_args!("{x}")) }
macro_rules! m { ($($t:tt)*) => { format_args!($($t)*) } }
async fn a() { b().await; b().await }
fn s() -> String { format!("{}", 1) }
"#;
        let c = analyze_text(text).compiler_generated;
        assert_eq!(c.format_args, 3, "f, h and the forwarding macro body");
        assert_eq!(c.await_points, 2);
        assert_eq!(
            c.derived_partial_eq, 1,
            "only Data: Mixed has a unit variant"
        );
        assert_eq!(c.total(), 3 + 4 + 1);
    }
}
