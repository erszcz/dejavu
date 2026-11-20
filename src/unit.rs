//! Units: the pieces of code that are compared. `Extractor::extract` reads one file's content and
//! nothing else, so its result can be stored by content hash later.

use anyhow::{anyhow, Result};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, Query, QueryCursor};

use crate::fingerprint::{hash_bytes, hash_sequence, minhash, shingles};
use crate::lang::LanguageSpec;

pub struct Unit {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub name: String,
    /// Token texts at the strict level: only the unit's own name is replaced.
    pub strict: Vec<String>,
    pub strict_hash: u64,
    /// The hash of the loose token sequence: names, numbers and strings are replaced too.
    pub loose_hash: u64,
    /// Shingles of the loose token sequence, as a sorted set.
    pub shingles: Vec<u64>,
    pub signature: Vec<u64>,
}

impl Unit {
    pub fn tokens(&self) -> usize {
        self.strict.len()
    }
}

pub struct Limits {
    pub min_lines: u32,
    pub min_tokens: usize,
}

/// The extra facts to collect. Each one costs a second walk of the tree.
#[derive(Clone, Copy, Default)]
pub struct Wanted {
    /// The features of every unit, for the shared-call signal.
    pub features: bool,
    /// The public functions of a public entry file, for the concept signal.
    pub api: bool,
}

/// Where a unit is and what it uses from other modules. Every unit has one, of any size.
#[derive(Debug, PartialEq)]
pub struct Outline {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub name: String,
    pub features: Vec<String>,
}

/// A public function of a public entry file. A function with several clauses is here once.
#[derive(Debug, PartialEq)]
pub struct ApiFunction {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub name: String,
}

#[derive(Default)]
pub struct FileUnits {
    /// All units in the file, before the size limits.
    pub total: usize,
    pub units: Vec<Unit>,
    pub outlines: Vec<Outline>,
    /// The modules that the file defines. Collected with the features.
    pub modules: Vec<String>,
    pub api: Vec<ApiFunction>,
}

pub struct Extractor {
    spec: &'static LanguageSpec,
    parser: Parser,
    query: Query,
    unit_capture: u32,
    wanted: Wanted,
}

// Placeholders are hashed with a prefix that no token text can start with.
const PLACEHOLDER: u8 = 0;

fn placeholder(name: &str) -> u64 {
    hash_bytes(&[&[PLACEHOLDER], name.as_bytes()].concat())
}

impl Extractor {
    pub fn new(spec: &'static LanguageSpec) -> Result<Self> {
        let language = (spec.language)();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .map_err(|error| anyhow!("{}: {error}", spec.name))?;
        let query = Query::new(&language, spec.unit_query)
            .map_err(|error| anyhow!("{} unit query: {error}", spec.name))?;
        let unit_capture = query
            .capture_index_for_name("unit")
            .ok_or_else(|| anyhow!("{} unit query has no @unit capture", spec.name))?;
        Ok(Self {
            spec,
            parser,
            query,
            unit_capture,
            wanted: Wanted::default(),
        })
    }

    pub fn with(mut self, wanted: Wanted) -> Self {
        self.wanted = wanted;
        self
    }

    pub fn extract(&mut self, path: &str, source: &[u8], limits: &Limits) -> FileUnits {
        let Some(tree) = self.parser.parse(source, None) else {
            return FileUnits::default();
        };
        let mut nodes = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source);
        while let Some(found) = matches.next() {
            nodes.extend(
                found
                    .captures
                    .iter()
                    .filter(|c| c.index == self.unit_capture)
                    .map(|c| c.node),
            );
        }
        nodes.sort_by_key(|node| (node.start_byte(), node.end_byte()));
        nodes.dedup_by_key(|node| node.id());

