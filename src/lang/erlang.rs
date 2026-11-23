//! Erlang. The unit is the function clause (top-level `function_clause` and `fun_clause` of
//! anonymous or named `fun` expressions).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use tree_sitter::{Language, Node};

use super::{Aliases, LanguageSpec, UnitParts};

pub static SPEC: LanguageSpec = LanguageSpec {
    name: "erlang",
    extensions: &["erl", "hrl", "escript", "yrl"],
    language,
    unit_query: "[(function_clause) (fun_clause)] @unit",
    unit_parts,
    skip_kinds: &["comment", "shebang"],
    identifier_kinds: &["var"],
    callee_positions: &[("macro_call_expr", "name")],
    number_kinds: &["integer", "float", "char"],
    string_kinds: &["string"],
    aliases,
    modules,
    features,
    api_file,
    api_functions,
    preprocess: Some(preprocess),
};

fn language() -> Language {
    tree_sitter_erlang::LANGUAGE.into()
}

fn unit_parts(unit: Node<'_>) -> UnitParts<'_> {
    UnitParts {
        keyword: None,
        name: unit.child_by_field_name("name"),
    }
}

fn aliases(_root: Node<'_>, _source: &[u8]) -> Aliases {
    HashMap::new()
}

fn modules(_root: Node<'_>, _source: &[u8]) -> Vec<String> {
    Vec::new()
}

fn features(_unit: Node<'_>, _source: &[u8], _aliases: &Aliases) -> BTreeSet<String> {
    BTreeSet::new()
}

fn api_file(_path: &str) -> bool {
    false
}

fn api_functions<'tree>(_root: Node<'tree>, _source: &[u8]) -> Vec<(String, Node<'tree>)> {
    Vec::new()
}

/// Blank out the Yecc grammar header in `.yrl` files up through `Erlang code.` while preserving
/// line numbers.
fn preprocess(path: &str, source: &[u8]) -> Option<Vec<u8>> {
    if Path::new(path).extension().is_none_or(|ext| ext != "yrl") {
        return None;
    }
    let mut offset = 0;
    let mut erlang_start = None;
    for line in source.split_inclusive(|&b| b == b'\n') {
        offset += line.len();
        if is_erlang_code_directive(line) {
            erlang_start = Some(offset);
            break;
        }
    }
    let Some(start) = erlang_start else {
        return Some(Vec::new());
    };
    let mut out = Vec::with_capacity(source.len());
    for &b in &source[..start] {
        out.push(if b == b'\n' || b == b'\r' { b } else { b' ' });
    }
    out.extend_from_slice(&source[start..]);
    Some(out)
}

fn is_erlang_code_directive(line: &[u8]) -> bool {
    let trimmed = trim_ascii_start(line);
    let Some(rest) = trimmed.strip_prefix(b"Erlang") else {
        return false;
    };
    if rest.first().is_none_or(|b| !b.is_ascii_whitespace()) {
        return false;
    }
    let rest = trim_ascii_start(rest);
    let Some(after) = rest.strip_prefix(b"code.") else {
        return false;
    };
    after
        .first()
        .is_none_or(|&b| b.is_ascii_whitespace() || b == b'%')
}

