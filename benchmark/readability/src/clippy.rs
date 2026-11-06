//! The two items that need types ([`Item::TYPES`]): clippy's
//! `disallowed_types` and `disallowed_methods` lints with the configuration
//! [`CONFIG`], one lint report per hit.
//!
//! The caller writes [`CONFIG`] as `clippy.toml` into a directory, sets
//! `CLIPPY_CONF_DIR` to it, runs `cargo clippy --message-format=json --`
//! with [`ARGS`], and maps each diagnostic with [`item_of`].

use crate::Item;

/// The `clippy.toml` of the run.
pub const CONFIG: &str = include_str!("clippy.toml");

/// The lint arguments of the run. `--force-warn` reports the two lints even
/// where the crate allows them; `-A clippy::all` silences every other clippy
/// lint; `--cap-lints warn` turns the lints that the crate denies into
/// warnings, so that each error left is a compile error.
pub const ARGS: [&str; 8] = [
    "--cap-lints",
    "warn",
    "-A",
    "clippy::all",
    "--force-warn",
    "clippy::disallowed_types",
    "--force-warn",
    "clippy::disallowed_methods",
];

/// The item of a diagnostic with the code `code`, if it is one of the two
/// lints.
pub fn item_of(code: &str) -> Option<Item> {
    match code {
        "clippy::disallowed_types" => Some(Item::CIntegerTypeUses),
        "clippy::disallowed_methods" => Some(Item::RawPointerArithmeticCalls),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_names_both_lists() {
        assert!(CONFIG.contains("disallowed-types = ["));
        assert!(CONFIG.contains("disallowed-methods = ["));
        assert_eq!(
            item_of("clippy::disallowed_types"),
            Some(Item::CIntegerTypeUses)
        );
        assert_eq!(item_of("clippy::needless_return"), None);
    }
}