        let spec = self.spec;
        let modules = if self.wanted.features {
            (spec.modules)(tree.root_node(), source)
        } else {
            Vec::new()
        };
        let outlines = if self.wanted.features {
            let aliases = (spec.aliases)(tree.root_node(), source);
            nodes
                .iter()
                .map(|node| Outline {
                    path: path.to_string(),
                    start_line: start_line(node),
                    end_line: end_line(node),
                    name: name((spec.unit_parts)(*node).name, source),
                    features: (spec.features)(*node, source, &aliases)
                        .into_iter()
                        .collect(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut api: Vec<ApiFunction> = Vec::new();
        if self.wanted.api && (spec.api_file)(path) {
            for (name, node) in (spec.api_functions)(tree.root_node(), source) {
                if api.iter().all(|function| function.name != name) {
                    api.push(ApiFunction {
                        path: path.to_string(),
                        start_line: start_line(&node),
                        end_line: end_line(&node),
                        name,
                    });
                }
            }
        }

        let total = nodes.len();
        let units = nodes
            .into_iter()
            .filter(|node| lines(node) >= limits.min_lines)
            .filter_map(|node| self.unit(path, node, source, limits))
            .collect();
        FileUnits {
            total,
            units,
            outlines,
            modules,
            api,
        }
    }

    fn unit(&self, path: &str, node: Node, source: &[u8], limits: &Limits) -> Option<Unit> {
        let spec = self.spec;
        let parts = (spec.unit_parts)(node);
        let name_id = parts.name.map(|name| name.id());
        let keyword_id = parts.keyword.map(|keyword| keyword.id());

        let mut strict = Vec::new();
        let mut strict_hashes = Vec::new();
        let mut loose = Vec::new();
        let mut parents: Vec<&str> = Vec::new();
        let mut cursor = node.walk();
        'tree: loop {
            let current = cursor.node();
            let kind = current.kind();
            let skipped = spec.skip_kinds.contains(&kind);
            if !skipped && current.child_count() == 0 {
                let text = String::from_utf8_lossy(&source[current.byte_range()]);
                if !text.is_empty() {
                    let exact = hash_bytes(text.as_bytes());
                    let id = Some(current.id());
                    let (strict_token, loose_token) = if id == name_id {
                        (placeholder("name"), placeholder("name"))
                    } else if id == keyword_id {
                        (exact, placeholder("keyword"))
                    } else if spec.identifier_kinds.contains(&kind) {
                        let position = (parents.last().copied().unwrap_or(""), cursor.field_name());
                        let callee = spec
                            .callee_positions
                            .iter()
                            .any(|(parent, field)| position == (*parent, Some(*field)));
                        (
                            exact,
                            if callee {
                                exact
                            } else {
                                placeholder("identifier")
                            },
                        )
                    } else if spec.number_kinds.contains(&kind) {
                        (exact, placeholder("number"))
                    } else if spec.string_kinds.contains(&kind) {
                        (exact, placeholder("string"))
                    } else {
                        (exact, exact)
                    };
                    strict.push(if id == name_id {
                        "FUNC".to_string()
                    } else {
                        text.into_owned()
                    });
                    strict_hashes.push(strict_token);
                    loose.push(loose_token);
                }
            }
            if !skipped && cursor.goto_first_child() {
                parents.push(kind);
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    break 'tree;
                }
                parents.pop();
            }
        }

        if strict.len() < limits.min_tokens {
            return None;
        }
        let shingles = shingles(&loose);
        Some(Unit {
            path: path.to_string(),
            start_line: start_line(&node),
            end_line: end_line(&node),
            name: name(parts.name, source),
            strict,
            strict_hash: hash_sequence(&strict_hashes),
            loose_hash: hash_sequence(&loose),
            signature: minhash(&shingles),
            shingles,
        })
    }
}

fn start_line(node: &Node) -> u32 {
    node.start_position().row as u32 + 1
}

fn end_line(node: &Node) -> u32 {
    node.end_position().row as u32 + 1
}

fn lines(node: &Node) -> u32 {
    end_line(node) - start_line(node) + 1
}

fn name(node: Option<Node>, source: &[u8]) -> String {
    node.map(|node| String::from_utf8_lossy(&source[node.byte_range()]).into_owned())
        .unwrap_or_else(|| "?".to_string())
}
