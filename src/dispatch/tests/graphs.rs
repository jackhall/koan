//! The binding graph each load lays: its bind sites and what each owns, the instances the load
//! makes from the root, and the recursions it refuses for growing a name over itself.

use bindgraph::{Answers, Binding, Bound, Doors, Graph, NameId, QuestionId, Resting, Site};

use crate::build::lay::{GraphAnswers, GraphFacts, GraphKept, GraphQuestion, NameKind, folded};
use crate::memory::{Bump, BumpVec};
use crate::program::{CellSubstrate, LoadError};
use crate::scope::{
    Address, Bare, DerivedName, NameAddress, SiteAnswer, SiteRule, Spellings, Tree, Unknown,
};
use crate::symbols::{BinderSymbol, ValueSymbol};
use crate::type_lattice::KType;
use crate::values::tests::reading;
use crate::values::{BuildBudget, Chain};

use super::super::graph::{Content, GraphDomain, Minted, Naming};
use super::{Koan, output};

/// Each `BindingNode` of `source`'s graph, by its site, beside the stored rules it owns.
fn nodes(source: &str) -> Vec<String> {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        let facts = running.program().facts();
        let laid = Bump::new();
        let folded = folded(facts, &types, &bump, &laid);
        let graph: Graph<GraphKept> = (folded.text.lay(&folded.facts)).expect("the text lays");
        (graph.nodes().iter())
            .map(|node| {
                let site = match node.site() {
                    Site::Entry(body) => format!("entry {}", body.index()),
                    Site::Statement(body, at) => format!("statement {} {at}", body.index()),
                };
                let owns: Vec<String> = (node.owns().iter())
                    .filter_map(|question| folded.facts.question(*question).stored())
                    .map(|stored| format!("{:?}", stored.rule))
                    .collect();
                format!("{site}: {}", owns.join(" "))
            })
            .collect()
    })
}

#[test]
fn report_has_a_node_at_its_entry_and_at_the_module_it_builds_and_none_at_its_first_let() {
    let source = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR FOR ALL #[Elem] #(REPORT x :Elem code :Any) -> Str = #(
  LET first = (SHOW x)
  LET built = (EVAL code -> Module)
  SHOW (built.sample)
)
PRINT (REPORT 5 #(MODULE made = (LET sample = \"hi\")))";
    // The bodies in order: the root, both `SHOW`s, `REPORT`, then the quote's code. `REPORT`'s
    // entry binds `x`'s type and the name of the type `code` carries: `SHOW x`'s selection, and
    // whether `code` is code for `EVAL`.
    let expected = [
        "entry 0: ",
        "entry 1: ",
        "entry 2: Selection Eval",
        "entry 3: ",
        "entry 4: ",
        // `LET built`: `SHOW (built.sample)`, the member read's selection and retype, and the
        // return check over its value.
        "statement 2 1: Selection Selection MemberRetype ReturnCheck",
    ];
    assert_eq!(nodes(source), expected);
}

/// Each question of `source`'s graph reading `rule`: the names its asks read, each by its binder's
/// spelling and which of its names it is.
fn questions_reading(source: &str, rule: &str) -> Vec<Vec<String>> {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        let laid = running.binding_graph();
        let facts = laid.facts;
        assert_eq!(facts.left, 0, "the fold leaves no question out");
        let shapes = running.program().facts().shapes();
        let symbols = running.symbols();
        let spelled = |name: NameId| {
            let held = facts.name(name);
            let NameAddress::Site(unknown) = held.address else {
                return "answered".to_string();
            };
            let Address::Slot(slot) = unknown.at else {
                return format!("capture {:?}", held.kind);
            };
            let binder = shapes.shape_at(unknown.segment).slot_name(slot);
            format!("{} {:?}", symbols.display(binder.symbol()), held.kind)
        };
        (facts.questions.iter())
            .filter_map(|question| match *question {
                GraphQuestion::Site { question, asks } => Some((question, asks)),
                GraphQuestion::Decides { .. } => None,
            })
            .filter(|(question, _)| format!("{:?}", question.rule) == rule)
            .map(|(_, asks)| {
                let mut names: Vec<String> = (asks.iter().flatten())
                    .flat_map(|ask| facts.ask(*ask).names.iter().map(|name| spelled(*name)))
                    .collect();
                names.sort();
                names.dedup();
                names
            })
            .collect()
    })
}

#[test]
fn a_selection_over_a_path_off_a_module_parameter_reads_the_type_it_carries_alone() {
    // `SHOW`'s overloads are told apart by `c.Carrier`, read off `c` itself: what `c` carries
    // decides it, so two modules of one carried type answer alike.
    let source = "\
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = (LET zero = 0)
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(TELL c :Counter) -> Str = #(
  LET t :(c.Carrier) = c.zero
  SHOW t
)
PRINT (TELL ints)";
    assert_eq!(
        questions_reading(source, "Selection"),
        [vec!["c Carried".to_string()]]
    );
}

/// Each answer of the questions reading `rule` that the node binding `name`'s carried name owns,
/// in an instance made under `name`'s names bound to what the program's `MODULE` binder `module`
/// holds — the type it carries and its content — and in one with nothing bound.
fn answers_under_the_module(
    source: &str,
    name: &str,
    module: &str,
    rule: &str,
) -> [Vec<SiteAnswer>; 2] {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        let laid = running.binding_graph();
        let (graph, facts) = (&laid.graph, laid.facts);
        let load = running.program().facts();
        let (shapes, symbols) = (load.shapes(), running.symbols());
        let spelled = |unknown: Unknown| match unknown.at {
            Address::Slot(slot) => {
                let binder = shapes.shape_at(unknown.segment).slot_name(slot);
                symbols.display(binder.symbol()).to_string()
            }
            Address::Capture(_) | Address::Held(_) => String::new(),
        };
        let of_binder = |held: NameId, kind: NameKind| {
            let named = facts.name(held);
            matches!(named.address, NameAddress::Site(unknown) if spelled(unknown) == name)
                && named.kind == kind
        };
        let (node, carried) = (graph.nodes().iter())
            .find_map(|node| {
                let carried = (node.names().iter().copied())
                    .find(|held| of_binder(*held, NameKind::Carried))?;
                Some((graph.node_at(node.site()).expect("a node"), carried))
            })
            .expect("a node binds the binder's carried name");
        let content = (graph.node(node).names().iter().copied())
            .find(|held| of_binder(*held, NameKind::Content));
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        // What `module` holds: its content tree as the load laid it, the digest a frame binds, and
        // the signature its `MODULE` body ties.
        let root = shapes.root();
        let (slot, _) = (root.slot(BinderSymbol::Value(
            ValueSymbol::declared(module, symbols).expect("a value token"),
        )))
        .expect("the program binds the module");
        let unit = (load.own_units())
            .find_map(|(unit, (unit_facts, _))| {
                let (_, typed) = unit_facts.typed.get(root.segment())?;
                typed.contents.get(slot).map(|(_, tree)| (unit, *tree))
            })
            .map(|(_, tree)| tree)
            .expect("the module's content is laid");
        let Tree::Body { body, .. } = unit else {
            panic!("a `MODULE` binder holds its body");
        };
        let digest = unit.digest().expect("the module's content is closed");
        let birth = shapes.birth_unit(body).expect("a module is born");
        let (birth_facts, _) = load.unit(birth).expect("the module's unit is typed");
        let (_, typed) = birth_facts
            .typed
            .get(body)
            .expect("the module's body is typed");
        let signature = (typed.signature)
            .and_then(|signature| types.concrete(signature))
            .expect("the module's signature is closed");
        let bare = Bare {
            signature: Some(signature),
            sealing: None,
        };
        let minted = Minted::new(&bump);
        let names = |id| load.derived_name(id).or_else(|| minted.get(id));
        let keep = |name: DerivedName<'_>| minted.keep(name);
        let domain = GraphDomain {
            facts,
            load,
            types: &types,
            scratch: &bump,
            naming: Naming {
                names: &names,
                spellings: Spellings::new(symbols),
                laid: &keep,
            },
            noted: None,
        };
        let owned: Vec<QuestionId> = (graph.node(node).owns().iter())
            .copied()
            .filter(|question| {
                (facts.question(*question).stored())
                    .is_some_and(|stored| format!("{:?}", stored.rule) == rule)
            })
            .collect();
        assert!(!owned.is_empty(), "the node owns a question reading {rule}");
        [true, false].map(|bound| {
            let lend = Bump::new();
            let resting = Resting::new(&lend, None);
            let mut instances = Bound::new(graph, facts, laid.frozen, &lend, None);
            let mut binding = Binding::new();
            if bound {
                binding.bind(carried, Content::Carried(signature));
                if let Some(content) = content {
                    binding.bind(content, Content::Module { digest, bare });
                }
            }
            let mut answers = GraphAnswers { facts };
            let mut words = vec![0u64; graph.node(node).handle_width()];
            let handle =
                instances.instance(resting, node, &binding, &domain, &mut answers, &mut words);
            (owned.iter())
                .map(|question| instances.answer(resting, handle, *question, &mut answers))
                .collect()
        })
    })
}

#[test]
fn a_binding_of_a_module_parameter_answers_a_selection_over_a_path_read_off_it() {
    // `c` bound to what `ints` carries makes `c.Carrier` `Number`, so `SHOW t` selects the
    // `Number` overload; bound to a module whose `zero` is a string, the other. With nothing bound
    // it is left to the run, as at the load.
    let source = |zero: &str| {
        format!(
            "\
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = (LET zero = {zero})
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(TELL c :Counter) -> Str = #(
  LET t :(c.Carrier) = c.zero
  SHOW t
)
PRINT (TELL ints)"
        )
    };
    let [numbers, open] = answers_under_the_module(&source("0"), "c", "ints", "Selection");
    let [strings, _] = answers_under_the_module(&source("\"zero\""), "c", "ints", "Selection");
    let arm = |answers: &[SiteAnswer]| match answers {
        [SiteAnswer::Arm(arm)] => *arm,
        _ => panic!("one selection, answered: {answers:?}"),
    };
    assert_ne!(arm(&numbers), arm(&strings));
    assert!(
        open.iter()
            .all(|answer| !matches!(answer, SiteAnswer::Arm(_))),
        "{open:?}"
    );
}

