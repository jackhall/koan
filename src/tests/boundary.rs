//! The import boundary and the storage discipline every rewrite module holds, as a check over a
//! module's own source.
//!
//! A module names only the crate paths its list allows; outside its tests it holds no owning heap
//! type but the ones it lists, so everything it builds rests in program storage or a region; and it
//! spells no lifetime `cellgraph` and `memory` retired. The compiler checks none of that, so a
//! module's boundary test hands its lists here and this reads the files.

use std::path::{Path, PathBuf};

/// Owning heap types a region-resident value may not hold. Each matches as a whole word.
const OWNING: &[&str] = &["Rc", "RefCell", "Box", "Vec", "String"];

/// Lifetime names the rest of the stack retired, so a region borrow reads the same everywhere.
const RETIRED_LIFETIMES: &[char] = &['b', 'r', 'v', 's', 't'];

/// Assert that `src/<module>.rs` and every file under `src/<module>/` names only `prefixes`, holds
/// no owning type outside its tests but the `(file, text)` spans `owning_allowed` blanks, and
/// spells no retired lifetime.
pub(crate) fn holds(module: &str, prefixes: &[&str], owning_allowed: &[(&str, &str)]) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("src").join(module);
    let mut sources = vec![manifest.join("src").join(format!("{module}.rs"))];
    collect_sources(&root, &mut sources);
    assert!(
        sources.len() > 3,
        "the walk found only {} files",
        sources.len()
    );
    let mut offenders = Vec::new();
    for source in &sources {
        let text = std::fs::read_to_string(source).expect("a listed source file reads");
        let name = source.display();
        for path in crate_paths(&text) {
            if path != "crate" && !prefixes.iter().any(|allowed| path.starts_with(allowed)) {
                offenders.push(format!("{name}: names {path}"));
            }
        }
        let mut code = strip_comments_and_strings(&text);
        for (file, allowed) in owning_allowed {
            if source.ends_with(file) {
                code = code.replace(allowed, "");
            }
        }
        let is_test = is_test_source(source, &root);
        if !is_test {
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

/// Whether `source` is one of the module's test files: its own `tests.rs`, or anything under a
/// `tests` directory of it or of one of its submodules.
fn is_test_source(source: &Path, root: &Path) -> bool {
    let Ok(relative) = source.strip_prefix(root) else {
        return false;
    };
    relative
        .components()
        .any(|part| part.as_os_str() == "tests" || part.as_os_str() == "tests.rs")
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
pub(crate) fn strip_comments_and_strings(text: &str) -> String {
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

pub(crate) fn contains_word(code: &str, word: &str) -> bool {
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

/// Every `.rs` file under `directory`, recursively.
pub(crate) fn collect_sources(directory: &Path, out: &mut Vec<PathBuf>) {
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

/// Every place `code`, a pass's rule file, asks which half of its store it runs over: a writer
/// named, a resumption named, or the resumption the store's read half alone reads. A rule lays in
/// the walk's scratch and hands what it fixes to the store, which knows.
pub(crate) fn mode_branches(code: &str) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    let mut found = Vec::new();
    let mut at = 0;
    for token in code.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')) {
        let start = code[at..].find(token).map_or(at, |offset| at + offset);
        at = start + token.len();
        let over = token == "over" && code[..start].ends_with('.');
        if over || ["writer", "Writer", "resume"].contains(&token) {
            let number = code[..start].matches('\n').count();
            found.push(format!("{}: {token}: {}", number + 1, lines[number].trim()));
        }
    }
    found
}

/// Each place `code` reaches a fact past its pass's store, as `line: text`: a call of one of
/// `accessors` on any receiver but `self` — the store's own methods share their names — and a read
/// of one of `fields` through an index into `chain` or a level named `at`, `popped` or `inner`.
pub(crate) fn store_bypasses(code: &str, accessors: &[&str], fields: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    let line_of = |at: usize| code[..at].matches('\n').count();
    let mut flag = |at: usize, what: &str| {
        let number = line_of(at);
        found.push(format!("{}: {what}: {}", number + 1, lines[number].trim()));
    };
    for accessor in accessors {
        let call = format!(".{accessor}(");
        for (at, _) in code.match_indices(&call) {
            // The receiver, across a line break a method chain is wrapped at.
            let receiver: String = code[..at]
                .trim_end()
                .chars()
                .rev()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                .collect();
            if receiver != "fles" {
                flag(at, accessor);
            }
        }
    }
    for field in fields {
        let read = format!(".{field}");
        for (at, _) in code.match_indices(&read) {
            let after = code[at + read.len()..].chars().next();
            if after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '(') {
                continue;
            }
            let before = &code[..at];
            let indexed = before.ends_with(']')
                && before
                    .rfind('[')
                    .is_some_and(|open| before[..open].ends_with("chain"));
            let named = ["at", "popped", "inner"].iter().any(|name| {
                before.ends_with(name)
                    && !before[..before.len() - name.len()]
                        .ends_with(|ch: char| ch.is_alphanumeric() || ch == '_')
            });
            if indexed || named {
                flag(at, field);
            }
        }
    }
    found
}
