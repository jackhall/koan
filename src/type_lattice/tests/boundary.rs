//! The import boundary, as a test over this module's own source.
//!
//! The lattice's only `crate::` edges are the label and symbol types from `parse` and `ScopeId`
//! from `memory`. The compiler cannot enforce that — a public module may name anything in its own
//! crate — so the rule is checked here, by reading the files.

use std::path::{Path, PathBuf};

/// The `crate::parse` items the lattice may name: the classified symbol types, the interner and its
/// display view, the static-name memo, and the identity hashers the registry's tables key with.
const PARSE_ITEMS: &[&str] = &[
    "BinderSymbol",
    "ClassifiedSymbol",
    "IdentityBuildHasher",
    "IdentityHasher",
    "KeywordSymbol",
    "LabelDisplay",
    "LabelInterner",
    "StaticName",
    "Symbol",
    "TypeSymbol",
    "ValueSymbol",
];

/// The path prefixes the lattice may name outside `parse`: `ScopeId` and its associated items, the
/// macro that mints a static name, and its own module path.
const PREFIXES: &[&str] = &[
    "crate::memory::ScopeId",
    "crate::static_name",
    "crate::type_lattice",
];

/// The two module names the lattice may write bare — in a doc link naming the module itself, and in
/// this file's own allowlist. Naming a module is not an edge to anything inside it: an item under
/// one still has to clear [`permitted`].
const MODULES: &[&str] = &["crate", "crate::parse", "crate::memory"];

#[test]
fn the_core_names_only_labels_and_scope_ids() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/type_lattice");
    let mut sources = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/type_lattice.rs")];
    collect_sources(&root, &mut sources);
    assert!(
        sources.len() > 10,
        "the walk found only {} files under {}",
        sources.len(),
        root.display()
    );
    let mut offenders: Vec<String> = Vec::new();
    for source in &sources {
        let text = std::fs::read_to_string(source).expect("a listed source file reads");
        for path in crate_paths(&text) {
            if !permitted(&path) {
                offenders.push(format!("{}: {path}", source.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the type lattice may name only the label types and `ScopeId`, but found:\n{}",
        offenders.join("\n")
    );
}

/// Whether one extracted path is on the allowlist. An item *of* an allowed item — an associated
/// constant, a method — rides on its prefix.
fn permitted(path: &str) -> bool {
    if MODULES.contains(&path) {
        return true;
    }
    if PREFIXES.iter().any(|allowed| path.starts_with(allowed)) {
        return true;
    }
    match path.strip_prefix("crate::parse::") {
        Some(rest) => PARSE_ITEMS.contains(&rest.split("::").next().unwrap_or(rest)),
        None => false,
    }
}

/// Every `crate::…` path the text names, brace groups expanded into one path per item. Doc comments
/// count: an intra-doc link is a real path, and one naming a forbidden item would be a real edge to
/// it the moment someone turned it into code.
fn crate_paths(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = text[at..].find("crate::") {
        let start = at + start;
        // A `crate::` that continues an identifier (`my_crate::`) is not the crate root.
        let boundary = start == 0 || !is_path_char(bytes[start - 1]);
        let mut end = start + "crate::".len();
        while end < bytes.len() && is_path_char(bytes[end]) {
            end += 1;
        }
        if boundary {
            let base = &text[start..end];
            match bytes.get(end) {
                Some(b'{') => {
                    let close = text[end..]
                        .find('}')
                        .map(|offset| end + offset)
                        .unwrap_or(end);
                    for item in text[end + 1..close].split(',') {
                        let item = item.trim().trim_end_matches("::*");
                        if !item.is_empty() {
                            found.push(format!("{base}{item}"));
                        }
                    }
                }
                _ => found.push(base.trim_end_matches("::").to_string()),
            }
        }
        at = end;
    }
    found
}

fn is_path_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b':'
}

fn collect_sources(directory: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory).expect("the lattice's source directory reads");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}
