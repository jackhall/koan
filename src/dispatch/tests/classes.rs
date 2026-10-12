//! Body classes: the class the load gives each frame body — sealed or porous by its exits — and the
//! minting sites it counts, the blocks and module bodies it runs among them.

use crate::program::Program;
use crate::scope::{BodyClass, ShapeKind};

use super::run;
use super::statics::{body, loaded, module_body};

/// `SIG Counter`, whose head parameter an opaque view hides, and `MODULE ints`, which fits it.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n";

/// A class, rendered: `sealed` or `porous`, its minting sites, and `lays` where a sealed body lays
/// a shape branch.
fn rendered(class: Option<BodyClass>) -> String {
    match class.expect("a frame body has a class") {
        BodyClass::Sealed { mints, lays_branch } => {
            format!("sealed {mints}{}", if lays_branch { " lays" } else { "" })
        }
        BodyClass::Porous { mints } => format!("porous {mints}"),
    }
}

/// The class of the body the top-level binder `name` of `COUNTER` and `source` births.
fn class(source: &str, name: &str) -> String {
    loaded(&format!("{COUNTER}{source}"), |program| {
        rendered(body(program, program.shape(), name).class())
    })
}

/// The class of the one quote's code nested in the top level of `COUNTER` and `source`.
fn code_class(source: &str) -> String {
    loaded(&format!("{COUNTER}{source}"), |program: &Program<'_>| {
        let (_, code) = (program.shape().nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Code)
            .expect("the program writes a quote");
        rendered(code.class())
    })
}

#[test]
fn a_body_returning_no_view_is_sealed_and_lays_a_branch_where_it_shares() {
    let body = "(LET v = (ints :| Counter)) (c)";
    assert_eq!(
        class(
            &format!("LET f = (FN :{{c :Number}} -> Any = #({body}))"),
            "f"
        ),
        "sealed 1 lays",
        "the return is porous, the value returned is not"
    );
    assert_eq!(
        class(
            &format!("LET f = (FN :{{c :Number}} -> Number = #({body}))"),
            "f"
        ),
        "sealed 1",
        "a body placed fresh lays no branch"
    );
}

#[test]
fn a_body_returning_its_view_is_porous() {
    assert_eq!(
        class(
            "LET f = (FN :{c :Number} -> Any = #((LET v = (ints :| Counter)) (v)))",
            "f"
        ),
        "porous 1"
    );
}

#[test]
fn a_call_into_a_frame_at_a_porous_slot_is_an_exit_and_a_builtin_is_none() {
    let keep = "EXPR #(KEEP x :Any) -> Number = #(1)\n";
    assert_eq!(
        class(
            &format!(
                "{keep}LET f = (FN :{{c :Number}} -> Any = \
                 #((LET v = (ints :| Counter)) (KEEP v) (c)))"
            ),
            "f"
        ),
        "porous 1"
    );
    assert_eq!(
        class(
            "LET f = (FN :{c :Number} -> Any = #((LET v = (ints :| Counter)) (PRINT v) (c)))",
            "f"
        ),
        "sealed 1 lays",
        "a builtin's native enters no frame"
    );
    assert_eq!(
        class(
            &format!(
                "{keep}LET f = (FN :{{c :Number}} -> Any = \
                 #((LET v = (ints :| Counter)) (KEEP c) (c)))"
            ),
            "f"
        ),
        "sealed 1 lays",
        "a value that is not porous carries nothing out"
    );
}

#[test]
fn a_call_by_name_is_an_exit_where_a_porous_value_meets_a_porous_slot() {
    let view = "(LET v = (ints :| Counter))";
    assert_eq!(
        class(
            &format!(
                "LET g = (FN :{{x :Any}} -> Number = #(1))\n\
                 LET f = (FN :{{c :Number}} -> Number = #({view} (g {{x = v}}) (c)))"
            ),
            "f"
        ),
        "porous 1",
        "an exact callee whose parameter is porous"
    );
    assert_eq!(
        class(
            &format!(
                "LET g = (FN :{{x :Any}} -> Number = #(1))\n\
                 LET f = (FN :{{c :Number}} -> Number = #({view} (g {{x = c}}) (c)))"
            ),
            "f"
        ),
        "sealed 1",
        "a value that is not porous carries nothing out, at a porous slot too"
    );
    assert_eq!(
        class(
            &format!(
                "LET f = (FN :{{c :Number, h :(FN :{{x :Any}} -> Number)}} -> Number = \
                 #({view} (h v) (c)))"
            ),
            "f"
        ),
        "porous 1",
        "a callee known at most"
    );
    assert_eq!(
        class(
            &format!("LET f = (FN :{{c :Number, h :Any}} -> Number = #({view} (h v) (c)))"),
            "f"
        ),
        "porous 1",
        "a callee the load cannot type"
    );
}