#[test]
fn two_modules_of_one_carried_type_hide_their_carriers_apart_under_a_binding() {
    // A view keys the carriers it hides on its module's content: `m` bound to `ints`, the module
    // `fixed` views, makes `v.zero` and `fixed.zero` one carrier, so `SAME` selects; bound to
    // `other`, of the same carried type, they are two, and no overload admits them, as the load
    // refuses `CHECK other`.
    let source = |module: &str| {
        format!(
            "\
SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = (LET zero = 0)
MODULE other = (LET zero = 1)
LET fixed = (ints :| Ord)
EXPR FOR ALL #[Elem] #(SAME a :Elem b :Elem) -> Str = #(\"same\")
EXPR #(CHECK m :Ord) -> Any = #(
  LET v = (m :| Ord)
  SAME (v.zero) (fixed.zero)
)
PRINT (CHECK {module})"
        )
    };
    let [ints, open] = answers_under_the_module(&source("ints"), "m", "ints", "Selection");
    let [other, _] = answers_under_the_module(&source("ints"), "m", "other", "Selection");
    // `SAME`'s selection, the one the load leaves to the run.
    let at = (open.iter())
        .position(|answer| matches!(answer, SiteAnswer::Kept { .. }))
        .expect("the load leaves `SAME (v.zero) (fixed.zero)` to the run");
    assert!(matches!(ints[at], SiteAnswer::Arm(_)), "{ints:?}");
    assert!(matches!(other[at], SiteAnswer::Refused(_)), "{other:?}");
}

#[test]
fn a_selection_over_a_view_s_carrier_reads_the_viewed_module_s_content() {
    // `v`'s carrier is keyed on `m`'s content, so each selection over `SAME`'s arguments reads both
    // of `m`'s names.
    let source = "\
SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = (LET zero = 0)
MODULE other = (LET zero = 1)
LET fixed = (ints :| Ord)
EXPR FOR ALL #[Elem] #(SAME a :Elem b :Elem) -> Str = #(\"same\")
EXPR #(CHECK m :Ord) -> Any = #(
  LET v = (m :| Ord)
  SAME (v.zero) (fixed.zero)
)
PRINT (CHECK ints)";
    let selections = questions_reading(source, "Selection");
    assert!(!selections.is_empty());
    for names in selections {
        assert_eq!(names, ["m Carried", "m Content"]);
    }
}

/// What `source`'s load refuses with, rendered; `loaded` where it loads.
fn refused(source: &str) -> String {
    match CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output()) {
        Ok(_) => "loaded".to_string(),
        Err(LoadError::Shape { rendered }) => rendered,
        Err(error) => format!("{error:?}"),
    }
}

/// The answer function, counting the questions it answers.
struct Counting<'f, 'g> {
    answers: GraphAnswers<'f, 'g>,
    runs: usize,
}

impl<'g> Answers<GraphFacts<'g>> for Counting<'_, 'g> {
    fn answer(
        &mut self,
        question: QuestionId,
        doors: &mut Doors<'_, GraphFacts<'g>>,
    ) -> SiteAnswer {
        self.runs += 1;
        self.answers.answer(question, doors)
    }
}

/// How many questions an instance made after `source`'s load answers: of the entry binding the
/// type name `name`, under `name` bound to `content`. None where the instance is one the load made,
/// found filed.
fn answered_after_the_load(
    source: &str,
    name: &str,
    content: impl Fn(&Chain<'_, '_, '_, '_>) -> KType,
) -> usize {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        let laid = running.binding_graph();
        let (graph, facts) = (&laid.graph, laid.facts);
        let load = running.program().facts();
        let (shapes, symbols) = (load.shapes(), running.symbols());
        let spelled = |address: NameAddress| match address {
            NameAddress::Site(unknown) => match unknown.at {
                Address::Slot(slot) => {
                    let binder = shapes.shape_at(unknown.segment).slot_name(slot);
                    symbols.display(binder.symbol()).to_string()
                }
                Address::Capture(_) | Address::Held(_) => String::new(),
            },
            _ => String::new(),
        };
        let (node, bound) = (graph.nodes().iter().enumerate())
            .find_map(|(at, node)| {
                let named = node
                    .names()
                    .iter()
                    .find(|held| spelled(facts.name(**held).address) == name);
                named.map(|named| (at, *named))
            })
            .expect("a node binds the name");
        let node = graph.nodes()[node].site();
        let node = graph.node_at(node).expect("the name's site is a node");
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        let minted = Minted::new(&bump);
        let names = |id| load.derived_name(id).or_else(|| minted.get(id));
        let keep = |name: DerivedName<'_>| minted.keep(name);
        let domain = GraphDomain {
            facts,
            load,
            types: &types,
            scratch: &bump,
            naming: Naming {
                names: &names,
                spellings: Spellings::new(symbols),
                laid: &keep,
            },
            noted: None,
        };
        let lend = Bump::new();
        let resting = Resting::new(&lend, None);
        let mut instances = Bound::new(graph, facts, laid.frozen, &lend, None);
        let mut binding = Binding::new();
        binding.bind(bound, Content::Type(content(&types).into()));
        let mut answers = Counting {
            answers: GraphAnswers { facts },
            runs: 0,
        };
        let mut words = BumpVec::new_in(&bump);
        words.extend((0..graph.node(node).handle_width()).map(|_| 0u64));
        instances.instance(resting, node, &binding, &domain, &mut answers, &mut words);
        answers.runs
    })
}

#[test]
fn the_load_makes_keep_s_instance_at_the_list_its_call_passes() {
    // `WRAP 5` binds `Item` to `Number`, and `KEEP [y]` passes `Elem` the list of it: the load makes
    // `KEEP`'s instance there, so an instance made after it at `LIST OF Number` answers nothing,
    // while one at `Str`, which no call passes, answers its questions.
    let source = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW l :(LIST OF Any)) -> Str = #(\"list\")
EXPR FOR ALL #[Elem] #(KEEP x :Elem) -> Elem = #((PRINT (SHOW x)) (x))
EXPR FOR ALL #[Item] #(WRAP y :Item) -> :(LIST OF Item) = #(KEEP [y])
PRINT (WRAP 5)";
    let list = |types: &Chain<'_, '_, '_, '_>| types.list(KType::NUMBER);
    assert_eq!(answered_after_the_load(source, "Elem", list), 0);
    assert!(answered_after_the_load(source, "Elem", |_| KType::STR) > 0);
}

#[test]
fn a_recursion_building_its_for_all_name_over_itself_is_refused() {
    let source = "EXPR FOR ALL #[Item] #(DEEPEN y :Item) -> Any = #(DEEPEN [y])\nPRINT 1";
    assert_eq!(
        refused(source),
        "<test>:1:50: each turn of the recursion through (DEEPEN [y]) builds the type of `Item` \
         over itself: declare `Item` at a recursive type"
    );
    // A parameter declared `Any` is the name of the type its value carries, which the call passes
    // it as it would a `FOR ALL` name.
    let source = "EXPR #(DEEPEN y :Any) -> Any = #(DEEPEN [y])\nPRINT 1";
    assert_eq!(
        refused(source),
        "<test>:1:33: each turn of the recursion through (DEEPEN [y]) builds the type of `y` over \
         itself: declare `y` at a recursive type"
    );
}

#[test]
fn a_recursion_whose_calls_each_repeat_alone_but_grow_composed_is_refused() {
    // `MIX b a` swaps the names and `MIX a [a]` builds `Right` over `Left`: each call alone repeats,
    // and the walk through both builds `Right` over itself.
    let source = "\
EXPR FOR ALL #[Left Right] #(MIX a :Left b :Right) -> Any = #(
  LET swapped = (MIX b a)
  MIX a [a]
)
PRINT 1";
    let refusal = refused(source);
    assert!(
        refusal.contains("builds the type of `Right` over itself"),
        "{refusal}"
    );
}

#[test]
fn a_recursion_passing_names_on_renamed_or_fixed_or_through_a_declared_type_builds() {
    // `WRAP` builds a list over `Item` for `KEEP`, which leads back to no `WRAP`.
    let wrap = "\
EXPR FOR ALL #[Elem] #(KEEP x :Elem) -> Elem = #(x)
EXPR FOR ALL #[Item] #(WRAP y :Item) -> :(LIST OF Item) = #(KEEP [y])
PRINT (WRAP 5)";
    assert_eq!(refused(wrap), "loaded");
    // Each turn passes `Nest`, which the text declares, so the recursion repeats.
    let nest = "\
NEWTYPE Nest = :{inner :Any}
EXPR FOR ALL #[Item] #(DEEPEN y :Item n :Number) -> Any = #(
  MATCH n WITH #{0: (y), Number: (DEEPEN (Nest {inner = y}) (n - 1))}
)
PRINT (DEEPEN 1 2)";
    assert_eq!(refused(nest), "loaded");
    // A parameter declared `Any` passed on as itself.
    let itself = "EXPR #(DEEPEN y :Any) -> Any = #(DEEPEN y)\nPRINT 1";
    assert_eq!(refused(itself), "loaded");
    // A cycle an `EVAL` closes is one the text cannot follow.
    let eval = "\
EXPR FOR ALL #[Item] #(DEEPEN y :Item n :Number) -> Any = #(
  MATCH n WITH #{0: (y), Number: (EVAL #(DEEPEN [$y] ($n - 1)) -> Any)}
)
PRINT (DEEPEN 1 2)";
    assert_eq!(refused(eval), "loaded");
}

