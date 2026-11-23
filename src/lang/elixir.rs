//! Elixir. The unit is the function clause.
//!
//! The grammar has no function-definition node: `def`, `defp` and the macro forms are all `call`
//! nodes, so the query matches on the call target. Operators are anonymous leaves below
//! `binary_operator`, so the tokens include anonymous leaves.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use tree_sitter::{Language, Node};

use super::{walk, LanguageSpec, UnitParts};

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
    aliases,
    modules,
    features,
    api_file,
    api_functions,
    preprocess: None,
};

fn language() -> Language {
    tree_sitter_elixir::LANGUAGE.into()
}

fn text<'a>(node: Node<'_>, source: &'a [u8]) -> Cow<'a, str> {
    String::from_utf8_lossy(&source[node.byte_range()])
}

/// The text of a call's target, when the target is a plain identifier such as `def` or `alias`.
fn target_text<'a>(call: Node<'_>, source: &'a [u8]) -> Option<Cow<'a, str>> {
    call.child_by_field_name("target")
        .filter(|target| target.kind() == "identifier")
        .map(|target| text(target, source))
}

fn arguments(call: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = call.walk();
    let found = call
        .named_children(&mut cursor)
        .find(|child| child.kind() == "arguments");
    found
}

fn first_argument(call: Node<'_>) -> Option<Node<'_>> {
    arguments(call).and_then(|arguments| arguments.named_child(0))
}

/// The name in a function head: `name(args)`, a bare `name` for a head with no parentheses, or
/// one of these on the left of a `when` guard or of the `::` of a callback.
fn head_name(mut head: Node<'_>) -> Option<Node<'_>> {
    while head.kind() == "binary_operator" {
        head = head.named_child(0)?;
    }
    match head.kind() {
        "call" => head
            .child_by_field_name("target")
            .filter(|target| target.kind() == "identifier"),
        "identifier" => Some(head),
        _ => None,
    }
}

fn unit_parts(unit: Node<'_>) -> UnitParts<'_> {
    UnitParts {
        keyword: unit.child_by_field_name("target"),
        name: first_argument(unit).and_then(head_name),
    }
}

fn last_segment(module: &str) -> &str {
    module.rsplit('.').next().unwrap_or(module)
}

/// `alias A.B`, `alias A.B, as: C` and `alias A.{B, C.D}`, anywhere in the file.
fn aliases(root: Node<'_>, source: &[u8]) -> HashMap<String, String> {
    let mut found = HashMap::new();
    walk(root, |node| {
        if node.kind() != "call" || target_text(node, source).as_deref() != Some("alias") {
            return;
        }
        let Some(arguments) = arguments(node) else {
            return;
        };
        let Some(first) = arguments.named_child(0) else {
            return;
        };
        match first.kind() {
            "alias" => {
                let full = text(first, source).into_owned();
                let short =
                    alias_as(arguments, source).unwrap_or_else(|| last_segment(&full).to_string());
                found.insert(short, full);
            }
            "dot" => {
                let base = first.child_by_field_name("left");
                let names = first.child_by_field_name("right");
                let (Some(base), Some(names)) = (base, names) else {
                    return;
                };
                if base.kind() != "alias" || names.kind() != "tuple" {
                    return;
                }
                let base = text(base, source);
                let mut cursor = names.walk();
                for name in names.named_children(&mut cursor) {
                    if name.kind() == "alias" {
                        let name = text(name, source);
                        found.insert(last_segment(&name).to_string(), format!("{base}.{name}"));
                    }
                }
            }
            _ => {}
        }
    });
    found
}

/// `defmodule` and `defprotocol` at the top level of the file.
fn modules(root: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|node| node.kind() == "call")
        .filter(|node| {
            matches!(
                target_text(*node, source).as_deref(),
                Some("defmodule" | "defprotocol")
            )
        })
        .filter_map(first_argument)
        .filter(|name| name.kind() == "alias")
        .map(|name| text(name, source).into_owned())
        .collect()
}

/// The `X` of `as: X` in the arguments of an alias.
fn alias_as(arguments: Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = arguments.walk();
    let keywords = arguments
        .named_children(&mut cursor)
        .find(|child| child.kind() == "keywords")?;
    let mut cursor = keywords.walk();
    let found = keywords.named_children(&mut cursor).find_map(|pair| {
        let key = pair.named_child(0)?;
        let value = pair.named_child(1)?;
        let is_as = text(key, source).trim_end().trim_end_matches(':') == "as";
        (is_as && value.kind() == "alias").then(|| text(value, source).into_owned())
    });
    found
}

/// A module name with its first segment replaced by what the file's aliases say.
fn resolve(module: &str, aliases: &HashMap<String, String>) -> String {
    let (head, rest) = module.split_once('.').unwrap_or((module, ""));
    match aliases.get(head) {
        Some(full) if rest.is_empty() => full.clone(),
        Some(full) => format!("{full}.{rest}"),
        None => module.to_string(),
    }
}

/// Remote calls as `Module.function` and struct literals as `%Module`.
fn features(unit: Node<'_>, source: &[u8], aliases: &HashMap<String, String>) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    walk(unit, |node| match node.kind() {
        "call" => {
            let Some(target) = node.child_by_field_name("target") else {
                return;
            };
            let module = target.child_by_field_name("left");
            let function = target.child_by_field_name("right");
            if let ("dot", Some(module), Some(function)) = (target.kind(), module, function) {
                if module.kind() == "alias" && function.kind() == "identifier" {
                    let module = resolve(&text(module, source), aliases);
                    found.insert(format!("{module}.{}", text(function, source)));
                }
            }
        }
        "struct" => {
            let mut cursor = node.walk();
            let module = node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "alias");
            if let Some(module) = module {
                found.insert(format!("%{}", resolve(&text(module, source), aliases)));
            }
        }
        _ => {}
    });
    found
}