#[test]
fn every_eval_is_an_exit() {
    assert_eq!(
        class(
            "LET f = (FN :{c :Number} -> Number = \
             #((LET v = (ints :| Counter)) (EVAL #(1) -> Number) (c)))",
            "f"
        ),
        "porous 1"
    );
}

#[test]
fn a_module_body_counts_toward_the_body_running_it() {
    assert_eq!(
        class(
            "LET f = (FN :{c :Number} -> Number = \
             #((MODULE m = (LET v = (ints :| Counter))) (c)))",
            "f"
        ),
        "sealed 1"
    );
}

#[test]
fn a_call_result_returned_is_the_only_exit() {
    assert_eq!(
        class(
            "LET g = (FN :{x :Number} -> Any = #(x))\n\
             LET f = (FN :{c :Number} -> Any = #((LET v = (ints :| Counter)) (g {x = c})))",
            "f"
        ),
        "porous 1"
    );
}

#[test]
fn a_nominal_declared_per_call_mints() {
    // A quantified function is bound as a module member, where nothing need fix its group.
    let source = "MODULE lib = (LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Number = \
                  #((NEWTYPE Box = :{v :Elt}) (1))))";
    loaded(&format!("{COUNTER}{source}"), |program| {
        let f = body(program, module_body(program, "lib"), "f");
        assert_eq!(rendered(f.class()), "sealed 1");
    });
    assert_eq!(
        class(
            "LET f = (FN :{x :Number} -> Number = #((NEWTYPE Box = :{v :Number}) (1)))",
            "f"
        ),
        "sealed 0",
        "a nominal the load closes is declared once"
    );
}

#[test]
fn an_arm_counts_toward_the_body_running_it() {
    assert_eq!(
        class(
            "LET f = (FN :{c :Number} -> Number = \
             #(MATCH (c) -> :Number WITH #{Number: ((LET v = (ints :| Counter)) (it))}))",
            "f"
        ),
        "sealed 1"
    );
    assert_eq!(
        class(
            "EXPR #(KEEP x :Any) -> Number = #(1)\n\
             LET f = (FN :{c :Number} -> Number = \
             #((LET v = (ints :| Counter)) (MATCH (c) -> :Number WITH #{Number: (KEEP v)})))",
            "f"
        ),
        "porous 1"
    );
}

#[test]
fn a_nested_function_is_a_body_of_its_own() {
    let source = "LET f = (FN :{c :Number} -> Any = #(\
                  (LET h = (FN :{y :Number} -> Number = #((LET v = (ints :| Counter)) (y)))) \
                  (c)))";
    assert_eq!(class(source, "f"), "sealed 0");
    loaded(&format!("{COUNTER}{source}"), |program| {
        let f = body(program, program.shape(), "f");
        assert_eq!(rendered(body(program, f, "h").class()), "sealed 1");
    });
}

#[test]
fn a_view_leaves_through_each_porous_return() {
    let returning = |returns: &str, value: &str| {
        class(
            &format!(
                "NEWTYPE Box = :{{v :Any}}\n\
                 LET f = (FN :{{c :Number}} -> {returns} = \
                 #((LET v = (ints :| Counter)) ({value})))"
            ),
            "f",
        )
    };
    assert_eq!(returning("Value", "[v]"), "porous 1");
    assert_eq!(returning("Module", "v"), "porous 1");
    assert_eq!(returning(":{b :Number}", "{a = v, b = 1}"), "porous 1");
    assert_eq!(returning("Type", "v.Carrier"), "porous 1");
    assert_eq!(returning("Box", "Box {v = v}"), "porous 1");
    assert_eq!(
        returning(":{x :(:{b :Number} | Null)}", "{x = {a = v, b = 1}}"),
        "porous 1",
        "under a union a part keeps its own memo"
    );
    assert_eq!(
        returning(":{b :Number}", "{b = 1}"),
        "sealed 1 lays",
        "an exact closed record exposes only what it names"
    );
}

#[test]
fn the_program_and_a_quote_are_frame_bodies() {
    assert_eq!(
        loaded(
            &format!("{COUNTER}LET v = (ints :| Counter)\nPRINT v"),
            |program| rendered(program.shape().class())
        ),
        "sealed 1",
        "the program lays no branch"
    );
    assert_eq!(
        code_class("LET code = #((LET v = (ints :| Counter)) (1))"),
        "sealed 1 lays",
        "a quote's code is placed `Shares`"
    );
    assert_eq!(
        code_class("LET code = #((LET v = (ints :| Counter)) (v))"),
        "porous 1"
    );
}