#[test]
fn a_selection_over_a_decided_name_reads_its_answered_name_which_a_binding_decides() {
    // `c.zero`'s type is decided by whether the type `c` carries holds `zero` at all, so `SHOW`'s
    // selection reads the answered names those questions decide, beside the type `c` carries,
    // which instances `zero` where it is quantified. Bound to what `ints` carries, the answered
    // names take their arms, and the selection the load keeps open picks the overload `zero`'s
    // type admits.
    let source = |zero: &str| {
        format!(
            "\
MODULE ints = (LET zero = {zero})
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(TELL c :Module) -> Str = #(SHOW (c.zero))
PRINT (TELL ints)"
        )
    };
    let reads = questions_reading(&source("0"), "Selection");
    assert!(
        (reads.iter()).any(|names| names.iter().any(|name| name == "answered")),
        "{reads:?}"
    );
    let [numbers, open] = answers_under_the_module(&source("0"), "c", "ints", "Selection");
    let [strings, _] = answers_under_the_module(&source("\"zero\""), "c", "ints", "Selection");
    let at = (open.iter())
        .position(|answer| matches!(answer, SiteAnswer::Kept { .. }))
        .expect("the load leaves `SHOW (c.zero)` to the run");
    let (SiteAnswer::Arm(number), SiteAnswer::Arm(string)) = (numbers[at], strings[at]) else {
        panic!("a binding selects: {numbers:?} {strings:?}");
    };
    assert_ne!(number, string);
}

thread_local! {
    /// Each instance a bind site held while a test ran: its node and its handle's words.
    static HELD: std::cell::RefCell<Vec<(bindgraph::NodeId, Vec<u64>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Each keyworded call a frame dispatched while a test ran: its source text, and whether the
    /// frame found its selection in its facts.
    static DISPATCHED: std::cell::RefCell<Vec<(String, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// What running a program showed: what it printed, each instance a bind site held as it ran, in
/// order, how many times the run's instances ran the answer function, and each keyworded call a
/// frame dispatched, by its source, with whether it found its selection in its facts.
struct Answered {
    printed: String,
    held: Vec<(bindgraph::NodeId, Vec<u64>)>,
    runs: u64,
    dispatched: Vec<(String, bool)>,
}

impl Answered {
    /// Whether every call written `call`, outer parentheses aside, was found selected, at least one ran, and the run's
    /// instances ran no answer: so the load answered its selection.
    fn selected_at_the_load(&self, call: &str) -> bool {
        // A body of one statement reports its statement within the body's parentheses.
        let unwrapped = |text: &str| {
            let inner = text
                .strip_prefix('(')
                .and_then(|text| text.strip_suffix(')'));
            inner.unwrap_or(text).trim().to_string()
        };
        let mut calls = (self.dispatched.iter()).filter(|(text, _)| unwrapped(text) == call);
        let found = calls.clone().count() > 0 && calls.all(|(_, selected)| *selected);
        found && self.runs == 0
    }
}

/// Load and run `source`, as [`Answered`] shows it.
fn answered(source: &str) -> Answered {
    use crate::program::INSTANCED;
    super::WRITTEN.with(|written| written.borrow_mut().clear());
    HELD.with(|held| held.borrow_mut().clear());
    DISPATCHED.with(|dispatched| dispatched.borrow_mut().clear());
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    INSTANCED.with(|hook| {
        hook.set(Some(|node, words, _| {
            HELD.with(|held| held.borrow_mut().push((node, words.to_vec())))
        }))
    });
    crate::dispatch::evaluate::CHOSE.with(|hook| {
        hook.set(Some(|_, _, source, selected| {
            DISPATCHED.with(|dispatched| dispatched.borrow_mut().push((source.text(), selected)))
        }))
    });
    let runs = substrate.with(|running| {
        running.ran().expect("the program runs");
        crate::build::lay::BaseInstances::runs(running.instances())
    });
    INSTANCED.with(|hook| hook.set(None));
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(None));
    let written = super::WRITTEN.with(|written| std::mem::take(&mut *written.borrow_mut()));
    Answered {
        printed: written.join("\n"),
        held: HELD.with(|held| std::mem::take(&mut *held.borrow_mut())),
        runs,
        dispatched: DISPATCHED.with(|dispatched| std::mem::take(&mut *dispatched.borrow_mut())),
    }
}

/// What `source` prints, beside each instance a bind site held as it ran, in order, and how many
/// times the run's instances ran the answer function.
fn instanced(source: &str) -> (String, Vec<(bindgraph::NodeId, Vec<u64>)>, u64) {
    let Answered {
        printed,
        held,
        runs,
        ..
    } = answered(source);
    (printed, held, runs)
}

/// The handles the bind sites at `node`'s site of `source`'s graph held, in order.
fn held_at(held: &[(bindgraph::NodeId, Vec<u64>)], node: bindgraph::NodeId) -> Vec<Vec<u64>> {
    (held.iter())
        .filter(|(at, _)| *at == node)
        .map(|(_, words)| words.clone())
        .collect()
}

/// The node of `source`'s graph at a statement that owns a question of `rule`.
fn statement_owning(source: &str, rule: SiteRule) -> bindgraph::NodeId {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        let laid = running.binding_graph();
        (laid.graph.nodes().iter())
            .filter(|node| matches!(node.site(), Site::Statement(..)))
            .find(|node| {
                (node.owns().iter())
                    .filter_map(|question| laid.facts.question(*question).stored())
                    .any(|stored| stored.rule == rule)
            })
            .and_then(|node| laid.graph.node_at(node.site()))
            .expect("a statement owns the rule")
    })
}

const REPORT: &str = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR FOR ALL #[Elem] #(REPORT x :Elem code :Any) -> Str = #(
  LET first = (SHOW x)
  LET built = (EVAL code -> Module)
  SHOW (built.sample)
)
";