/// A context's public module has the name of its directory: `folders/folders.ex`.
fn api_file(path: &str) -> bool {
    let path = Path::new(path);
    let directory = path.parent().and_then(Path::file_name);
    directory.is_some() && path.file_stem() == directory
}

/// `def`, `defdelegate`, `defmacro` and `@callback`. A context that dispatches to an
/// implementation declares its functions as callbacks.
fn api_functions<'tree>(root: Node<'tree>, source: &[u8]) -> Vec<(String, Node<'tree>)> {
    let mut found = Vec::new();
    walk(root, |node| {
        let head = match node.kind() {
            "call" => match target_text(node, source).as_deref() {
                Some("def" | "defdelegate" | "defmacro") => first_argument(node),
                _ => None,
            },
            "unary_operator" => node
                .named_child(0)
                .filter(|inner| inner.kind() == "call")
                .filter(|inner| target_text(*inner, source).as_deref() == Some("callback"))
                .and_then(first_argument),
            _ => None,
        };
        if let Some(name) = head.and_then(head_name) {
            found.push((text(name, source).into_owned(), node));
        }
    });
    found
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

    fn outlines(source: &str) -> Vec<crate::unit::Outline> {
        let limits = Limits {
            min_lines: 1,
            min_tokens: 1,
        };
        let wanted = crate::unit::Wanted {
            features: true,
            api: false,
        };
        Extractor::new(&super::SPEC)
            .unwrap()
            .with(wanted)
            .extract("lib/app/a/a.ex", source.as_bytes(), &limits)
            .outlines
    }

    fn api(path: &str, source: &str) -> Vec<(String, u32)> {
        let limits = Limits {
            min_lines: 1,
            min_tokens: 1,
        };
        let wanted = crate::unit::Wanted {
            features: false,
            api: true,
        };
        Extractor::new(&super::SPEC)
            .unwrap()
            .with(wanted)
            .extract(path, source.as_bytes(), &limits)
            .api
            .into_iter()
            .map(|function| (function.name, function.start_line))
            .collect()
    }

    #[test]
    fn features_are_remote_calls_and_structs_with_full_names() {
        let source = "defmodule M do
  alias App.Billing
  alias App.Accounts.User, as: Account
  alias App.Shared.{Repo, Errors.NotFound}

  def f(id) do
    user = Repo.get(Account, id)
    local(user)
    Billing.Plans.charge(user)
    Enum.map([], & &1)
    %NotFound{id: id}
  end
end
";
        let found = outlines(source);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (
                found[0].name.as_str(),
                found[0].start_line,
                found[0].end_line
            ),
            ("f", 6, 12)
        );
        assert_eq!(
            found[0].features,
            [
                "%App.Shared.Errors.NotFound",
                "App.Billing.Plans.charge",
                "App.Shared.Repo.get",
                "Enum.map"
            ]
        );
    }

    #[test]
    fn modules_are_the_top_level_definitions() {
        let source = "defmodule App.A do\n  defmodule Inner do\n  end\nend\n\ndefprotocol App.P do\nend\n\ndefimpl App.P, for: App.A do\nend\n";
        let tree = {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&super::language()).unwrap();
            parser.parse(source, None).unwrap()
        };
        assert_eq!(
            super::modules(tree.root_node(), source.as_bytes()),
            ["App.A", "App.P"]
        );
    }

    #[test]
    fn a_small_unit_has_an_outline_too() {
        let found = outlines("def f(a), do: Map.get(a, :k)\n");
        assert_eq!(found[0].features, ["Map.get"]);
    }

    #[test]
    fn the_public_entry_file_has_the_name_of_its_directory() {
        assert!(super::api_file("lib/app/folders/folders.ex"));
        assert!(!super::api_file("lib/app/folders/internal/folders_impl.ex"));
        assert!(!super::api_file("folders.ex"));
    }

    #[test]
    fn api_functions_are_public_definitions_and_callbacks_once_each() {
        let source = "defmodule App.Folders do
  @callback create_folder(map()) :: {:ok, term()} | {:error, term()}
  @callback list_folders(binary()) :: [term()] when a: term()
  defdelegate get_folder(id), to: Impl
  def delete_folder(id) when is_binary(id), do: Impl.delete(id)
  def delete_folder(_), do: :error
  defp helper(x), do: x
  @doc \"text\"
  defmacro update_folder(x), do: x
end
";
        assert_eq!(
            api("lib/app/folders/folders.ex", source),
            [
                ("create_folder".to_string(), 2),
                ("list_folders".to_string(), 3),
                ("get_folder".to_string(), 4),
                ("delete_folder".to_string(), 5),
                ("update_folder".to_string(), 9),
            ]
        );
        assert_eq!(api("lib/app/folders/internal/impl.ex", source), []);
    }

    #[test]
    fn comments_give_no_tokens() {
        let a = only("def f(a) do\n  # why\n  g(a)\nend\n");
        let b = only("def f(a) do\n  g(a)\nend\n");
        assert_eq!(a.strict_hash, b.strict_hash);
    }
}
