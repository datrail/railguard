//! `CONFIGURATION.md` documents every setting the hosts read, and nothing else: each in a section
//! of its own, and each retired one in the table instead. Only names are checked, not what's said.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

// What `finish` is given to warn of unread variables: a prefix, not a setting.
const PREFIX: &str = "RAIL_";
const RETIRED_HEADING: &str = "## Retired names";
// Names settings only to test them, so it reads none.
const NOT_A_HOST: &str = "rail-test-support";

fn get_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// Every `.rs` file under each host crate's `src`, so tests' made-up names aren't counted.
fn read_sources() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(get_root().join("crates")).unwrap() {
        let crate_dir = entry.unwrap().path();
        let src = crate_dir.join("src");
        if src.is_dir() && !crate_dir.ends_with(NOT_A_HOST) {
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

// Whole-line comments dropped, so commented-out code neither reads nor retires a name.
fn strip_comment_lines(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Each `"RAIL_…"` literal, the bare prefix aside. A name built from parts would go unseen, so one
// is refused.
fn find_names(path: &Path, source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let quoted = format!("\"{PREFIX}");
    assert!(
        !source.contains(&format!("concat!({quoted}")),
        "{}: a setting's name is built with concat!; write it whole",
        path.display()
    );
    for (start, _) in source.match_indices(&quoted) {
        let rest = &source[start + 1..];
        let end = rest
            .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        assert!(
            rest[end..].starts_with('"'),
            "{}: {:?} isn't a whole setting name; write it whole",
            path.display(),
            &rest[..end + 1]
        );
        if end > PREFIX.len() {
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
        // rustfmt adds a trailing comma when it puts the argument on a line of its own.
        let argument = compact[start..end].trim_end_matches(',');
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
    ["&str", "&'staticstr"].into_iter().find_map(|kind| {
        let definition = format!("const{name}:{kind}=\"");
        let start = compact.find(&definition)? + definition.len();
        let end = start + compact[start..].find('"')?;
        Some(compact[start..end].to_owned())
    })
}

// The names read, and those of them retired.
fn read_settings() -> (BTreeSet<String>, BTreeSet<String>) {
    let mut read = BTreeSet::new();
    let mut retired = BTreeSet::new();
    for (path, source) in read_sources() {
        let source = strip_comment_lines(&source);
        read.extend(find_names(&path, &source));
        retired.extend(find_retired(&path, &source));
    }
    (read, retired)
}

// Each setting heading's name, as often as it's given, and each retired table row's; code
// blocks skipped.
fn read_doc(doc: &str) -> (Vec<String>, Vec<String>) {
    let mut sections = Vec::new();
    let mut table = Vec::new();
    let mut in_retired = false;
    let mut in_code = false;
    for line in doc.lines() {
        if line.starts_with("```") {
            in_code = !in_code;
        }
        if in_code {
            continue;
        }
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

// Every way `doc` disagrees with the names read and retired, one line each.
fn check_doc(read: &BTreeSet<String>, retired: &BTreeSet<String>, doc: &str) -> Vec<String> {
    let (sections, table) = read_doc(doc);
    let current: BTreeSet<&String> = read.difference(retired).collect();
    let retired: BTreeSet<&String> = retired.iter().collect();
    let mut problems = Vec::new();
    for (kind, names, expected) in [
        ("section", &sections, &current),
        ("retired table row", &table, &retired),
    ] {
        let mut seen = BTreeSet::new();
        for name in names {
            if !seen.insert(name) {
                problems.push(format!("{name}: more than one {kind}"));
            }
        }
        for name in expected.difference(&seen) {
            problems.push(format!("{name}: no {kind}"));
        }
        for name in seen.difference(expected) {
            problems.push(format!(
                "{name}: a {kind}, but not read as one in crates/*/src"
            ));
        }
    }
    problems
}

#[test]
fn configuration_md_has_a_section_per_setting_and_a_row_per_retired_one() {
    let (read, retired) = read_settings();
    let doc = fs::read_to_string(get_root().join("CONFIGURATION.md")).unwrap();
    let problems = check_doc(&read, &retired, &doc);
    assert!(
        problems.is_empty(),
        "CONFIGURATION.md:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_check_finds_each_way_the_doc_can_be_wrong() {
    let read = BTreeSet::from(["RAIL_A".to_owned(), "RAIL_OLD".to_owned()]);
    let retired = BTreeSet::from(["RAIL_OLD".to_owned()]);
    let good = "## H\n### `RAIL_A`\n```\n### `RAIL_X`\n```\n## Retired names\n| `RAIL_OLD` | x |\n";
    assert_eq!(check_doc(&read, &retired, good), Vec::<String>::new());
    for (doc, problem) in [
        (
            "## H\n## Retired names\n| `RAIL_OLD` |\n",
            "RAIL_A: no section",
        ),
        (
            "## H\n```\n### `RAIL_A`\n```\n## Retired names\n| `RAIL_OLD` |\n",
            "RAIL_A: no section",
        ),
        (
            "## H\n### `RAIL_A`\n### `RAIL_A`\n## Retired names\n| `RAIL_OLD` |\n",
            "RAIL_A: more than one section",
        ),
        (
            "## H\n### `RAIL_A`\n### `RAIL_B`\n## Retired names\n| `RAIL_OLD` |\n",
            "RAIL_B: a section, but not read as one in crates/*/src",
        ),
        (
            "## H\n### `RAIL_A`\n### `RAIL_OLD`\n## Retired names\n| `RAIL_OLD` |\n",
            "RAIL_OLD: a section, but not read as one in crates/*/src",
        ),
        (
            "## H\n### `RAIL_A`\n## Retired names\n",
            "RAIL_OLD: no retired table row",
        ),
        (
            "## H\n### `RAIL_A`\n## Retired names\n| `RAIL_OLD` |\n| `RAIL_OLD` |\n",
            "RAIL_OLD: more than one retired table row",
        ),
        (
            "## H\n### `RAIL_A`\n## Retired names\n| `RAIL_OLD` |\n| `RAIL_A` |\n",
            "RAIL_A: a retired table row, but not read as one in crates/*/src",
        ),
        (
            "## H\n### `RAIL_A`\n| `RAIL_OLD` |\n## Retired names\n",
            "RAIL_OLD: no retired table row",
        ),
        (
            "## H\n### `RAIL_A`\n## Retired names\n## Notes\n| `RAIL_OLD` |\n",
            "RAIL_OLD: no retired table row",
        ),
        (
            "## H\n### `RAIL_A`\n## Retired names (old)\n| `RAIL_OLD` |\n",
            "RAIL_OLD: no retired table row",
        ),
    ] {
        assert_eq!(check_doc(&read, &retired, doc), [problem], "{doc}");
    }
}

#[test]
fn the_scan_finds_literals_and_constants_and_skips_the_bare_prefix_and_comments() {
    let source = strip_comment_lines(
        r#"
        const RETIRED: &str = "RAIL_OLD";
        const NEW: &str = "RAIL_NEW_2";
        const LONG: &'static str = "RAIL_LONG";
        settings.get(RETIRED).retired("x");
        settings.get(LONG).retired("x");
        settings
            .get("RAIL_GONE")
            .retired(&instead);
        settings
            .get(
                "RAIL_WRAPPED",
            )
            .retired(
                "x",
            );
        // settings.get("RAIL_COMMENTED").retired("x");
        settings.finish("RAIL_", config);
    "#,
    );
    let path = Path::new("example.rs");
    let names: Vec<_> = find_names(path, &source).into_iter().collect();
    assert_eq!(
        names,
        [
            "RAIL_GONE",
            "RAIL_LONG",
            "RAIL_NEW_2",
            "RAIL_OLD",
            "RAIL_WRAPPED"
        ]
    );
    let retired: Vec<_> = find_retired(path, &source).into_iter().collect();
    assert_eq!(
        retired,
        ["RAIL_GONE", "RAIL_LONG", "RAIL_OLD", "RAIL_WRAPPED"]
    );
}

#[test]
#[should_panic(expected = "isn't a whole setting name")]
fn a_name_built_with_format_is_refused() {
    find_names(Path::new("example.rs"), r#"format!("RAIL_{suffix}")"#);
}

#[test]
#[should_panic(expected = "built with concat!")]
fn a_name_built_with_concat_is_refused() {
    find_names(Path::new("example.rs"), r#"concat!("RAIL_", "NEW")"#);
}

#[test]
fn the_sources_are_read_from_nested_directories() {
    let dir = std::env::temp_dir().join(format!("configuration-doc-{}", std::process::id()));
    fs::create_dir_all(dir.join("nested")).unwrap();
    fs::write(dir.join("lib.rs"), "").unwrap();
    fs::write(dir.join("nested/mod.rs"), "").unwrap();
    fs::write(dir.join("nested/notes.md"), "").unwrap();
    let mut files = Vec::new();
    collect_rust_files(&dir, &mut files);
    fs::remove_dir_all(&dir).unwrap();
    files.sort();
    assert_eq!(files, [dir.join("lib.rs"), dir.join("nested/mod.rs")]);
}
