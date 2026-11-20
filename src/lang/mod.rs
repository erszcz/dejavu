//! Language support. A language is data (node kinds, one query) plus one small hook.

use std::path::Path;

use tree_sitter::{Language, Node};

pub mod elixir;

/// The two leaves of a unit that are normalised apart from the general rules.
pub struct UnitParts<'tree> {
    /// The keyword that defines the unit, for example `def` or `defp`.
    pub keyword: Option<Node<'tree>>,
    /// The unit's own name.
    pub name: Option<Node<'tree>>,
}

pub struct LanguageSpec {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
    pub language: fn() -> Language,
    /// A tree-sitter query. The capture `@unit` marks one comparison unit.
    pub unit_query: &'static str,
    pub unit_parts: for<'tree> fn(Node<'tree>) -> UnitParts<'tree>,
    /// Subtrees of these kinds give no tokens.
    pub skip_kinds: &'static [&'static str],
    /// Leaves of these kinds are names. The loose level replaces them, unless they are callees.
    pub identifier_kinds: &'static [&'static str],
    /// `(parent kind, field name)` positions where an identifier names the thing that is called.
    /// The loose level keeps these, because the callee is what the code does.
    pub callee_positions: &'static [(&'static str, &'static str)],
    pub number_kinds: &'static [&'static str],
    pub string_kinds: &'static [&'static str],
}

pub static ALL: &[&LanguageSpec] = &[&elixir::SPEC];

pub fn for_path(path: &Path) -> Option<&'static LanguageSpec> {
    let extension = path.extension()?.to_str()?;
    ALL.iter()
        .copied()
        .find(|spec| spec.extensions.contains(&extension))
}