#[test]
fn two_modules_whose_sample_is_one_type_hold_one_handle_where_report_builds_them() {
    let source = format!(
        "{REPORT}\
PRINT (REPORT 5 #(MODULE made = (LET sample = \"hi\")))
PRINT (REPORT 5 #(MODULE other = (LET sample = \"yo\")))"
    );
    let built = statement_owning(&source, SiteRule::MemberRetype);
    let (printed, held, _) = instanced(&source);
    assert_eq!(printed, "string\nstring");
    let at = held_at(&held, built);
    assert_eq!(at.len(), 2, "each call reaches `LET built`");
    assert_eq!(at[0], at[1], "one admission: both modules carry one type");
}

#[test]
fn the_walk_types_each_unit_once_at_the_load_and_none_across_a_run() {
    // The program's units are typed by its load alone, each once; the last `EVAL` runs code the
    // run composes, which the composition's own load types.
    let source = format!(
        "{REPORT}\
PRINT (REPORT 5 #(MODULE made = (LET sample = \"hi\")))
PRINT (REPORT 5 #(MODULE other = (LET sample = \"yo\")))
LET q = #(PRINT \"composed\")
LET qs = [q]
EVAL #($..qs) -> Any"
    );
    let mut substrate =
        CellSubstrate::load::<Koan>(&source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    let typed = |substrate: &mut CellSubstrate| {
        substrate.with(|running| crate::build::typed_over(running.program().shapes()))
    };
    let loaded = typed(&mut substrate);
    let units = substrate.with(|running| running.program().facts().own_units().count());
    assert_eq!(loaded, units as u64, "each unit the load reached, once");
    let before = crate::build::typed_in_all();
    substrate.with(|running| running.ran().expect("the program runs"));
    assert_eq!(typed(&mut substrate), loaded, "none across the run");
    // The count runs across the run: the composition's load types its code's units.
    assert!(
        crate::build::typed_in_all() > before,
        "the compositions' loads are counted"
    );
}

#[test]
fn a_second_module_of_one_carried_type_runs_no_stored_question() {
    let once = format!(
        "{REPORT}\
PRINT (REPORT 5 #(MODULE made = (LET sample = \"hi\")))"
    );
    let twice = format!(
        "{once}
PRINT (REPORT 5 #(MODULE other = (LET sample = \"yo\")))"
    );
    let (_, _, runs_once) = instanced(&once);
    let (_, _, runs_twice) = instanced(&twice);
    assert!(
        runs_once > 0,
        "the first module's bind site makes its instance"
    );
    assert_eq!(runs_twice, runs_once, "the second finds the first's filed");
}

#[test]
fn check_refuses_at_the_load_a_second_module_of_one_carried_type_hiding_another_carrier() {
    let check = "\
SIG Ord FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = (LET zero = 0)
MODULE other = (LET zero = 1)
LET fixed = (ints :| Ord)
EXPR FOR ALL #[Elem] #(SAME a :Elem b :Elem) -> Str = #(\"same\")
EXPR #(CHECK m :Ord) -> Any = #(
  LET v = (m :| Ord)
  SAME (v.zero) (fixed.zero)
)
PRINT (CHECK ints)";
    let same = "SAME (v.zero) (fixed.zero)";
    let ints = answered(check);
    assert_eq!(ints.printed, "same");
    assert!(
        ints.selected_at_the_load(same),
        "the load made the instance `CHECK ints` enters: {:?}",
        ints.dispatched
    );
    let fixed = answered(&check.replace("(CHECK ints)", "(CHECK fixed)"));
    assert_eq!(fixed.printed, "same");
    assert!(fixed.selected_at_the_load(same), "a sealed argument");
    let both = format!("{check}\nPRINT (CHECK other)");
    let Err(LoadError::Shape { rendered }) =
        CellSubstrate::load::<Koan>(&both, "<test>", 8, BuildBudget::default(), output())
    else {
        panic!("the load refuses the second call");
    };
    assert!(rendered.contains("in (CHECK other)"), "{rendered}");
}

#[test]
fn a_match_over_a_for_all_name_runs_the_arm_its_call_s_instance_chooses() {
    let source = "\
EXPR FOR ALL #[Elem] #(PICK x :Elem) -> Str = #(
  MATCH x WITH #{:Number: (\"n\"), :Str: (\"s\")}
)
PRINT (PICK 5)
PRINT (PICK \"a\")";
    let (printed, held, _) = instanced(source);
    assert_eq!(printed, "n\ns");
    assert!(!held.is_empty(), "each call's entry holds its instance");
}

#[test]
fn a_use_over_a_for_all_name_selects_the_candidate_its_call_s_instance_chooses() {
    let source = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR FOR ALL #[Elem] #(SHOWN x :Elem) -> Str = #(SHOW x)
PRINT (SHOWN 5)
PRINT (SHOWN \"a\")";
    let shown = answered(source);
    let (printed, held) = (shown.printed, shown.held);
    assert_eq!(printed, "number\nstring");
    let entries: Vec<_> = (held.iter())
        .map(|(node, _)| *node)
        .filter(|node| held.iter().filter(|(each, _)| each == node).count() == 2)
        .collect();
    let handles = held_at(&held, entries[0]);
    assert_ne!(
        handles[0], handles[1],
        "each call's entry holds its own instance"
    );
    let shows =
        (shown.dispatched.iter()).filter(|(call, selected)| call == "(SHOW x)" && *selected);
    assert_eq!(
        shows.count(),
        2,
        "each call selects `SHOW x`: {:?}",
        shown.dispatched
    );
}

/// A type a frame's binding closes is closed once per binding and kept: a loop whose every frame
/// binds `Elem` to `Number` closes as many types afresh over fifty hops as over two.
#[test]
fn a_type_a_binding_closes_is_closed_once_and_kept() {
    let source = |hops: usize| {
        format!(
            "EXPR FOR ALL #[Elem] #(KEEP x :Elem) -> Elem = #(x)\n\
             EXPR FOR ALL #[Elem] #(SPIN x :Elem n :Number) -> Any = #(\n  \
             MATCH n WITH #{{0: (x), Number: (SPIN (KEEP x) (n - 1))}}\n)\n\
             PRINT (SPIN 5 {hops})"
        )
    };
    let fresh = |hops| {
        crate::dispatch::graph::CLOSED_FRESH.with(|count| count.set(0));
        let (printed, _, _) = instanced(&source(hops));
        assert_eq!(printed, "5");
        crate::dispatch::graph::CLOSED_FRESH.with(std::cell::Cell::get)
    };
    let two = fresh(2);
    assert!(
        two > 0,
        "the frames close the result of `KEEP x` under their binding"
    );
    assert_eq!(fresh(50), two);
}

#[test]
fn a_read_through_a_call_whose_candidates_unfold_waits_on_its_selection_not_its_holder() {
    // Each `BUILD` the selection may run has a summary, so `a.Zero` is read off the one `Elt`
    // selects: `a`'s question is `SUBJECT`'s entry's, over `Elt` and the argument `x`.
    let source = "\
EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(
  EXPR #(BUILD e :Number) -> Module = #(
    (MODULE built OVER #[e] = (LET Zero = Number))
    built
  )
  EXPR #(BUILD e :Str) -> Module = #(
    (MODULE built OVER #[e] = (LET Zero = Str))
    built
  )
  LET a = (BUILD x)
  LET z :(a.Zero) = \"no\"
  z
)
PRINT 1";
    assert_eq!(
        nodes(source),
        [
            "entry 0: ",
            "entry 1: Selection Hold HoldRetype",
            "entry 2: ",
            "entry 3: "
        ]
    );
    let read = ["Elt Type", "x Carried", "x Content"].map(String::from);
    assert_eq!(questions_reading(source, "Hold"), [read]);
}

#[test]
fn a_read_through_a_call_whose_candidate_recurses_waits_on_its_holder_alone() {
    // `MAKE ints chain` unfolds once, to `MAKE`'s own call over `link.next`, which only an answer
    // selects: `r` holds that call's result whole, so `r.zero`'s questions are `r`'s statement's,
    // over `r` alone, which the run binds there.
    let source = super::caching::program(
        2,
        "EXPR #(MAKE m :Counter link :Link) -> Counter = #(MAKE m (ATTR link next))
EXPR #(MAKE m :Counter end :Null) -> Counter = #(m)
EXPR #(PICK x :Number) -> Str = #(\"n\")
EXPR #(PICK x :Str) -> Number = #(1)
EXPR #(USE n :Number) -> Any = #(
  (LET r = (MAKE ints chain))
  (LET p = (PICK r.zero))
  p
)
PRINT (USE 1)
",
    );
    let owners: Vec<_> = (nodes(&source).into_iter())
        .filter(|node| !node.ends_with(": "))
        .collect();
    assert_eq!(
        owners,
        [
            "entry 5: Selection ReturnCheck",
            "statement 1 0: Selection MemberRetype"
        ]
    );
    let read = |names: &[&str]| {
        names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        questions_reading(&source, "Selection"),
        [read(&["m Carried"]), read(&["r Carried", "r Content"])]
    );
}

/// A callable whose own bound names the encloser's `Elt` is typed by its scheme beside that bound,
/// so each use of it is a question over `Elt`: `INNER 5`'s selection, and a call by name of an
/// `inner` value, held by a plain `LET` or not, are answered where `Elt` is bound.
#[test]
fn a_use_of_a_callable_bounded_over_an_encloser_name_reads_that_name() {
    let keyworded = "EXPR #(INNER y :Any) -> Str = #(\"outer\")\n\
                     EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Str = #(\n  \
                     EXPR FOR ALL #{Tee: Elt} #(INNER y :Tee) -> Str = #(\"inner\")\n  \
                     INNER 5\n)\n\
                     PRINT (OUTER 1)\nPRINT (OUTER \"a\")";
    assert_eq!(super::run(keyworded), "inner\nouter");
    assert_eq!(questions_reading(keyworded, "Selection"), [["Elt Type"]]);
    let by_name = "EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Any = #(\n  \
                   LET inner = (FN FOR ALL #{Tee: Elt} :{y :Tee} -> Tee = #(y))\n  \
                   LET held = inner\n  \
                   PRINT (inner {y = x})\n  \
                   held {y = x}\n)\n\
                   PRINT (OUTER 5)";
    assert_eq!(super::run(by_name), "5\n5");
    let reads = questions_reading(by_name, "ByName");
    assert!(
        !reads.is_empty()
            && reads
                .iter()
                .all(|names| names.contains(&"Elt Type".to_string())),
        "{reads:?}"
    );
    // A module member typed so is no lattice node, so the module's signature leaves it unknown.
    let member = "EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Any = #(\n  \
                  MODULE holder OVER #[Elt] = \
                  (LET inner = (FN FOR ALL #{Tee: Elt} :{y :Tee} -> Tee = #(y)))\n  \
                  holder.inner {y = x}\n)\n\
                  PRINT (OUTER 5)";
    assert_eq!(super::run(member), "5");
}

/// An instance of a callable whose own bound names the encloser's `Elt`, wanted where a binding
/// of `Elt` admits none, is refused where `Elt` is bound, as a load declaring it refuses.
#[test]
fn an_instance_a_binding_of_a_bound_over_names_admits_none_is_refused() {
    let source = "EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Any = #(\n  \
                  LET inner :(FN :{y :Number} -> Number) = \
                  (FN FOR ALL #{Tee: Elt} :{y :Tee} -> Tee = #(y))\n  \
                  inner {y = 3}\n)\n";
    assert_eq!(super::run(&format!("{source}PRINT (OUTER 5)")), "3");
    assert!(super::run(&format!("{source}PRINT (OUTER \"a\")")).contains("has no instance under"),);
}

/// A member read off a module the load cannot see, called, is typed by the member read whole off
/// the type the module carries, a scheme included: `SHOW`'s selection over the call's result is a
/// question over `built`'s carried name, answered where `built` is bound, with the member held by
/// a plain `LET` first or not.
#[test]
fn a_call_of_a_quantified_member_of_an_unseen_module_is_answered_where_the_module_binds() {
    let program = |call: &str| {
        format!(
            "EXPR #(SHOW n :Number) -> Str = #(\"number\")\n\
             EXPR #(SHOW s :Str) -> Str = #(\"string\")\n\
             EXPR #(USE code :Any) -> Any = #(\n  \
             LET built = (EVAL code -> Module)\n  \
             {call}\n)\n\
             PRINT (USE #(MODULE made = (LET identity = (FN FOR ALL #[Item] :{{x :Item}} -> Item = #(x)))))"
        )
    };
    for call in [
        "SHOW ((built.identity) {x = 5})",
        "LET f = built.identity\n  SHOW (f {x = 5})",
    ] {
        let source = program(call);
        assert_eq!(super::run(&source), "number", "{call}");
        let reads = questions_reading(&source, "Selection");
        assert!(
            !reads.is_empty()
                && (reads.iter()).all(|names| names.iter().any(|name| name == "built Carried")),
            "{call}: {reads:?}"
        );
    }
}

/// What `source` prints, its locations, trace frames and when it refuses aside: a module an
/// `EVAL` builds is bound at the run, which refuses under its binding where a fresh load refuses
/// at the load, in the same words.
pub(super) fn printed_unlocated(source: &str) -> String {
    let unlocated = |line: &str| match line.split_once("<test>:") {
        Some((before, after)) => {
            let rest = after.splitn(3, ':').nth(2).unwrap_or(after);
            format!("{before}{}", rest.trim_start())
        }
        None => line.to_string(),
    };
    (super::run(source).lines())
        .filter(|line| !line.starts_with("  in "))
        .map(|line| line.strip_prefix("load: ").unwrap_or(line))
        .map(|line| line.strip_prefix("error: ").unwrap_or(line))
        .map(unlocated)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A value member of a module the load cannot see, read off a call's head — held by a name, wanted
/// at an annotation, an element or an argument — is an instance site of its scheme, which the
/// module's binding gives: each program answers as a fresh load with the module declared does,
/// instanced, refused or read as it is alike.
#[test]
fn an_unseen_member_at_an_instance_site_answers_as_a_fresh_load() {
    let prelude = "EXPR #(SHOW n :Number) -> Str = #(\"number\")\n\
                   EXPR #(SHOW s :Str) -> Str = #(\"string\")\n\
                   EXPR #(APPLY f :(FN :{x :Number} -> Number)) -> Number = #(f {x = 4})\n\
                   EXPR #(APPLY f :(FN :{x :Str} -> Str)) -> Str = #(f {x = \"s\"})\n";
    let generic = "MODULE made = (LET identity = (FN FOR ALL #[Item] :{x :Item} -> Item = #(x)))";
    let plain = "MODULE made = (LET identity = (FN :{x :Number} -> Number = #(x)))";
    let cases = [
        (
            generic,
            "(LET go :(FN :{x :Number} -> Number) = made.identity)\n  (PRINT (go {x = 3}))",
        ),
        (
            generic,
            "(LET go :(FN :{x :Number} -> Any) = made.identity)\n  (PRINT (SHOW (go {x = 3})))",
        ),
        (generic, "(PRINT [made.identity])"),
        (generic, "(LET f = made.identity)\n  (PRINT [f])"),
        (
            generic,
            "(LET f = made.identity)\n  (LET go :(FN :{x :Number} -> Number) = f)\n  (PRINT [go])",
        ),
        (
            generic,
            "(LET f = made.identity)\n  (PRINT (f {x = 3}))\n  (PRINT (f {x = \"s\"}))",
        ),
        (
            generic,
            "(LET go :(FN :{x :Str} -> Number) = made.identity)\n  (PRINT 1)",
        ),
        (generic, "(LET go :Any = made.identity)\n  (PRINT 1)"),
        (plain, "(PRINT (APPLY made.identity))"),
        (plain, "(LET f = made.identity)\n  (PRINT (APPLY f))"),
    ];
    for (module, body) in cases {
        let fresh = format!("{prelude}{module}\nEXPR #(USE) -> Any = #(\n  {body}\n)\nPRINT (USE)");
        let unseen = body.replace("made.", "built.");
        let code = module.trim_start_matches("MODULE ");
        let instance = format!(
            "{prelude}EXPR #(USE code :Any) -> Any = #(\n  (LET built = (EVAL code -> Module))\n  \
             {unseen}\n)\nPRINT (USE #(MODULE {code}))"
        );
        assert_eq!(
            printed_unlocated(&instance),
            printed_unlocated(&fresh),
            "{instance}\nas\n{fresh}"
        );
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(crate::tests::case_share(1, 1)))]

    /// A value member of a module the load cannot see answers where the module's binder binds as
    /// a fresh load with the module declared does, at every site that reads it: instanced at the
    /// type it is wanted at, read as it is, or refused in that load's words, under the binding as
    /// the run reaches it.
    #[test]
    fn an_unseen_member_answers_as_a_fresh_load_at_every_read(case in super::generate::unseen_member()) {
        let (rendered, restarted) = (case.render(), case.restart());
        proptest::prop_assert_eq!(
            printed_unlocated(&rendered),
            printed_unlocated(&restarted),
            "{}\nas\n{}",
            rendered,
            restarted
        );
    }
}

/// A member of a module the load cannot see, passed where the overloads the load keeps want it
/// at different types, takes the instance the overload its binding selects wants: as a fresh
/// load, which drops an overload the member can never be passed to, ranks those left, and
/// refuses a call whose overloads left by the text still disagree.
#[test]
fn an_unseen_member_argument_takes_the_instance_its_selection_runs() {
    let prelude = "EXPR #(APPLY f :(FN :{x :Number} -> Number)) -> Number = #(f {x = 4})\n\
                   EXPR #(APPLY n :Number) -> Number = #(n)\n\
                   EXPR #(PAIR k :Number f :(FN :{x :Number} -> Number)) -> Number = #(f {x = k})\n\
                   EXPR #(PAIR k :Str f :(FN :{x :Str} -> Str)) -> Str = #(f {x = k})\n\
                   EXPR #(RANK f :(FN :{x :Number} -> Number)) -> Number = #(f {x = 4})\n\
                   EXPR #(RANK f :(FN :{x :Number} -> Any)) -> Any = #(f {x = 5})\n";
    let generic = "MODULE made = (LET identity = (FN FOR ALL #[Item] :{x :Item} -> Item = #(x)))";
    let plain = "MODULE made = (LET identity = (FN :{x :Number} -> Number = #(x)))";
    let bodies = [
        "(PRINT (APPLY made.identity))",
        "(LET f = made.identity)\n  (PRINT (APPLY f))",
        "(PRINT (PAIR 7 made.identity))",
        "(PRINT (APPLY (APPLY made.identity)))",
        "(PRINT (RANK made.identity))",
        "(LET v :Any = 3)\n  (PRINT (PAIR v made.identity))",
    ];
    for module in [generic, plain] {
        for body in bodies {
            let fresh =
                format!("{prelude}{module}\nEXPR #(USE) -> Any = #(\n  {body}\n)\nPRINT (USE)");
            let unseen = body.replace("made.", "built.");
            let code = module.trim_start_matches("MODULE ");
            let instance = format!(
                "{prelude}EXPR #(USE code :Any) -> Any = #(\n  (LET built = (EVAL code -> Module))\n  \
                 {unseen}\n)\nPRINT (USE #(MODULE {code}))"
            );
            assert_eq!(
                printed_unlocated(&instance),
                printed_unlocated(&fresh),
                "{instance}\nas\n{fresh}"
            );
        }
    }
}

/// A refusal of a held callable bounded over an encloser's name, under the encloser's binding,
/// names the callable's scheme with that bound closed, as a fresh load with the name declared
/// does: a call by name it can never admit, and an annotation it has no instance under.
#[test]
fn a_refusal_under_a_binding_names_the_bound_it_closes() {
    let inner = "LET inner = (FN FOR ALL #{Tee: :(LIST OF Elt)} :{y :Tee} -> Any = #(y))";
    for read in [
        "inner {y = [\"s\"]}",
        "LET g :(FN :{y :(LIST OF Str)} -> Any) = inner\n  g",
    ] {
        let body = format!("#(OUTER x :Elt) -> Any = #(\n  {inner}\n  {read}\n)\nPRINT (OUTER 1)");
        let instance = format!("EXPR FOR ALL #[Elt] {body}");
        let fresh = format!("LET Elt = Number\nEXPR {body}");
        assert_eq!(
            printed_unlocated(&instance),
            printed_unlocated(&fresh),
            "{instance}\nas\n{fresh}"
        );
        assert!(
            printed_unlocated(&fresh).contains("LIST OF Number"),
            "{fresh}"
        );
    }
}

/// A read of a member a module the load cannot see does not declare is refused where the module
/// binds, worded as a fresh load with the module declared words it: the module has no such member.
#[test]
fn a_member_an_unseen_module_lacks_is_refused_as_a_fresh_load() {
    let module = "MODULE made = (LET identity = (FN FOR ALL #[Item] :{x :Item} -> Item = #(x)))";
    for body in [
        "(LET f :(FN :{x :Number} -> Number) = made.missing)\n  (PRINT \"after\")",
        "(PRINT (made.missing {x = 1}))",
    ] {
        let fresh = format!("{module}\nEXPR #(USE) -> Any = #(\n  {body}\n)\nPRINT (USE)");
        let unseen = body.replace("made.", "built.");
        let code = module.trim_start_matches("MODULE ");
        let instance = format!(
            "EXPR #(USE code :Any) -> Any = #(\n  (LET built = (EVAL code -> Module))\n  \
             {unseen}\n)\nPRINT (USE #(MODULE {code}))"
        );
        assert_eq!(
            printed_unlocated(&instance),
            printed_unlocated(&fresh),
            "{instance}\nas\n{fresh}"
        );
        assert!(
            printed_unlocated(&fresh).contains("has no member missing"),
            "{fresh}"
        );
    }
}

/// Where one instance answers two refusing questions, it refuses with the one a fresh load with
/// its binding declared meets first, walking what lies inside an expression before it: an
/// earlier statement before a nested body's, an argument before its call's selection, a `LET`'s
/// value before its annotation, at the load and where a module binds at the run. Of two
/// instances that refuse, the load reports the one a walk of the calls meets first: a caller
/// before its callee, and an earlier call's callee before a later one's.
#[test]
fn an_instance_refuses_with_the_refusal_a_fresh_load_meets_first() {
    let defs = "EXPR #(NEEDS n :Number) -> Number = #(n)\n\
                EXPR #(WANTS s :Str) -> Str = #(s)\n\
                EXPR #(PAIR a :Number b :Any) -> Number = #(a)\n\
                EXPR #(TWO f :(FN :{x :Number} -> Str) n :Number) -> Number = #(n)\n\
                EXPR #(APPLY f :(FN :{x :Number} -> Number)) -> Number = #(f {x = 4})\n\
                EXPR #(APPLY n :Number) -> Number = #(n)\n";
    let mapper = "SIG Mapper FOR ALL #[Carrier] = #[(VAL seed :Carrier) \
                  (VAL apply :(FN FOR ALL #[Elt] :{x :Carrier, y :Elt} -> Elt))]\n\
                  MODULE strs = ((LET seed = \"a\") \
                  (LET apply = (FN FOR ALL #[Elt] :{x :Str, y :Elt} -> Elt = #(y))))\n";
    let made = "MODULE made = ((LET ident = (FN FOR ALL #[Item] :{x :Item} -> Item = #(x))) \
                (LET word = \"s\") (LET other = true))";
    let code = made.trim_start_matches("MODULE ");
    let unseen = |body: &str| {
        format!(
            "{defs}EXPR #(USE code :Any) -> Any = #(\n  (LET built = (EVAL code -> Module))\n  \
             {}\n)\nPRINT (USE #(MODULE {code}))",
            body.replace("made.", "built.")
        )
    };
    let declared = |body: &str| format!("{defs}{made}\n{body}");
    let generic = |body: &str| {
        format!(
            "{defs}EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Any = #(\n  {body}\n)\nPRINT (OUTER true)"
        )
    };
    let bool_outer = |body: &str| {
        format!("{defs}EXPR #(OUTER x :Bool) -> Any = #(\n  {body}\n)\nPRINT (OUTER true)")
    };
    let nested = "(WANTS x)\n  (LET f = (FN :{y :Number} -> Any = #(NEEDS x)))\n  (x)";
    let run_nested =
        "(PRINT (NEEDS made.other))\n  (LET f = (FN :{y :Number} -> Any = #(NEEDS made.word)))";
    // The selection and the return check share a span; the selection is asked first.
    let tied = |declared: &str, head: &str| {
        format!(
            "{defs}{declared}EXPR {head}#(OUTER x :Elt) -> Elt = #(PAIR x 1)\nPRINT (OUTER true)"
        )
    };
    let pairs = [
        (generic(nested), bool_outer(nested)),
        (
            unseen(run_nested),
            declared(
                "PRINT (NEEDS made.other)\nLET f = (FN :{y :Number} -> Any = #(NEEDS made.word))",
            ),
        ),
        (
            unseen("(PRINT (TWO made.ident (NEEDS made.word)))"),
            declared("PRINT (TWO made.ident (NEEDS made.word))"),
        ),
        (
            unseen("(PRINT (APPLY made.missing))"),
            declared("PRINT (APPLY made.missing)"),
        ),
        (tied("", "FOR ALL #[Elt] "), tied("LET Elt = Bool\n", "")),
        (
            format!(
                "{mapper}EXPR #(DIRECT m :Mapper) -> Any = \
                 #(LET go :(FN :{{x :Number, y :Number}} -> Number) = m.apply)\nPRINT (DIRECT strs)"
            ),
            format!("{mapper}LET go :(FN :{{x :Number, y :Number}} -> Number) = strs.apply"),
        ),
    ];
    let siblings = |head: &str, a: &str, b: &str| {
        format!(
            "{defs}EXPR {head}#(GENA x :{a}) -> Any = #(NEEDS x)\n\
             EXPR {head}#(GENB x :{b}) -> Any = #(WANTS x)\n\
             PRINT (GENA \"s\")\nPRINT (GENB 5)"
        )
    };
    let caller = |head: &str, outer: &str| {
        format!(
            "{defs}EXPR FOR ALL #[Elt] #(GENA x :Elt) -> Any = #(NEEDS x)\n\
             EXPR {head}#(OUTER x :{outer}) -> Any = #(\n  (LET a = (GENA x))\n  (WANTS x)\n)\n\
             PRINT (OUTER true)"
        )
    };
    let pairs = pairs.into_iter().chain([
        (
            siblings("FOR ALL #[Elt] ", "Elt", "Elt"),
            siblings("", "Str", "Number"),
        ),
        (caller("FOR ALL #[Elt] ", "Elt"), caller("", "Bool")),
    ]);
    for (instance, fresh) in pairs {
        assert_eq!(
            printed_unlocated(&instance),
            printed_unlocated(&fresh),
            "{instance}\nas\n{fresh}"
        );
    }
}

#[test]
fn a_name_declared_any_is_answered_at_the_load_where_the_text_fixes_it_and_by_the_run_cell_otherwise()
 {
    // `TELL 5` gives `x` the content `Number`, so the load makes `TELL`'s instance there. `RUN`
    // passes `TELL` an `EVAL`'s result, whose type arrives as the program runs: the run cell that
    // binds `x` makes that instance, or finds the load's.
    let tell = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(TELL x :Any) -> Str = #(SHOW x)
EXPR #(RUN code :Any) -> Str = #(TELL (EVAL code -> Any))
";
    let told = answered(&format!("{tell}PRINT (TELL 5)"));
    assert_eq!(told.printed, "number");
    assert!(told.selected_at_the_load("SHOW x"), "{:?}", told.dispatched);
    let (printed, _, runs) = instanced(&format!("{tell}PRINT (RUN #(\"a\"))"));
    assert_eq!(printed, "string");
    assert!(runs > 0, "the run cell binding `x` makes the instance");
    let both = answered(&format!("{tell}PRINT (TELL 5)\nPRINT (RUN #(5))"));
    assert_eq!(both.printed, "number\nnumber");
    assert!(
        both.selected_at_the_load("SHOW x"),
        "the load's instance, found: {:?}",
        both.dispatched
    );
}
/// How many distinct instances the bind sites at each node held, by node, beside how many times a
/// bind site there was reached.
fn distinct(held: &[(bindgraph::NodeId, Vec<u64>)]) -> Vec<(usize, usize)> {
    let mut nodes: Vec<bindgraph::NodeId> = held.iter().map(|(node, _)| *node).collect();
    nodes.sort_by_key(|node| node.index());
    nodes.dedup();
    (nodes.into_iter())
        .map(|node| {
            let mut at = held_at(held, node);
            let reached = at.len();
            at.sort();
            at.dedup();
            (at.len(), reached)
        })
        .collect()
}

/// Each spelling of a read of `call`'s result's `sample` through `SHOW`: held at a `LET`, and
/// written inline.
fn held_and_inline(call: &str) -> [String; 2] {
    [
        format!("EXPR #(USE) -> Str = #(\n  LET result = ({call})\n  SHOW (result.sample)\n)\n"),
        format!("EXPR #(USE) -> Str = #(\n  SHOW (({call}).sample)\n)\n"),
    ]
}

/// `SHOW`'s two overloads, and `BOX`, a functor whose overloads build a module whose `sample` is
/// a number or a string.
pub(super) const SHOW_BOX: &str = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(BOX n :Number) -> Module = #(MODULE made = (LET sample = 1))
EXPR #(BOX s :Str) -> Module = #(MODULE made = (LET sample = \"hi\"))
";

#[test]
fn a_holder_the_text_types_is_answered_at_the_load_and_one_the_run_types_is_shared() {
    // `FIXED`'s call passes `PICK` a number: the answer through the call types the holder, and the
    // load makes its instance. `USE`'s passes an `EVAL`'s result: the run binds the holder, and
    // two modules of one carried type share its instance. A `LET` holds `PICK`'s value, or the
    // call is written in place.
    let pick = format!("{SHOW_BOX}EXPR #(PICK x :Any) -> Module = #(\n  BOX x\n)\n");
    let shows: [(Spelling, Spelling); 2] = [
        (held_read, |_| "SHOW (result.sample)".to_string()),
        (inline_read, |call| format!("SHOW (({call}).sample)")),
    ];
    for (call, show) in shows {
        let fixed = format!(
            "EXPR #(FIXED) -> Str = #(\n{}\n)\nPRINT (FIXED)",
            call("PICK 5")
        );
        let run = answered(&format!("{pick}{fixed}"));
        assert_eq!(run.printed, "number");
        assert!(
            run.selected_at_the_load(&show("PICK 5")),
            "{fixed}: {:?}",
            run.dispatched
        );
        let user = format!(
            "EXPR #(USE code :Any) -> Str = #(\n{}\n)\n",
            call("PICK (EVAL code -> Any)")
        );
        let (once, twice) = (
            answered(&format!("{pick}{user}PRINT (USE #(5))")),
            answered(&format!("{pick}{user}PRINT (USE #(5))\nPRINT (USE #(6))")),
        );
        assert_eq!(twice.printed, "number\nnumber");
        assert!(once.runs > 0, "the run binds the holder");
        assert_eq!(
            twice.runs, once.runs,
            "the second module finds the first's instance"
        );
        // Each call holds one handle at each of its bind sites, the holder's among them.
        assert_eq!(twice.held.len(), 2 * once.held.len());
        for (node, _) in &twice.held {
            let handles = held_at(&twice.held, *node);
            assert!(
                handles.len() == 2 && handles[0] == handles[1],
                "{user}: {:?}",
                twice.held
            );
        }
    }
}

/// A spelling of a read of a call's result, by the call.
type Spelling = fn(&str) -> String;

/// `PICK`'s call `call` held at a `LET`, beside `SHOW`'s read of its `sample`.
fn held_read(call: &str) -> String {
    format!("  LET result = ({call})\n  SHOW (result.sample)")
}

/// `SHOW`'s read of `call`'s `sample`, the call written in place.
fn inline_read(call: &str) -> String {
    format!("  SHOW (({call}).sample)")
}

#[test]
fn a_refusal_in_a_holder_the_text_types_refuses_the_build_and_one_the_run_types_faults() {
    // `BOX`'s number module holds a list, which no `SHOW` admits. `FIXED`'s holder is typed at the
    // load, so the build is refused; `USE`'s is typed by the run, which faults at its bind site
    // once `start` prints. The two spellings refuse alike.
    let boxes = SHOW_BOX.replace("(LET sample = 1)", "(LET sample = [1])");
    let pick = format!("{boxes}EXPR #(PICK x :Any) -> Module = #(\n  BOX x\n)\n");
    let mut said = Vec::new();
    for call in [held_read, inline_read] {
        let fixed = format!(
            "EXPR #(FIXED) -> Str = #(\n  PRINT \"start\"\n{}\n)\nPRINT (FIXED)",
            call("PICK 5")
        );
        let refused = super::run(&format!("{pick}{fixed}"));
        assert!(refused.starts_with("load: "), "{refused}");
        assert!(refused.contains("no overload of `SHOW _`"), "{refused}");
        assert!(refused.contains("in (FIXED)"), "{refused}");
        let user = format!(
            "EXPR #(USE code :Any) -> Str = #(\n  PRINT \"start\"\n{}\n)\nPRINT (USE #(5))",
            call("PICK (EVAL code -> Any)")
        );
        let faulted = super::run(&format!("{pick}{user}"));
        assert!(faulted.starts_with("start\n"), "{faulted}");
        assert!(faulted.contains("no overload of `SHOW _`"), "{faulted}");
        said.push((unplaced(&refused), unplaced(&faulted)));
    }
    assert_eq!(said[0], said[1]);
}

/// That `SHOW`'s read of `call`'s result's `sample`, held at a `LET` or written `inline`, prints
/// `printed` and is answered at the load, after `prelude`.
fn read_at_the_load(prelude: &str, cases: &[(&str, &str)], inline: bool) {
    for (call, printed) in cases {
        let spelling = &held_and_inline(call)[usize::from(inline)];
        let run = answered(&format!("{SHOW_BOX}{prelude}{spelling}PRINT (USE)"));
        assert_eq!(run.printed, *printed, "{spelling}");
        let show = match inline {
            false => "SHOW (result.sample)".to_string(),
            true => format!("SHOW (({call}).sample)"),
        };
        assert!(
            run.selected_at_the_load(&show),
            "{spelling}: {:?}",
            run.dispatched
        );
    }
}

/// `PICK`, whose last statement selects between `NUMS` and `STRS` by `Elem`, and the calls a
/// text fixes `Elem` at, beside what each prints.
const PICK: &str = "\
EXPR #(NUMS) -> Module = #(MODULE made = (LET sample = 1))
EXPR #(STRS) -> Module = #(MODULE made = (LET sample = \"hi\"))
EXPR FOR ALL #[Elem] #(PICK x :Elem) -> Module = #(
  MATCH x WITH #{Number: (NUMS), Str: (STRS)}
)
";
const PICKED: [(&str, &str); 2] = [("PICK 5", "number"), ("PICK \"a\"", "string")];

/// `SPIN`, whose result is `BOX x` at the end of the recursion, selected by `Elem`, and the calls
/// a text fixes `Elem` at, beside what each prints.
const SPIN: &str = "\
EXPR FOR ALL #[Elem] #(SPIN x :Elem n :Number) -> Module = #(
  MATCH n WITH #{0: (BOX x), Number: (SPIN x (n - 1))}
)
";
const SPUN: [(&str, &str); 2] = [("SPIN 5 3", "number"), ("SPIN \"a\" 3", "string")];