fn trim_ascii_start(mut bytes: &[u8]) -> &[u8] {
    while let Some((first, rest)) = bytes.split_first() {
        if first.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::lang;
    use crate::unit::{Extractor, Limits, Unit};

    fn units_for(path: &str, source: &str) -> Vec<Unit> {
        let limits = Limits {
            min_lines: 1,
            min_tokens: 1,
        };
        Extractor::new(&super::SPEC)
            .unwrap()
            .extract(path, source.as_bytes(), &limits)
            .units
    }

    fn units(source: &str) -> Vec<Unit> {
        units_for("a.erl", source)
    }

    fn only(source: &str) -> Unit {
        let mut found = units(source);
        assert_eq!(found.len(), 1, "expected one unit in {source:?}");
        found.remove(0)
    }

    #[test]
    fn reads_the_name_of_a_clause_with_a_guard() {
        assert_eq!(
            only("foo(A) when is_integer(A) ->\n    A.\n").name,
            "foo"
        );
    }

    #[test]
    fn each_clause_is_one_unit() {
        let found = units("f(1) -> one;\nf(2) -> two.\n");
        let lines: Vec<_> = found
            .iter()
            .map(|unit| (unit.name.as_str(), unit.start_line))
            .collect();
        assert_eq!(lines, [("f", 1), ("f", 2)]);
    }

    #[test]
    fn anonymous_and_named_funs_are_units() {
        let found = units(
            "outer(List) ->\n    lists:map(fun(X) -> X + 1 end, List),\n    Loop = fun Loop(0) -> ok; Loop(N) -> Loop(N - 1) end,\n    Loop(3).\n",
        );
        let names: Vec<_> = found.iter().map(|unit| unit.name.as_str()).collect();
        assert_eq!(names, ["outer", "?", "Loop", "Loop"]);

        let named_a = units("f() -> fun Loop(X) -> g(X) end.\n").remove(1);
        let named_b = units("f() -> fun Iter(X) -> g(X) end.\n").remove(1);
        assert_eq!(named_a.strict_hash, named_b.strict_hash);

        let rec_a = units("f() -> fun Loop(X) -> Loop(X - 1) end.\n").remove(1);
        let rec_b = units("f() -> fun Iter(Y) -> Iter(Y - 1) end.\n").remove(1);
        assert_eq!(rec_a.loose_hash, rec_b.loose_hash);
    }

    #[test]
    fn loose_level_ignores_variable_names_numbers_chars_and_strings() {
        let a = only("f(A, B) ->\n    X = A + 1,\n    g(X, \"s\", B).\n");
        let b = only("h(P, Q) ->\n    Y = P + 22,\n    g(Y, \"other\", Q).\n");
        assert_eq!(a.loose_hash, b.loose_hash);
        assert_ne!(a.strict_hash, b.strict_hash);

        let num_a = only("f(_) -> g(1, 1_000, 16#FF, 2#1010, 3.14, 1.0e-5, 16#F.8, $a).\n");
        let num_b = only("f(_) -> g(9, 2_000, 16#00, 2#0101, 2.71, 2.0e+3, 16#A.1, $\\n).\n");
        assert_eq!(num_a.loose_hash, num_b.loose_hash);
        assert_ne!(num_a.strict_hash, num_b.strict_hash);

        let base_str = only("f(A) -> g(A, \"hello\").\n");
        let string_variants = [
            "f(B) -> g(B, \"hello\nworld\").\n",
            "f(B) -> g(B, <<\"binary\">>).\n",
            "f(B) -> g(B, \"\"\"\n  triple\n  \"\"\").\n",
            "f(B) -> g(B, ~\"sigil\").\n",
            "f(B) -> g(B, ~b\"sigil\").\n",
            "f(B) -> g(B, ~B\"sigil\").\n",
            "f(B) -> g(B, ~s\"sigil\").\n",
            "f(B) -> g(B, ~S\"sigil\").\n",
            "f(B) -> g(B, ~\"\"\"\n  sigil triple\n  \"\"\").\n",
            "f(B) -> g(B, ~S\"\"\"\n  verbatim triple\n  \"\"\").\n",
        ];
        // Regular, multiline, triple-quoted, and sigil strings all tokenize as `string` leaves.
        // Binary strings `<<"binary">>` have surrounding `<<` and `>>` tokens.
        for variant in string_variants {
            let parsed = only(variant);
            if variant.contains("<<") {
                let other_bin = only("f(A) -> g(A, <<\"other\">>).\n");
                assert_eq!(parsed.loose_hash, other_bin.loose_hash, "{variant:?}");
            } else {
                assert_eq!(base_str.loose_hash, parsed.loose_hash, "{variant:?}");
            }
        }
    }

    #[test]
    fn strict_level_ignores_only_the_unit_name() {
        let a = only("f(A) -> g(A).\n");
        let b = only("h(A) -> g(A).\n");
        assert_eq!(a.strict_hash, b.strict_hash);
    }

    #[test]
    fn operators_are_tokens() {
        let a = only("f(A, B) -> A + B.\n");
        let b = only("f(A, B) -> A - B.\n");
        assert_ne!(a.loose_hash, b.loose_hash);
    }

    #[test]
    fn loose_level_keeps_callees_modules_atoms_and_macros() {
        let base = only("f(A) -> maps:put(A, k, 1).\n");
        for other in [
            "f(A) -> maps:get(A, k, 1).\n",
            "f(A) -> lists:put(A, k, 1).\n",
            "f(A) -> maps:put(A, j, 1).\n",
        ] {
            assert_ne!(base.loose_hash, only(other).loose_hash, "{other:?}");
        }
        let local = only("f(A) -> g(A).\n");
        assert_ne!(local.loose_hash, only("f(A) -> h(A).\n").loose_hash);

        let macro_a = only("f(A) -> ?LOG_INFO(A).\n");
        let macro_b = only("f(A) -> ?LOG_ERROR(A).\n");
        assert_ne!(macro_a.loose_hash, macro_b.loose_hash);
    }

    #[test]
    fn comments_and_shebangs_give_no_tokens() {
        let a = only("f(A) ->\n    % why\n    g(A).\n");
        let b = only("f(A) ->\n    g(A).\n");
        assert_eq!(a.strict_hash, b.strict_hash);

        let escript_a = units_for(
            "script.escript",
            "#!/usr/bin/env escript\n%%! -sname foo\nmain(Args) ->\n    io:format(\"~p~n\", [Args]).\n",
        );
        let escript_b = units_for(
            "script.escript",
            "main(Args) ->\n    io:format(\"~p~n\", [Args]).\n",
        );
        assert_eq!(escript_a.len(), 1);
        assert_eq!(escript_a[0].strict_hash, escript_b[0].strict_hash);
    }

    #[test]
    fn extensions_match_erl_hrl_escript_yrl() {
        for ext in ["erl", "hrl", "escript", "yrl"] {
            let path = format!("src/mod.{ext}");
            let spec = lang::for_path(Path::new(&path)).expect("should match erlang");
            assert_eq!(spec.name, "erlang");
        }

        let hrl = units_for(
            "defs.hrl",
            "-define(HANDLER(X), fun(Y) -> X + Y end).\n",
        );
        assert_eq!(hrl.len(), 1);
        assert_eq!(hrl[0].name, "?");

        let yrl = units_for(
            "parser.yrl",
            "Nonterminals expr.\nTerminals int.\nRootsymbol expr.\nexpr -> int : '$1'.\nErlang code.\nunwrap({_, _, Val}) -> Val.\n",
        );
        assert_eq!(yrl.len(), 1);
        assert_eq!((yrl[0].name.as_str(), yrl[0].start_line), ("unwrap", 6));

        let yrl_no_code = units_for(
            "grammar_only.yrl",
            "Nonterminals expr.\nTerminals int.\nRootsymbol expr.\nexpr -> int : '$1'.\n",
        );
        assert!(yrl_no_code.is_empty());
    }
}
