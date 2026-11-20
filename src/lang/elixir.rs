//! Elixir. The unit is the function clause.
//!
//! The grammar has no function-definition node: `def`, `defp` and the macro forms are all `call`
//! nodes, so the query matches on the call target. Operators are anonymous leaves below
//! `binary_operator`, so the tokens include anonymous leaves.

use tree_sitter::{Language, Node};

use super::{LanguageSpec, UnitParts};

pub static SPEC: LanguageSpec = LanguageSpec {
    name: "elixir",
    extensions: &["ex", "exs"],
    language,
    unit_query: r#"(call target: (identifier) @keyword
  (#any-of? @keyword "def" "defp" "defmacro" "defmacrop")) @unit"#,
    unit_parts,
    skip_kinds: &["comment"],
    identifier_kinds: &["identifier"],
    callee_positions: &[("call", "target"), ("dot", "right")],
    number_kinds: &["integer", "float"],
    string_kinds: &["quoted_content"],
};

fn language() -> Language {
    tree_sitter_elixir::LANGUAGE.into()
}

/// The name is in the first argument of the clause: `name(args)`, a bare `name` for a head with
/// no parentheses, or one of these on the left of a `when` guard.
fn unit_parts(unit: Node<'_>) -> UnitParts<'_> {
    let keyword = unit.child_by_field_name("target");
    let mut cursor = unit.walk();
    let arguments = unit
        .named_children(&mut cursor)
        .find(|child| child.kind() == "arguments");
    let head = arguments
        .and_then(|arguments| arguments.named_child(0))
        .and_then(|head| {
            if head.kind() == "binary_operator" {
                head.named_child(0)
            } else {
                Some(head)
            }
        });
    let name = head.and_then(|head| match head.kind() {
        "call" => head
            .child_by_field_name("target")
            .filter(|target| target.kind() == "identifier"),
        "identifier" => Some(head),
        _ => None,
    });
    UnitParts { keyword, name }
}

#[cfg(test)]
mod tests {
    use crate::unit::{Extractor, Limits, Unit};

    fn units(source: &str) -> Vec<Unit> {
        let limits = Limits {
            min_lines: 1,
            min_tokens: 1,
        };
        Extractor::new(&super::SPEC)
            .unwrap()
            .extract("a.ex", source.as_bytes(), &limits)
            .units
    }

    fn only(source: &str) -> Unit {
        let mut found = units(source);
        assert_eq!(found.len(), 1, "expected one unit in {source:?}");
        found.remove(0)
    }

    #[test]
    fn reads_the_name_of_a_clause_with_a_guard() {
        assert_eq!(
            only("def foo(a) when is_integer(a) do\n  a\nend\n").name,
            "foo"
        );
    }

    #[test]
    fn reads_the_name_of_a_head_without_parentheses() {
        assert_eq!(only("def foo do\n  1\nend\n").name, "foo");
    }

    #[test]
    fn keeps_a_one_line_clause() {
        let unit = only("def foo(a), do: a + 1\n");
        assert_eq!(
            (unit.name.as_str(), unit.start_line, unit.end_line),
            ("foo", 1, 1)
        );
        assert!(unit.strict.contains(&"+".to_string()));
    }

    #[test]
    fn each_clause_is_one_unit() {
        let found = units("defmodule M do\n  def f(1), do: :one\n  def f(2), do: :two\nend\n");
        let lines: Vec<_> = found
            .iter()
            .map(|unit| (unit.name.as_str(), unit.start_line))
            .collect();
        assert_eq!(lines, [("f", 2), ("f", 3)]);
    }

    #[test]
    fn macros_and_private_forms_are_units() {
        let found = units("defmacro a(x), do: x\ndefmacrop b(x), do: x\ndefp c(x), do: x\n");
        let names: Vec<_> = found.iter().map(|unit| unit.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn other_calls_are_not_units() {
        assert!(units("defmodule M do\n  use Foo\n  @x 1\n  foo(1)\nend\n").is_empty());
    }

    #[test]
    fn the_loose_level_ignores_variable_names_numbers_and_strings() {
        let a = only("def f(a, b) do\n  x = a + 1\n  g(x, \"s\", b)\nend\n");
        let b = only("def h(p, q) do\n  y = p + 22\n  g(y, \"other\", q)\nend\n");
        assert_eq!(a.loose_hash, b.loose_hash);
        assert_ne!(a.strict_hash, b.strict_hash);
    }

    #[test]
    fn the_strict_level_ignores_only_the_unit_name() {
        let a = only("def f(a), do: g(a)\n");
        let b = only("def h(a), do: g(a)\n");
        assert_eq!(a.strict_hash, b.strict_hash);
    }

    #[test]
    fn the_loose_level_treats_def_and_defp_as_the_same() {
        let a = only("def f(a), do: g(a)\n");
        let b = only("defp f(a), do: g(a)\n");
        assert_eq!(a.loose_hash, b.loose_hash);
        assert_ne!(a.strict_hash, b.strict_hash);
    }

    #[test]
    fn operators_are_tokens() {
        let a = only("def f(a, b), do: a + b\n");
        let b = only("def f(a, b), do: a - b\n");
        assert_ne!(a.loose_hash, b.loose_hash);
    }

    #[test]
    fn the_loose_level_keeps_callees_modules_and_atoms() {
        let base = only("def f(a), do: Map.put(a, :k, 1)\n");
        for other in [
            "def f(a), do: Map.get(a, :k, 1)\n",
            "def f(a), do: Keyword.put(a, :k, 1)\n",
            "def f(a), do: Map.put(a, :j, 1)\n",
        ] {
            assert_ne!(base.loose_hash, only(other).loose_hash, "{other:?}");
        }
        let local = only("def f(a), do: g(a)\n");
        assert_ne!(local.loose_hash, only("def f(a), do: h(a)\n").loose_hash);
    }

    #[test]
    fn comments_give_no_tokens() {
        let a = only("def f(a) do\n  # why\n  g(a)\nend\n");
        let b = only("def f(a) do\n  g(a)\nend\n");
        assert_eq!(a.strict_hash, b.strict_hash);
    }
}