#[test]
fn a_member_of_a_functor_selected_by_a_for_all_name_is_answered_at_the_load() {
    read_at_the_load(PICK, &PICKED, false);
}

#[test]
fn a_result_reaches_a_holder_through_a_chain_of_calls_and_under_the_caller_s_own_name() {
    // `MID` returns `PICK`'s result; `PASS`'s `LET` holds `PICK`'s result under its own `Item`.
    let chained = "\
EXPR FOR ALL #[Mid] #(MID z :Mid) -> Module = #(
  PICK z
)
";
    read_at_the_load(&format!("{PICK}{chained}"), &[("MID 5", "number")], false);
    let passing = format!(
        "{SHOW_BOX}{PICK}EXPR FOR ALL #[Item] #(PASS y :Item) -> Str = #(\n  \
         LET result = (PICK y)\n  SHOW (result.sample)\n)\n"
    );
    let run = answered(&format!("{passing}PRINT (PASS 5)\nPRINT (PASS \"a\")"));
    assert_eq!(run.printed, "number\nstring");
    assert!(
        run.selected_at_the_load("SHOW (result.sample)"),
        "{:?}",
        run.dispatched
    );
}

#[test]
fn a_member_of_a_functor_selected_by_a_for_all_name_written_inline_is_answered_at_the_load() {
    read_at_the_load(PICK, &PICKED, true);
}

