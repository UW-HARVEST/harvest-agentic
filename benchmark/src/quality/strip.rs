//! The source rewrite behind the unsafe-operation count: blank the keyword of
//! every `unsafe { … }` block so that each operation inside it surfaces as a
//! compiler diagnostic.
//!
//! The rewrite is lexical. In Rust grammar the keyword `unsafe` directly
//! followed by `{` is always an unsafe block (`unsafe fn`, `unsafe impl`,
//! `unsafe extern`, `unsafe trait` and `#[unsafe(…)]` are followed by
//! something else), and the tokenizer already skips comments, strings and
//! raw strings. Blocks inside `macro_rules!` bodies are rewritten too, which
//! matches DARPA's count of the operations a macro expansion performs.
//!
//! The keyword is replaced by text of the same byte length, so every byte
//! offset, line and column of the rewritten file is that of the original.

use proc_macro2::{Delimiter, TokenStream, TokenTree};

/// Replacement for `unsafe`: six spaces.
const BLANK: &[u8; 6] = b"      ";

/// Replacement for `unsafe` right after `..`/`..=`: a block label.
///
/// `for i in 0..unsafe { (*p).n } { … }` with the keyword blanked parses as
/// an open range followed by the loop body `{ (*p).n }`; the resulting type
/// error makes rustc skip the whole function, silently losing its operations.
/// A labelled block keeps the range bound an expression. It is used only
/// here: elsewhere a label changes meaning (`break unsafe { … }`).
const LABEL: &[u8; 6] = b"'z:   ";

/// A rewritten file.
#[derive(Debug, PartialEq, Eq)]
pub struct Stripped {
    pub text: String,
    /// Number of `unsafe` blocks rewritten.
    pub blocks: usize,
}

/// Blanks the keyword of every `unsafe { … }` block in `text`, given the
/// tokens `text` lexes to (the caller lexes each file once for all uses).
pub fn strip_unsafe_blocks(text: &str, tokens: TokenStream) -> Stripped {
    let mut edits = Vec::new();
    collect(tokens, &mut edits);
    let mut bytes = text.as_bytes().to_vec();
    for (range, replacement) in &edits {
        debug_assert_eq!(range.len(), replacement.len());
        bytes[range.clone()].copy_from_slice(*replacement);
    }
    Stripped {
        // Only ASCII bytes inside an ASCII keyword were replaced.
        text: String::from_utf8(bytes).expect("rewrite keeps UTF-8 intact"),
        blocks: edits.len(),
    }
}

type Edit = (std::ops::Range<usize>, &'static [u8; 6]);

fn collect(stream: TokenStream, edits: &mut Vec<Edit>) {
    let tts: Vec<TokenTree> = stream.into_iter().collect();
    for (i, tt) in tts.iter().enumerate() {
        match tt {
            TokenTree::Group(g) => collect(g.stream(), edits),
            TokenTree::Ident(id) if id == "unsafe" && opens_block(tts.get(i + 1)) => {
                let replacement = if follows_range_operator(&tts[..i]) {
                    LABEL
                } else {
                    BLANK
                };
                edits.push((id.span().byte_range(), replacement));
            }
            _ => {}
        }
    }
}

fn opens_block(next: Option<&TokenTree>) -> bool {
    matches!(next, Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace)
}

/// True when the tokens before end in `..` or `..=`.
fn follows_range_operator(before: &[TokenTree]) -> bool {
    let tail: Vec<char> = before
        .iter()
        .rev()
        .take(3)
        .map_while(|t| match t {
            TokenTree::Punct(p) => Some(p.as_char()),
            _ => None,
        })
        .collect();
    matches!(tail.as_slice(), ['.', '.', ..] | ['=', '.', '.'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stripped(text: &str) -> Stripped {
        strip_unsafe_blocks(text, text.parse().unwrap())
    }

    fn strip(text: &str) -> String {
        stripped(text).text
    }

    #[test]
    fn blanks_blocks_and_keeps_offsets() {
        let src = "fn f(p: *const u8) -> u8 { unsafe { *p } }";
        let out = stripped(src);
        assert_eq!(out.text, "fn f(p: *const u8) -> u8 {        { *p } }");
        assert_eq!(out.text.len(), src.len());
        assert_eq!(out.blocks, 1);
    }

    #[test]
    fn leaves_other_uses_of_the_keyword_alone() {
        let src = r##"
unsafe fn a() {}
unsafe impl Send for S {}
unsafe trait T {}
unsafe extern "C" { fn c(); }
#[unsafe(no_mangle)] pub extern "C" fn d() {}
// unsafe { in a comment }
const S: &str = "unsafe { in a string }";
const R: &str = r#"unsafe { raw }"#;
"##;
        let out = stripped(src);
        assert_eq!(out.blocks, 0);
        assert_eq!(out.text, src);
    }

    #[test]
    fn rewrites_nested_blocks_and_macro_bodies() {
        let src = "macro_rules! m { ($e:expr) => { unsafe { $e } } }\nfn f() { unsafe { unsafe { g() } } }";
        let out = stripped(src);
        assert_eq!(out.blocks, 3);
        assert!(!out.text.contains("unsafe"));
    }

    #[test]
    fn labels_a_block_that_ends_a_range() {
        assert_eq!(
            strip("for i in 0..unsafe { (*c).n } {}"),
            "for i in 0..'z:    { (*c).n } {}"
        );
        assert_eq!(
            strip("for i in 0..=unsafe { n } {}"),
            "for i in 0..='z:    { n } {}"
        );
        // Not a range: `=` and `==` keep the plain blank.
        assert_eq!(strip("let x = unsafe { n };"), "let x =        { n };");
        assert_eq!(strip("if a == unsafe { n } {}"), "if a ==        { n } {}");
    }

    #[test]
    fn non_ascii_text_keeps_byte_offsets() {
        let src = "// 中文注释\nfn f() { let s = \"é\"; unsafe { g() } }";
        let out = stripped(src);
        assert_eq!(out.text.len(), src.len());
        assert_eq!(out.text.find("{ g()"), src.find("{ g()"));
        assert!(!out.text.contains("unsafe"));
    }
}