/// Debug builds check at each minting door that the load recorded the site: a view and a nominal
/// declared per call, each in a callable's body, run without tripping either check.
#[test]
fn the_run_finds_each_minting_site_the_load_recorded() {
    assert_eq!(
        run(&format!(
            "{COUNTER}MODULE lib = (LET f = (FN FOR ALL #[Elt] :{{x :Elt}} -> Number = \
             #((NEWTYPE Box = :{{v :Elt}}) (LET v = (ints :| Counter)) (1))))\n\
             PRINT (lib.f {{x = 2}})"
        )),
        "1"
    );
}

/// Each runnable snippet of the tutorial, as `tools/verify_snippets.py` picks them: a `koan` block
/// followed, with only whitespace between, by a `text` block, and holding no pending expression
/// shape.
fn tutorial_snippets() -> Vec<(String, String)> {
    const PENDING: [&str; 6] = ["MATCH", "TRY", "CATCH", "Result", "TYPE", "CLOSE"];
    let pending = |code: &str| {
        code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|word| PENDING.contains(&word))
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tutorial");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("the tutorial is readable")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    let mut snippets = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).expect("a tutorial page is readable");
        // Each fenced block: its language, its body, and the text after it up to the next fence.
        let mut blocks = Vec::new();
        let mut rest = text.as_str();
        while let Some(open) = rest.find("```") {
            let after = &rest[open + 3..];
            let Some(newline) = after.find('\n') else {
                break;
            };
            let language = &after[..newline];
            let Some(close) = after[newline + 1..].find("\n```") else {
                break;
            };
            let content = &after[newline + 1..newline + 1 + close];
            rest = &after[newline + 1 + close + 4..];
            blocks.push((language, content, rest.find("```").map(|at| &rest[..at])));
        }
        for pair in blocks.windows(2) {
            let ((language, code, between), (next, _, _)) = (pair[0], pair[1]);
            if language == "koan"
                && next == "text"
                && between.is_some_and(|gap| gap.trim().is_empty())
                && !pending(code)
            {
                let name = path.file_name().expect("a file").to_string_lossy();
                snippets.push((name.into_owned(), code.to_string()));
            }
        }
    }
    snippets
}

/// The measurement the shape-branches work reads: over the tutorial's runnable programs, how many
/// frame bodies the load gives each class, and how many minting sites each sealed body laying a
/// shape branch holds. Run with `cargo test classes_over_the_tutorial -- --ignored --nocapture`.
#[test]
#[ignore]
fn classes_over_the_tutorial() {
    use std::collections::BTreeMap;
    use std::collections::HashSet;

    use crate::program::CellSubstrate;
    use crate::scope::BodyShape;

    use super::{Koan, output};

    let (mut programs, mut refused) = (0, 0);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut laying: BTreeMap<u32, usize> = BTreeMap::new();
    for (_, code) in tutorial_snippets() {
        let Ok(mut substrate) = CellSubstrate::load::<Koan>(&code, "<tutorial>", 8, output())
        else {
            refused += 1;
            continue;
        };
        programs += 1;
        substrate.with(|running| {
            let mut seen = HashSet::new();
            let mut stack: Vec<&BodyShape<'_>> = vec![running.program().shape()];
            while let Some(shape) = stack.pop() {
                if !seen.insert(shape as *const BodyShape<'_>) {
                    continue;
                }
                stack.extend(shape.nested_shapes().iter().map(|(_, nested)| *nested));
                let kind = match shape.kind() {
                    ShapeKind::Program => "program",
                    ShapeKind::Callable => "callable",
                    ShapeKind::Code => "code",
                    ShapeKind::Block | ShapeKind::Module => continue,
                };
                let class = match shape.class() {
                    None => "unfixed".to_string(),
                    Some(BodyClass::Porous { .. }) => "porous".to_string(),
                    Some(BodyClass::Sealed { mints, lays_branch }) => {
                        if lays_branch {
                            *laying.entry(mints).or_default() += 1;
                        }
                        match (mints, lays_branch) {
                            (0, _) => "sealed, mints nothing",
                            (_, true) => "sealed, lays a branch",
                            (_, false) => "sealed, mints, lays no branch",
                        }
                        .to_string()
                    }
                };
                *kinds.entry(format!("{kind:8} {class}")).or_default() += 1;
            }
        });
    }
    println!("{programs} tutorial programs loaded, {refused} refused at load");
    println!("| frame body | class | bodies |\n|---|---|---|");
    for (row, count) in &kinds {
        let (kind, class) = row.split_at(8);
        println!("| {} | {} | {count} |", kind.trim(), class.trim());
    }
    println!("\n| minting sites | sealed bodies laying a branch |\n|---|---|");
    for (mints, count) in &laying {
        println!("| {mints} | {count} |");
    }
}