#[test]
fn a_recursion_whose_result_waits_on_itself_is_answered_at_the_load_within_its_bound() {
    read_at_the_load(SPIN, &SPUN, false);
}

#[test]
fn a_result_bound_at_a_callee_s_holder_is_answered_at_the_load() {
    // `MID`'s result is selected over `first`, a holder of its own: the load reaches `LET first`'s
    // instance from `MID`'s entry, and takes `MID`'s result there.
    let mid = "\
EXPR #(PICK x :Any) -> Module = #(
  BOX x
)
EXPR #(MID z :Any) -> Module = #(
  LET first = (PICK z)
  BOX (first.sample)
)
";
    read_at_the_load(mid, &[("MID 5", "number"), ("MID \"a\"", "string")], false);
}

#[test]
fn a_match_with_an_arm_the_load_cannot_type_leaves_its_holder_to_the_run() {
    // `OR`'s second arm returns an `EVAL`'s module, which only the run types: the first arm's
    // result is no answer for `result`, and the run binds it.
    let or = "\
EXPR FOR ALL #[Elem] #(OR x :Elem n :Number code :Any) -> Module = #(
  MATCH n WITH #{0: (BOX x), Number: (EVAL code -> Module)}
)
EXPR #(USE) -> Str = #(
  LET result = (OR 5 1 #(MODULE made = (LET sample = \"hi\")))
  SHOW (result.sample)
)
PRINT (USE)";
    let run = answered(&format!("{SHOW_BOX}{or}"));
    assert_eq!(run.printed, "string");
    assert!(run.runs > 0, "the run binds `result`");
}

