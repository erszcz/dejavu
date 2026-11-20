//! The concept signal: a noun that two or more areas each manage through their public functions.
//!
//! When one area has its own implementation of a thing that another area owns, the two share no
//! code and no calls. Their public functions still name the same thing: `create_folder` in one
//! and `create_journal_folder` in the other. A public function name is read as a verb and a
//! noun, and a noun is compared by its last word.
//!
//! A finding says that two areas each own a concept with the same name. It does not say that one
//! of them is a mistake.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::report::Place;
use crate::unit::ApiFunction;

const VERBS: &[&str] = &[
    "create",
    "update",
    "delete",
    "remove",
    "get",
    "fetch",
    "list",
    "move",
    "rename",
    "archive",
    "restore",
    "duplicate",
    "copy",
];
/// An area manages a noun when it has this many verbs on it.
const MIN_VERBS: usize = 3;
/// A finding needs this many verbs that two or more areas both have.
const MIN_SHARED_VERBS: usize = 2;

#[derive(Debug, PartialEq, Serialize)]
pub struct Finding {
    /// The last word of the nouns, for example `folder`.
    pub noun: String,
    /// The verbs that two or more of the areas have on the noun.
    pub shared_verbs: Vec<String>,
    pub contexts: Vec<Context>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Context {
    /// The directory of the public entry file.
    pub area: String,
    /// The full noun in this area, for example `journal_folder`.
    pub noun: String,
    pub verbs: Vec<String>,
    pub members: Vec<Place>,
}

/// Findings with the most shared verbs first. With `changed`, only findings that have a changed
/// function are kept.
pub fn findings(api: &[ApiFunction], changed: Option<&[bool]>) -> Vec<Finding> {
    // last word of the noun -> (area, noun) -> verb -> function
    type Entries<'a> = BTreeMap<(&'a str, String), BTreeMap<String, usize>>;
    let mut by_word: BTreeMap<String, Entries> = BTreeMap::new();
    for (index, function) in api.iter().enumerate() {
        if let Some((verb, noun)) = verb_and_noun(&function.name) {
            let word = noun.rsplit('_').next().unwrap_or(&noun).to_string();
            let area = function.path.rsplit_once('/').map_or("", |(area, _)| area);
            by_word
                .entry(word)
                .or_default()
                .entry((area, noun))
                .or_default()
                .entry(verb)
                .or_insert(index);
        }
    }

    let mut found: Vec<Finding> = by_word
        .into_iter()
        .filter_map(|(word, entries)| {
            let entries: Vec<_> = entries
                .into_iter()
                .filter(|(_, verbs)| verbs.len() >= MIN_VERBS)
                .collect();
            let mut areas_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for ((area, _), verbs) in &entries {
                for verb in verbs.keys() {
                    areas_of.entry(verb).or_default().insert(area);
                }
            }
            let shared_verbs: Vec<String> = areas_of
                .iter()
                .filter(|(_, areas)| areas.len() >= 2)
                .map(|(verb, _)| verb.to_string())
                .collect();
            let is_changed = |index: usize| changed.is_some_and(|changed| changed[index]);
            let any_changed = entries
                .iter()
                .any(|(_, verbs)| verbs.values().any(|index| is_changed(*index)));
            if shared_verbs.len() < MIN_SHARED_VERBS || (changed.is_some() && !any_changed) {
                return None;
            }
            let contexts = entries
                .iter()
                .map(|((area, noun), verbs)| Context {
                    area: area.to_string(),
                    noun: noun.clone(),
                    verbs: verbs.keys().cloned().collect(),
                    members: verbs
                        .values()
                        .map(|index| {
                            let function = &api[*index];
                            Place {
                                path: function.path.clone(),
                                start_line: function.start_line,
                                end_line: function.end_line,
                                name: function.name.clone(),
                                changed: is_changed(*index),
                            }
                        })
                        .collect(),
                })
                .collect();
            Some(Finding {
                noun: word,
                shared_verbs,
                contexts,
            })
        })
        .collect();
    found.sort_by(|a, b| {
        (b.shared_verbs.len(), b.contexts.len(), &a.noun).cmp(&(
            a.shared_verbs.len(),
            a.contexts.len(),
            &b.noun,
        ))
    });
    found
}

/// `create_journal_folders` gives `("create", "journal_folder")`. The name can be in snake case
/// or in camel case. A name with an unknown first word, or with no noun, gives nothing.
fn verb_and_noun(name: &str) -> Option<(String, String)> {
    let mut words = words(name);
    if words.len() < 2 || !VERBS.contains(&words[0].as_str()) {
        return None;
    }
    let verb = words.remove(0);
    let last = words.pop()?;
    words.push(singular(&last));
    Some((verb, words.join("_")))
}

