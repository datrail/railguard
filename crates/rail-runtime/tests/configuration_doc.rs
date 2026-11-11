//! `CONFIGURATION.md` documents every setting the crates read, and nothing else: each in a section
//! of its own, and each retired one in the table instead.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

// What `finish` is given to warn of unread variables: a prefix, not a setting.
const PREFIX: &str = "RAIL_";
const RETIRED_HEADING: &str = "## Retired names";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// Every `.rs` file under each crate's `src`, so tests' made-up names aren't counted.
fn read_sources() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root().join("crates")).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            collect_rust_files(&src, &mut files);
        }
    }
    assert!(!files.is_empty(), "no source found under crates/*/src");
    files
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).unwrap();
            (path, text)
        })
        .collect()
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

// Each `"RAIL_…"` literal holding a whole name, the bare prefix aside.
fn find_names(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let quoted = format!("\"{PREFIX}");
    for (start, _) in source.match_indices(&quoted) {
        let rest = &source[start + 1..];
        let end = rest
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        if rest[end..].starts_with('"') && end > PREFIX.len() {
            names.insert(rest[..end].to_owned());
        }
    }
    names
}

// The names given to `.get(…).retired(…)`, as a literal or as a constant the same file defines.
fn find_retired(path: &Path, source: &str) -> BTreeSet<String> {
    let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    let mut retired = BTreeSet::new();
    for (end, _) in compact.match_indices(").retired(") {
        let start = compact[..end]
            .rfind(".get(")
            .unwrap_or_else(|| panic!("{}: a retired name not read with get", path.display()))
            + ".get(".len();
        let argument = &compact[start..end];
        let name = match argument
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            Some(name) => name.to_owned(),
            None => resolve_constant(&compact, argument).unwrap_or_else(|| {
                panic!(
                    "{}: retired name {argument} is neither a literal nor a constant defined here",
                    path.display()
                )
            }),
        };
        retired.insert(name);
    }
    retired
}

fn resolve_constant(compact: &str, name: &str) -> Option<String> {
    let definition = format!("const{name}:&str=\"");
    let start = compact.find(&definition)? + definition.len();
    let end = start + compact[start..].find('"')?;
    Some(compact[start..end].to_owned())
}

// The names read, and those of them retired.
fn read_settings() -> (BTreeSet<String>, BTreeSet<String>) {
    let mut read = BTreeSet::new();
    let mut retired = BTreeSet::new();
    for (path, source) in read_sources() {
        read.extend(find_names(&source));
        retired.extend(find_retired(&path, &source));
    }
    (read, retired)
}

// Each `### \`RAIL_…\`` heading's name, as often as it's given, and each retired table row's.
fn read_doc() -> (Vec<String>, Vec<String>) {
    let doc = fs::read_to_string(root().join("CONFIGURATION.md")).unwrap();
    let mut sections = Vec::new();
    let mut table = Vec::new();
    let mut in_retired = false;
    for line in doc.lines() {
        if line.starts_with("## ") {
            in_retired = line == RETIRED_HEADING;
        }
        if let Some(name) = line
            .strip_prefix("### `")
            .and_then(|rest| rest.strip_suffix('`'))
        {
            sections.push(name.to_owned());
        }
        if in_retired
            && let Some(rest) = line.strip_prefix("| `")
            && let Some((name, _)) = rest.split_once('`')
            && name.starts_with(PREFIX)
        {
            table.push(name.to_owned());
        }
    }
    (sections, table)
}

fn find_duplicates(names: &[String]) -> Vec<&String> {
    let mut seen = BTreeSet::new();
    names.iter().filter(|name| !seen.insert(*name)).collect()
}

#[test]
fn every_setting_read_has_one_section_and_every_section_a_setting_read() {
    let (read, retired) = read_settings();
    let current: BTreeSet<String> = read.difference(&retired).cloned().collect();
    let (sections, _) = read_doc();
    assert_eq!(
        find_duplicates(&sections),
        Vec::<&String>::new(),
        "documented twice"
    );
    let documented: BTreeSet<String> = sections.into_iter().collect();
    let undocumented: Vec<_> = current.difference(&documented).collect();
    let unread: Vec<_> = documented.difference(&current).collect();
    assert!(
        undocumented.is_empty() && unread.is_empty(),
        "read but with no section in CONFIGURATION.md: {undocumented:?}; with a section but not \
         read in crates/*/src, or retired: {unread:?}"
    );
}

#[test]
fn every_retired_name_is_in_the_table_and_only_there() {
    let (_, retired) = read_settings();
    let (_, table) = read_doc();
    assert_eq!(
        find_duplicates(&table),
        Vec::<&String>::new(),
        "listed twice"
    );
    let listed: BTreeSet<String> = table.into_iter().collect();
    let unlisted: Vec<_> = retired.difference(&listed).collect();
    let not_retired: Vec<_> = listed.difference(&retired).collect();
    assert!(
        unlisted.is_empty() && not_retired.is_empty(),
        "retired but not in CONFIGURATION.md's table: {unlisted:?}; in the table but not retired: \
         {not_retired:?}"
    );
}

#[test]
fn the_scan_finds_literals_and_constants_and_skips_the_bare_prefix() {
    let source = r#"
        const RETIRED: &str = "RAIL_OLD";
        const NEW: &str = "RAIL_NEW_2";
        settings.get(RETIRED).retired("x");
        settings
            .get("RAIL_GONE")
            .retired(&instead);
        settings.finish("RAIL_", config);
        let not_a_name = "RAIL_X=true";
    "#;
    let names: Vec<_> = find_names(source).into_iter().collect();
    assert_eq!(names, ["RAIL_GONE", "RAIL_NEW_2", "RAIL_OLD"]);
    let retired: Vec<_> = find_retired(Path::new("example.rs"), source)
        .into_iter()
        .collect();
    assert_eq!(retired, ["RAIL_GONE", "RAIL_OLD"]);
}