#[test]
fn a_recursion_whose_result_waits_on_itself_written_inline_is_answered_at_the_load() {
    read_at_the_load(SPIN, &SPUN, true);
}

#[test]
fn a_refusal_at_each_bind_site_names_its_own_site() {
    // `x` arrives from an `EVAL`, so the entry's instance refuses `SHOW x`; `built`'s module holds a
    // list, so the instance at `LET built` refuses `SHOW (built.sample)`.
    let report = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(REPORT x :Any code :Any) -> Str = #(
  LET first = (SHOW x)
  LET built = (EVAL code -> Module)
  SHOW (built.sample)
)
PRINT (REPORT 5 #(MODULE made = (LET sample = \"hi\")))
";
    let at_entry = super::run(&format!(
        "{report}PRINT (REPORT (EVAL #([1]) -> Any) #(MODULE made = (LET sample = \"hi\")))"
    ));
    assert!(
        at_entry.starts_with("string\nerror: <test>:4:15: no overload of `SHOW _`"),
        "{at_entry}"
    );
    let at_built = super::run(&format!(
        "{report}PRINT (REPORT 5 #(MODULE made = (LET sample = [1])))"
    ));
    assert!(
        at_built.starts_with("string\nerror: <test>:6:3: no overload of `SHOW _`"),
        "{at_built}"
    );
}

#[test]
fn a_loop_alternating_its_module_binder_holds_one_instance_per_carrier() {
    // Each hop of `WALK` swaps its two modules, and `TELL m.held` reads only the type `m` carries:
    // two modules of one carried type share one instance at `WALK`'s entry, and of two carried
    // types hold two, however long the loop runs.
    let walk = |second: &str, depth: u32| {
        let mut source = String::from(
            "\
SIG Hold FOR ALL #[Elt] = #[(VAL held :Elt)]
MODULE ints = (LET held = 1)
MODULE more = (LET held = 2)
MODULE strs = (LET held = \"a\")
EXPR #(TELL x :Number) -> Str = #(\"num\")
EXPR #(TELL x :Str) -> Str = #(\"str\")
EXPR #(WALK m :Hold other :Hold link :{next :Any}) -> Number = #(
  LET told = (TELL m.held)
  WALK other m (ATTR link next)
)
EXPR #(WALK m :Hold other :Hold end :Null) -> Number = #(7)
LET c0 = null
",
        );
        for at in 1..=depth {
            source.push_str(&format!("LET c{at} = {{next = c{}}}\n", at - 1));
        }
        source.push_str(&format!("PRINT (WALK ints {second} c{depth})"));
        source
    };
    for (second, instances) in [("more", 1), ("strs", 2)] {
        let runs = [4, 8].map(|depth| {
            let (printed, held, runs) = instanced(&walk(second, depth));
            assert_eq!(printed, "7");
            assert_eq!(distinct(&held), [(instances, depth as usize)], "{second}");
            runs
        });
        assert_eq!(
            runs[0], runs[1],
            "{second}: later hops find their instances filed"
        );
    }
}

#[test]
fn a_loop_through_a_new_type_each_hop_makes_no_instance_once_its_answers_repeat() {
    // Each hop hands `DEEPEN` a list one level deeper, built by an `EVAL` the text cannot follow:
    // each new type is answered once, and every answer repeats the first, so one instance serves.
    let deepen = |depth: u32| {
        format!(
            "\
EXPR #(SHOW l :(LIST OF Any)) -> Str = #(\"list\")
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(DEEPEN y :Any n :Number) -> Str = #(
  MATCH n WITH #{{0: (SHOW y), Number: (DEEPEN (EVAL #([$y]) -> Any) (n - 1))}}
)
PRINT (DEEPEN [1] {depth})"
        )
    };
    for depth in [4, 8] {
        let (printed, held, _) = instanced(&deepen(depth));
        assert_eq!(printed, "list");
        assert_eq!(distinct(&held), [(1, depth as usize + 1)], "{depth}");
    }
}

#[test]
fn a_recursion_over_a_declared_recursive_type_holds_one_instance_at_every_depth() {
    // Each call binds `Item` to `Chain`, which the text declares over itself: the end the first
    // call takes and each link built over it are one type, so one instance serves every depth.
    let deepen = |depth: u32| {
        format!(
            "\
UNION Chain = #{{Link: :{{next :Chain}}, End: Null}}
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW c :Chain) -> Str = #(\"chain\")
EXPR FOR ALL #[Item] #(DEEPEN y :Item n :Number) -> Str = #(
  LET shown = (SHOW y)
  MATCH n WITH #{{0: (shown), Number: (DEEPEN (Chain.Link {{next = y}}) (n - 1))}}
)
PRINT (DEEPEN (Chain.End null) {depth})"
        )
    };
    for depth in [4, 8] {
        let (printed, held, _) = instanced(&deepen(depth));
        assert_eq!(printed, "chain");
        assert_eq!(distinct(&held), [(1, depth as usize + 1)], "{depth}");
    }
}

/// `printed` with every position the text writes at taken out: two spellings of one program
/// write their refusals at different lines.
fn unplaced(printed: &str) -> String {
    let mut out = String::new();
    let mut rest = printed;
    while let Some(at) = rest.find("<test>:") {
        out.push_str(&rest[..at + "<test>".len()]);
        rest = &rest[at + "<test>:".len()..];
        rest = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == ':');
    }
    out.push_str(rest);
    out
}

