//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `values` may name `memory`, `parse`, `source` and `type_lattice` and nothing else in the crate;
//! outside its tests it holds no owning heap type, so everything it builds rests in a region; and
//! it spells no lifetime `cellgraph` and `memory` retired. The compiler checks none of that, so this
//! test reads the files.

use std::path::{Path, PathBuf};

/// The path prefixes `values` may name.
const PREFIXES: &[&str] = &[
    "crate::memory",
    "crate::parse",
    "crate::source",
    "crate::type_lattice",
    "crate::values",
];

/// Owning heap types a region-resident value may not hold. Each matches as a whole word.
const OWNING: &[&str] = &["Rc", "RefCell", "Box", "Vec", "String"];

/// Lifetime names the rest of the stack retired, so a region borrow reads the same everywhere.
const RETIRED_LIFETIMES: &[char] = &['b', 'r', 'v', 's', 't'];

#[test]
fn values_names_only_its_four_modules_holds_no_heap_and_spells_the_stack_lifetimes() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = vec![manifest.join("src/values.rs")];
    collect_sources(&manifest.join("src/values"), &mut sources);
    assert!(
        sources.len() > 10,
        "the walk found only {} files",
        sources.len()
    );
    let mut offenders = Vec::new();
    for source in &sources {
        let text = std::fs::read_to_string(source).expect("a listed source file reads");
        let name = source.display();
        for path in crate_paths(&text) {
            if path != "crate" && !PREFIXES.iter().any(|allowed| path.starts_with(allowed)) {
                offenders.push(format!("{name}: names {path}"));
            }
        }
        let code = strip_comments_and_strings(&text);
        if !is_test_source(&manifest, source) {
            for word in OWNING {
                if contains_word(&code, word) {
                    offenders.push(format!("{name}: holds {word}"));
                }
            }
        }
        for lifetime in RETIRED_LIFETIMES {
            if spells_lifetime(&code, *lifetime) {
                offenders.push(format!("{name}: spells '{lifetime}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "boundary violations:\n{}",
        offenders.join("\n")
    );
}

/// Whether `path` is a test file: the suite root, or anything under the module's own `tests/`.
fn is_test_source(manifest: &Path, path: &Path) -> bool {
    let module = manifest.join("src/values");
    path == module.join("tests.rs") || path.starts_with(module.join("tests"))
}

/// Every `crate::…` path the text names, brace groups expanded. Doc comments count: an intra-doc
/// link naming a forbidden item is a real edge the moment it becomes code.
fn crate_paths(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find("crate::") {
        let start = at + offset;
        let boundary = start == 0 || !is_path_char(bytes[start - 1]);
        let mut end = start + "crate::".len();
        while end < bytes.len() && is_path_char(bytes[end]) {
            end += 1;
        }
        if boundary {
            let base = &text[start..end];
            if bytes.get(end) == Some(&b'{') {
                let close = text[end..].find('}').map_or(end, |offset| end + offset);
                for item in text[end + 1..close].split(',') {
                    let item = item.trim().trim_end_matches("::*");
                    if !item.is_empty() {
                        found.push(format!("{base}{item}"));
                    }
                }
            } else {
                found.push(base.trim_end_matches("::").to_string());
            }
        }
        at = end;
    }
    found
}

/// The text with every `//` comment and every string literal blanked, so prose and messages never
/// read as code.
fn strip_comments_and_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut in_string = false;
        let mut previous = '\0';
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if in_string {
                if ch == '"' && previous != '\\' {
                    in_string = false;
                }
                previous = if previous == '\\' { '\0' } else { ch };
                continue;
            }
            if ch == '/' && chars.peek() == Some(&'/') {
                break;
            }
            if ch == '"' {
                in_string = true;
                previous = '\0';
                continue;
            }
            out.push(ch);
        }
        out.push('\n');
    }
    out
}

fn contains_word(code: &str, word: &str) -> bool {
    code.match_indices(word).any(|(at, _)| {
        let before = code[..at].chars().next_back();
        let after = code[at + word.len()..].chars().next();
        !before.is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == ':')
            && !after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
    })
}

/// Whether `'x` appears as a lifetime: a quote, the letter, then no identifier character and no
/// closing quote (which would make it a character literal).
fn spells_lifetime(code: &str, letter: char) -> bool {
    let pattern = format!("'{letter}");
    code.match_indices(&pattern).any(|(at, _)| {
        let after = code[at + pattern.len()..].chars().next();
        !after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '\'')
    })
}

fn is_path_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b':'
}

fn collect_sources(directory: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory).expect("the source directory reads");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}
