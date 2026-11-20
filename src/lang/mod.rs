//! Language support. A language is data (node kinds, one query) plus a few small hooks.

use std::collections::{BTreeSet, HashMap};
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

/// The short names that a file gives to other modules, as `short name -> full name`.
pub type Aliases = HashMap<String, String>;
pub type FeaturesFn = fn(Node<'_>, &[u8], &Aliases) -> BTreeSet<String>;
pub type ApiFunctionsFn = for<'tree> fn(Node<'tree>, &[u8]) -> Vec<(String, Node<'tree>)>;

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
    pub aliases: fn(Node<'_>, &[u8]) -> Aliases,
    /// The full names of the modules that a file defines at its top level. A feature of one of
    /// these modules, or of a module below one, is local to the scanned code.
    pub modules: fn(Node<'_>, &[u8]) -> Vec<String>,
    /// What a unit uses from other modules: the functions it calls and the types it builds, with
    /// full names. A call is `Module.function`. A type starts with `%`. Two units that share rare
    /// features do related work.
    pub features: FeaturesFn,
    /// True when the file at this path is the public entry of its directory.
    pub api_file: fn(&str) -> bool,
    /// The public functions that a file defines, in source order.
    pub api_functions: ApiFunctionsFn,
}

pub static ALL: &[&LanguageSpec] = &[&elixir::SPEC];

pub fn for_path(path: &Path) -> Option<&'static LanguageSpec> {
    let extension = path.extension()?.to_str()?;
    ALL.iter()
        .copied()
        .find(|spec| spec.extensions.contains(&extension))
}

/// Visits `root` and every named node below it, in source order.
pub fn walk<'tree>(root: Node<'tree>, mut visit: impl FnMut(Node<'tree>)) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        visit(node);
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
}