/// `held`, a program whose holders are `LET` binders, and `inline`, the same program with each
/// written in place: they print alike, refuse alike but for where each is written, leave as many
/// calls to the run, and make as many instances through as many answer runs.
pub(super) fn spelled_alike(held: &str, inline: &str) {
    let (held_run, inline_run) = (answered(held), answered(inline));
    assert_eq!(
        unplaced(&inline_run.printed),
        unplaced(&held_run.printed),
        "{inline}"
    );
    let unselected = |run: &Answered| {
        (run.dispatched.iter())
            .filter(|(_, selected)| !selected)
            .count()
    };
    assert_eq!(
        unselected(&inline_run),
        unselected(&held_run),
        "{inline}: {:?}\n{held}: {:?}",
        inline_run.dispatched,
        held_run.dispatched
    );
    assert_eq!(
        (inline_run.runs, inline_run.held.len()),
        (held_run.runs, held_run.held.len()),
        "{inline}"
    );
}

/// That `inline`, a program writing parts in place, lays the binding graph its `LET` spelling
/// `held` lays, up to where each is written: as many names of each kind, questions of each rule
/// reading as many names, edges mapping as many names, and nodes.
pub(super) fn laid_alike(held: &str, inline: &str) {
    assert_eq!(laid_shape(inline), laid_shape(held), "{inline}");
}

/// The binding graph `source` lays, its addresses aside: each name's kind, each question's rule
/// beside how many names it reads, each edge's mapping width, each sorted, and its node count.
#[allow(clippy::type_complexity)]
fn laid_shape(source: &str) -> (Vec<String>, Vec<(String, usize)>, Vec<usize>, usize) {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .expect("the program loads");
    substrate.with(|running| {
        let laid = running.binding_graph();
        let (graph, facts) = (&laid.graph, laid.facts);
        let mut names: Vec<_> = (facts.names.iter())
            .map(|name| format!("{:?}", name.kind))
            .collect();
        let mut questions: Vec<_> = (graph.questions().iter().enumerate())
            .map(|(at, question)| {
                let rule = match facts.questions[at] {
                    GraphQuestion::Site { question, .. } => format!("{:?}", question.rule),
                    GraphQuestion::Decides { .. } => "Decides".to_string(),
                };
                (rule, question.reads().len())
            })
            .collect();
        let mut edges: Vec<_> = (graph.edges().iter())
            .map(|edge| edge.mapping().len())
            .collect();
        names.sort();
        questions.sort();
        edges.sort();
        (names, questions, edges, graph.nodes().len())
    })
}

/// Three overloads of `BOTH` over a `Number` and a `Str`, and the generic `TRIM`, which prints.
pub(super) const BOTHS: &str = "\
EXPR #(BOTH a :Number b :Number) -> Str = #(\"two numbers\")
EXPR #(BOTH a :Number b :Str) -> Str = #(\"number, string\")
EXPR #(BOTH a :Str b :Number) -> Str = #(\"string, number\")
EXPR FOR ALL #[Elem] #(TRIM x :Elem) -> Elem = #(
  PRINT \"trimming\"
  x
)
";

/// `PING` and `PONG`, each a module-valued recursion through the other ending at `BOX`.
const PING_PONG: &str = "\
EXPR FOR ALL #[Elem] #(PING x :Elem n :Number) -> Module = #(
  MATCH n WITH #{0: (BOX x), Number: (PONG x (n - 1))}
)
EXPR FOR ALL #[Elem] #(PONG x :Elem n :Number) -> Module = #(
  MATCH n WITH #{0: (BOX x), Number: (PING x (n - 1))}
)
";

#[test]
fn two_holders_of_one_unit_read_by_one_question_refuse_where_the_second_binds() {
    // `BOTH`'s question reads both holders, and is answered where the second binds, under the
    // first's binding: refused before `TRIM` prints, at `BOTH`.
    let call =
        "PRINT (USE #(MODULE made = (LET sample = \"yo\")) #(MODULE made = (LET sample = \"hi\")))";
    let held = format!(
        "{BOTHS}EXPR #(USE left :Any right :Any) -> Str = #(
  PRINT \"start\"
  LET first = (EVAL left -> Module)
  LET second = (EVAL right -> Module)
  BOTH (first.sample) (TRIM (second.sample))
)
{call}"
    );
    let inline = format!(
        "{BOTHS}EXPR #(USE left :Any right :Any) -> Str = #(
  PRINT \"start\"
  BOTH ((EVAL left -> Module).sample) (TRIM ((EVAL right -> Module).sample))
)
{call}"
    );
    spelled_alike(&held, &inline);
    let printed = answered(&inline).printed;
    assert!(
        printed.starts_with("start\nerror: <test>:10:3: no overload of `BOTH _ _` admits"),
        "{printed}"
    );
}

#[test]
fn holders_written_in_place_answer_as_their_let_spellings_do() {
    let numbers =
        "PRINT (USE #(MODULE made = (LET sample = 1)) #(MODULE made = (LET sample = \"hi\")))
PRINT (USE #(MODULE made = (LET sample = 2)) #(MODULE made = (LET sample = 3)))";
    let pick = format!("{SHOW_BOX}EXPR #(PICK x :Any) -> Module = #(\n  BOX x\n)\n{BOTHS}");
    let wrap = "EXPR #(WRAP m :Module) -> Module = #(
  MODULE outer OVER #[m] = (LET sample = (m.sample))
)
";
    let pairs = [
        // Two holders of one unit, each read by one question both read.
        (
            format!(
                "{BOTHS}EXPR #(USE left :Any right :Any) -> Str = #(
  PRINT \"start\"
  LET first = (EVAL left -> Module)
  LET second = (EVAL right -> Module)
  BOTH (first.sample) (second.sample)
)
{numbers}"
            ),
            format!(
                "{BOTHS}EXPR #(USE left :Any right :Any) -> Str = #(
  PRINT \"start\"
  BOTH ((EVAL left -> Module).sample) ((EVAL right -> Module).sample)
)
{numbers}"
            ),
        ),
        // A holder inside another's call.
        (
            format!(
                "{pick}EXPR #(USE) -> Str = #(
  LET inner = (PICK 5)
  LET outer = (PICK (SHOW (inner.sample)))
  SHOW (outer.sample)
)
PRINT (USE)"
            ),
            format!(
                "{pick}EXPR #(USE) -> Str = #(
  SHOW ((PICK (SHOW ((PICK 5).sample))).sample)
)
PRINT (USE)"
            ),
        ),
        // A unit's value written over a holder, the text typing it and the run.
        (
            format!(
                "{pick}EXPR #(USE code :Any) -> Str = #(
  LET built = (PICK (EVAL code -> Any))
  LET shown = (TRIM (built.sample))
  SHOW shown
)
PRINT (USE #(5))"
            ),
            format!(
                "{pick}EXPR #(USE code :Any) -> Str = #(
  LET shown = (TRIM ((PICK (EVAL code -> Any)).sample))
  SHOW shown
)
PRINT (USE #(5))"
            ),
        ),
        // A functor applied to a functor's result.
        (
            format!(
                "{pick}{wrap}EXPR #(USE code :Any) -> Str = #(
  LET built = (PICK (EVAL code -> Any))
  LET wrapped = (WRAP built)
  SHOW (wrapped.sample)
)
PRINT (USE #(5))
PRINT (USE #(\"a\"))"
            ),
            format!(
                "{pick}{wrap}EXPR #(USE code :Any) -> Str = #(
  SHOW ((WRAP (PICK (EVAL code -> Any))).sample)
)
PRINT (USE #(5))
PRINT (USE #(\"a\"))"
            ),
        ),
        // Two results of a mutual recursion, the text typing them and the run.
        (
            format!(
                "{SHOW_BOX}{BOTHS}{PING_PONG}EXPR #(USE) -> Str = #(
  LET first = (PING 5 3)
  LET second = (PONG \"a\" 2)
  BOTH (first.sample) (second.sample)
)
PRINT (USE)"
            ),
            format!(
                "{SHOW_BOX}{BOTHS}{PING_PONG}EXPR #(USE) -> Str = #(
  BOTH ((PING 5 3).sample) ((PONG \"a\" 2).sample)
)
PRINT (USE)"
            ),
        ),
        (
            format!(
                "{SHOW_BOX}{BOTHS}{PING_PONG}EXPR #(USE left :Any right :Any) -> Str = #(
  LET first = (PING (EVAL left -> Any) 3)
  LET second = (PONG (EVAL right -> Any) 2)
  BOTH (first.sample) (second.sample)
)
PRINT (USE #(5) #(\"a\"))"
            ),
            format!(
                "{SHOW_BOX}{BOTHS}{PING_PONG}EXPR #(USE left :Any right :Any) -> Str = #(
  BOTH ((PING (EVAL left -> Any) 3).sample) ((PONG (EVAL right -> Any) 2).sample)
)
PRINT (USE #(5) #(\"a\"))"
            ),
        ),
        // A holder passed on as an argument, and at the top level.
        (
            format!(
                "{pick}EXPR #(TELL m :Module) -> Str = #(\n  SHOW (m.sample)\n)
EXPR #(USE) -> Str = #(\n  LET built = (PICK 5)\n  TELL built\n)\nPRINT (USE)"
            ),
            format!(
                "{pick}EXPR #(TELL m :Module) -> Str = #(\n  SHOW (m.sample)\n)
EXPR #(USE) -> Str = #(\n  TELL (PICK 5)\n)\nPRINT (USE)"
            ),
        ),
        (
            format!("{pick}LET built = (PICK 5)\nPRINT (SHOW (built.sample))"),
            format!("{pick}PRINT (SHOW ((PICK 5).sample))"),
        ),
    ];
    let arm = format!(
        "{pick}EXPR #(USE n :Number) -> Str = #(
  MATCH n WITH #{{0: (SHOW ((PICK 5).sample)), Number: (\"other\")}}
)
PRINT (USE 0)"
    );
    for (held, inline) in &pairs {
        spelled_alike(held, inline);
    }
    // A holder in an arm, which no `LET` spells, binds in the unit of its encloser running it.
    for inline in (pairs.iter().map(|(_, inline)| inline)).chain([&arm]) {
        let run = answered(inline);
        assert!(
            (run.dispatched.iter()).all(|(_, selected)| *selected),
            "{inline}: {:?}",
            run.dispatched
        );
    }
    assert_eq!(answered(&arm).printed, "number");
}