fn words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut previous_lower = false;
    for character in name.chars() {
        let boundary = character == '_' || (character.is_uppercase() && previous_lower);
        if boundary && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        if character.is_alphanumeric() {
            word.extend(character.to_lowercase());
        }
        previous_lower = character.is_lowercase() || character.is_ascii_digit();
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn singular(word: &str) -> String {
    if let Some(stem) = word.strip_suffix("ies") {
        format!("{stem}y")
    } else if ["ss", "us", "is"].iter().any(|end| word.ends_with(end)) {
        word.to_string()
    } else {
        word.strip_suffix('s').unwrap_or(word).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(functions: &[(&str, &str)]) -> Vec<ApiFunction> {
        functions
            .iter()
            .enumerate()
            .map(|(index, (path, name))| ApiFunction {
                path: path.to_string(),
                start_line: index as u32 + 1,
                end_line: index as u32 + 1,
                name: name.to_string(),
            })
            .collect()
    }

    const FOLDERS: &str = "lib/app/folders/folders.ex";
    const JOURNALS: &str = "lib/app/journals/journals.ex";

    fn two_folder_owners() -> Vec<ApiFunction> {
        api(&[
            (FOLDERS, "create_folder"),
            (FOLDERS, "list_folders"),
            (FOLDERS, "delete_folder"),
            (FOLDERS, "get_folder_tree"),
            (JOURNALS, "create_journal_folder"),
            (JOURNALS, "list_journal_folders"),
            (JOURNALS, "delete_journal_folder"),
            (JOURNALS, "create_journal"),
            (JOURNALS, "run_journal"),
        ])
    }

    #[test]
    fn a_name_is_a_verb_and_a_singular_noun() {
        let split = |name| verb_and_noun(name).map(|(verb, noun)| format!("{verb} {noun}"));
        assert_eq!(split("create_folder").as_deref(), Some("create folder"));
        assert_eq!(
            split("list_journal_folders").as_deref(),
            Some("list journal_folder")
        );
        assert_eq!(split("get_policies").as_deref(), Some("get policy"));
        assert_eq!(split("get_status").as_deref(), Some("get status"));
        assert_eq!(split("delete_folder!").as_deref(), Some("delete folder"));
        assert_eq!(
            split("createJournalFolder").as_deref(),
            Some("create journal_folder")
        );
        assert_eq!(split("run_journal"), None);
        assert_eq!(split("list"), None);
    }

    #[test]
    fn two_areas_that_manage_one_noun_are_a_finding() {
        let found = findings(&two_folder_owners(), None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].noun, "folder");
        assert_eq!(found[0].shared_verbs, ["create", "delete", "list"]);
        let contexts: Vec<_> = found[0]
            .contexts
            .iter()
            .map(|c| (c.area.as_str(), c.noun.as_str(), c.verbs.join(" ")))
            .collect();
        assert_eq!(
            contexts,
            [
                (
                    "lib/app/folders",
                    "folder",
                    "create delete list".to_string()
                ),
                (
                    "lib/app/journals",
                    "journal_folder",
                    "create delete list".to_string()
                ),
            ]
        );
        let names: Vec<_> = found[0].contexts[1]
            .members
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "create_journal_folder",
                "delete_journal_folder",
                "list_journal_folders"
            ]
        );
    }

    #[test]
    fn an_area_with_fewer_than_three_verbs_does_not_manage_the_noun() {
        let mut functions = two_folder_owners();
        functions.retain(|function| function.name != "delete_journal_folder");
        assert_eq!(findings(&functions, None), []);
    }

    #[test]
    fn one_area_is_not_a_finding() {
        let mut functions = two_folder_owners();
        functions.retain(|function| function.path == FOLDERS);
        assert_eq!(findings(&functions, None), []);
    }

    #[test]
    fn the_areas_must_share_two_verbs() {
        let functions = api(&[
            (FOLDERS, "create_folder"),
            (FOLDERS, "list_folders"),
            (FOLDERS, "delete_folder"),
            (JOURNALS, "create_journal_folder"),
            (JOURNALS, "move_journal_folder"),
            (JOURNALS, "rename_journal_folder"),
        ]);
        assert_eq!(findings(&functions, None), []);
    }

    #[test]
    fn with_changes_only_a_finding_with_a_changed_function_is_kept() {
        let functions = two_folder_owners();
        let mut changed = vec![false; functions.len()];
        changed[7] = true;
        assert_eq!(findings(&functions, Some(&changed)), []);
        changed[4] = true;
        let found = findings(&functions, Some(&changed));
        let flags: Vec<_> = found[0].contexts[1]
            .members
            .iter()
            .map(|m| m.changed)
            .collect();
        assert_eq!(flags, [true, false, false]);
    }
}
