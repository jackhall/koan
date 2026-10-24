//! The import boundary and the storage discipline, as a test over this module's own source.
//!
//! `program` may name the layers it stands over — `build`, `elaborate`, `knot`, `memory`, `parse`,
//! `scheduler`, `scope`, `symbols`, `type_lattice` and `values`. Outside its tests it holds no
//! owning heap type but a refused load's rendered shape error, reported once and never stored.

/// The path prefixes `program` may name.
const PREFIXES: &[&str] = &[
    "crate::build",
    "crate::elaborate",
    "crate::knot",
    "crate::memory",
    "crate::parse",
    "crate::program",
    "crate::scheduler",
    "crate::scope",
    "crate::symbols",
    "crate::tests::boundary",
    "crate::type_lattice",
    "crate::values",
];

/// The one owning type outside the tests: a refused load's rendered shape error.
const OWNING_ALLOWED: &[(&str, &str)] = &[("src/program/record.rs", "rendered: String")];

#[test]
fn program_names_only_the_layers_below_it_and_holds_no_heap() {
    crate::tests::boundary::holds("program", PREFIXES, OWNING_ALLOWED);
}

/// Each holder a cell parks or a value keeps, beside the file that defines it.
const PARKED: &[(&str, &str)] = &[
    ("src/program/bundle.rs", "KBirth"),
    ("src/program/bundle.rs", "KState"),
    ("src/program/body.rs", "Runner"),
    ("src/scope/activation.rs", "ActivationView"),
    ("src/knot/function.rs", "Function"),
    ("src/knot/code.rs", "Code"),
    ("src/knot/module.rs", "Content"),
];

/// What a holder that crosses a park must not name: a shape or a load's facts, which a step reads
/// at its own lifetime through its door. It names a segment, a site or a digest instead.
const STEP_READS: &[&str] = &["BodyShape", "LoadFacts", "Body", "Facts", "Shapes"];

/// The text of the definition of the struct or enum `name` in `code`: from its keyword to the brace
/// that closes it.
fn definition<'c>(code: &'c str, name: &str) -> &'c str {
    let start = ["struct", "enum"]
        .iter()
        .find_map(|keyword| {
            let head = format!("{keyword} {name}");
            code.match_indices(&head)
                .map(|(at, _)| at)
                .find(|at| !code[at + head.len()..].starts_with(|ch: char| ch.is_alphanumeric()))
        })
        .unwrap_or_else(|| panic!("`{name}` is defined"));
    let open = start + code[start..].find('{').expect("a definition has a body");
    let mut depth = 0;
    for (at, ch) in code[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &code[start..open + at + 1];
                }
            }
            _ => {}
        }
    }
    panic!("`{name}`'s definition closes")
}

#[test]
fn nothing_a_cell_parks_names_a_shape_or_the_loads_facts() {
    use crate::tests::boundary::{contains_word, strip_comments_and_strings};
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    for (file, name) in PARKED {
        let source = std::fs::read_to_string(manifest.join(file)).expect("the source reads");
        let code = strip_comments_and_strings(&source);
        let definition = definition(&code, name);
        for read in STEP_READS {
            if contains_word(definition, read) {
                offenders.push(format!("{file}: `{name}` names `{read}`"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a parked holder names what a step reads:\n{}",
        offenders.join("\n")
    );
}
