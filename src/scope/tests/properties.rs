//! The laws: a generated shape plan, rendered and parsed, shapes back into the plan; a plan with one
//! refusal injected is refused with it; a re-declared name takes the reads nearest it; and every
//! activation of a planned shape reads what its coordinates name.

use std::collections::BTreeSet;

use proptest::prelude::*;

use crate::memory::{CellHandle, KnotPlan, Writer};
use crate::parse::{BinderSymbol, ExpressionPart, KExpression, LabelInterner};
use crate::scope::{
    Activation, Binding, Builtins, Capture, CaptureSource, ClosureBindings, ClosureRefused,
    Coordinate, MentionClass, Position, Shape, ShapeError, ShapeKind, Site, Slot, Target,
};
use crate::type_lattice::KType;
use crate::values::{Value, resident};

use super::plan::{self, Class, Generator, Kind, Lands, Refusal, Rendering, Token};
use super::{builtins, with_fixture};

// ---------- reading the rendering back ----------

/// A name part or a nested body met walking parsed parts in source order.
enum Met<'g> {
    Name(Site),
    Open(&'g Shape<'g>),
    Close,
}

/// Walk `part` in source order. With a shape chain, a part the innermost shape nests a body at opens
/// that body's shape.
fn walk<'g>(part: &ExpressionPart<'g>, shapes: &mut Vec<&'g Shape<'g>>, met: &mut Vec<Met<'g>>) {
    match part {
        ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => {
            met.push(Met::Name(Site::of(part)))
        }
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node) => {
            let nested = shapes.last().and_then(|shape| shape.nested(Site::of(part)));
            if let Some(nested) = nested {
                met.push(Met::Open(nested));
                shapes.push(nested);
            }
            for inner in node.reference().parts {
                walk(&inner.value, shapes, met);
            }
            if nested.is_some() {
                shapes.pop();
                met.push(Met::Close);
            }
        }
        ExpressionPart::ListLiteral(items) => {
            for item in items.iter() {
                walk(item, shapes, met);
            }
        }
        ExpressionPart::DictLiteral(pairs) => {
            for (key, value) in pairs.iter() {
                walk(key, shapes, met);
                walk(value, shapes, met);
            }
        }
        ExpressionPart::RecordLiteral(pairs) => {
            for (_, value) in pairs.iter() {
                walk(value, shapes, met);
            }
        }
        ExpressionPart::Keyword(_)
        | ExpressionPart::Literal(_)
        | ExpressionPart::QuotedExpression(_) => {}
    }
}

/// Where each planned read and each planned scope landed in the parsed source.
struct Located<'g> {
    sites: Vec<Site>,
    shapes: Vec<Option<&'g Shape<'g>>>,
}

/// Pair the rendering's tokens with the names and bodies met walking `nodes`. Without a `root`
/// shape, only the names are paired.
fn locate<'g>(
    rendering: &Rendering<'_>,
    root: Option<&'g Shape<'g>>,
    nodes: &[&KExpression<'g>],
) -> Located<'g> {
    let mut met = Vec::new();
    let mut shapes: Vec<_> = root.into_iter().collect();
    for node in nodes {
        for part in node.parts {
            walk(&part.value, &mut shapes, &mut met);
        }
    }
    let tokens: Vec<Token> = rendering
        .tokens
        .iter()
        .copied()
        .filter(|token| root.is_some() || matches!(token, Token::Mention(_) | Token::Other))
        .collect();
    let source = &rendering.source;
    assert_eq!(
        tokens.len(),
        met.len(),
        "`{source}` parsed out of step with its rendering"
    );
    let mut sites = vec![None; rendering.reads.len()];
    let mut shapes = vec![None; rendering.scopes.len()];
    shapes[0] = root;
    for (token, met) in tokens.into_iter().zip(met) {
        match (token, met) {
            (Token::Mention(index), Met::Name(site)) => sites[index] = Some(site),
            (Token::Other, Met::Name(_)) | (Token::Close, Met::Close) => {}
            (Token::Open(index), Met::Open(shape)) => shapes[index] = Some(shape),
            (token, _) => panic!("`{source}` parsed out of step with its rendering at {token:?}"),
        }
    }
    Located {
        sites: sites
            .into_iter()
            .map(|site| site.expect("every read is met"))
            .collect(),
        shapes,
    }
}

// ---------- the shape against its plan ----------

fn kind(kind: Kind) -> ShapeKind {
    match kind {
        Kind::Program => ShapeKind::Program,
        Kind::Callable => ShapeKind::Callable,
        Kind::Arm | Kind::Eval => ShapeKind::Block,
    }
}

fn class(class: Class) -> MentionClass {
    match class {
        Class::Eager => MentionClass::Eager,
        Class::Deferred => MentionClass::Deferred,
    }
}

/// Where a coordinate lands: a builtin, or a binder of the shape at `level` of the chain.
#[derive(PartialEq, Eq, Debug)]
enum Found {
    Builtin,
    Binder { level: usize, name: BinderSymbol },
}

/// Follow `coordinate`, read at `level`, back through the chain's block steps and captures.
fn follow(chain: &[&Shape<'_>], level: usize, coordinate: Coordinate) -> Found {
    let mut at = level;
    for _ in 0..coordinate.hops {
        assert_eq!(
            chain[at].kind(),
            ShapeKind::Block,
            "a hop steps out of a block"
        );
        at -= 1;
    }
    let shape = chain[at];
    match coordinate.target {
        Target::Builtin(_) => {
            assert_eq!(
                coordinate.hops, 0,
                "a builtin reads through the reader's own header"
            );
            Found::Builtin
        }
        Target::Local(slot) => Found::Binder {
            level: at,
            name: shape.slot_name(slot),
        },
        Target::Capture(slot) => {
            assert_eq!(
                shape.kind(),
                ShapeKind::Callable,
                "only a callable captures"
            );
            match shape.captures()[slot.index()].source {
                CaptureSource::Read(source) => follow(chain, at - 1, source),
                CaptureSource::Member { component, index } => {
                    let enclosing = chain[at - 1];
                    let member = enclosing.components()[component as usize].members[index as usize];
                    Found::Binder {
                        level: at - 1,
                        name: enclosing.slot_name(member),
                    }
                }
            }
        }
    }
}

/// Check every planned scope's shape against its plan; `prefix` is the chain of shapes the root
/// scope reads through.
fn check(
    labels: &LabelInterner,
    rendering: &Rendering<'_>,
    located: &Located<'_>,
    prefix: &[&Shape<'_>],
) {
    let source = &rendering.source;
    for (index, rendered) in rendering.scopes.iter().enumerate() {
        let planned = rendered.scope;
        let shape = located.shapes[index].expect("every planned scope has a shape");
        let mut chain = Vec::new();
        let mut at = Some(index);
        while let Some(scope) = at {
            chain.push(located.shapes[scope].expect("every planned scope has a shape"));
            at = rendering.scopes[scope].parent;
        }
        chain.extend(prefix.iter().rev());
        chain.reverse();
        let level = rendered.level;
        assert_eq!(chain.len(), level + 1);

        assert_eq!(shape.kind(), kind(planned.kind), "`{source}`");
        assert_eq!(shape.statements(), planned.statements.len() as u32);
        assert_eq!(
            shape.keeps_defining_scope(),
            planned.keeps_defining_scope(),
            "`{source}`"
        );

        // The layout: values in symbol order, then types in symbol order, each at its position.
        let mut layout: Vec<_> = planned
            .binders()
            .into_iter()
            .map(|(name, position)| (name.is_type(), name.symbol(labels), position))
            .collect();
        layout.sort();
        assert_eq!(shape.slots(), layout.len(), "`{source}`");
        for (slot, (_, name, position)) in layout.into_iter().enumerate() {
            assert_eq!(
                shape.slot(name),
                Some((Slot(slot as u32), Position(position)))
            );
        }

        // The components are the planned partition, each holding only deferred reads.
        let components =
            |sets: Vec<BTreeSet<BinderSymbol>>| sets.into_iter().collect::<BTreeSet<_>>();
        let built: Vec<BTreeSet<BinderSymbol>> = shape
            .components()
            .iter()
            .map(|component| {
                assert!(component.deferred_only, "`{source}`");
                component
                    .members
                    .iter()
                    .map(|slot| shape.slot_name(*slot))
                    .collect()
            })
            .collect();
        let expected: Vec<BTreeSet<BinderSymbol>> = planned
            .components
            .iter()
            .map(|members| members.iter().map(|name| name.symbol(labels)).collect())
            .collect();
        assert_eq!(built.len(), expected.len(), "`{source}`");
        assert_eq!(components(built), components(expected), "`{source}`");

        // One mention at each planned read's site, with its class, statement and landing, and no
        // other.
        let reads: Vec<_> = rendering
            .reads
            .iter()
            .enumerate()
            .filter(|(_, placed)| placed.scope == index)
            .collect();
        assert_eq!(shape.mentions().len(), reads.len(), "`{source}`");
        for (read, placed) in reads {
            let mention = shape
                .mention(located.sites[read])
                .expect("a mention at the planned read's site");
            assert_eq!(mention.name, placed.read.name.symbol(labels), "`{source}`");
            assert_eq!(mention.statement, placed.statement, "`{source}`");
            assert_eq!(mention.class, class(placed.read.class), "`{source}`");
            let expected = match placed.read.lands {
                Lands::Builtin => Found::Builtin,
                Lands::Binder { up } => Found::Binder {
                    level: level - up,
                    name: placed.read.name.symbol(labels),
                },
                Lands::Nowhere => unreachable!("a valid plan reads nowhere"),
            };
            assert_eq!(
                follow(&chain, level, mention.coordinate),
                expected,
                "`{source}`"
            );
        }

        // Each nested scope, and a capture of a fellow member of its statement's binder's component
        // is a member, every other one a read.
        let children: Vec<_> = rendering
            .scopes
            .iter()
            .enumerate()
            .filter(|(_, child)| child.parent == Some(index))
            .collect();
        assert_eq!(shape.nested_shapes().len(), children.len(), "`{source}`");
        for (child, rendered) in children {
            let binder = planned.statements[rendered.statement as usize].binder;
            let fellows: BTreeSet<BinderSymbol> = planned
                .components
                .iter()
                .find(|members| binder.is_some_and(|binder| members.contains(&binder)))
                .map(|members| members.iter().map(|name| name.symbol(labels)).collect())
                .unwrap_or_default();
            let nested = located.shapes[child].expect("every planned scope has a shape");
            for capture in nested.captures() {
                match capture.source {
                    CaptureSource::Member { .. } => {
                        assert!(fellows.contains(&capture.name), "`{source}`")
                    }
                    CaptureSource::Read(Coordinate {
                        hops: 0,
                        target: Target::Local(_),
                    }) => assert!(!fellows.contains(&capture.name), "`{source}`"),
                    CaptureSource::Read(_) => {}
                }
            }
        }
    }
}

/// Build `program`'s rendering and hand the program's shape, its lines and where the plan landed
/// to `test`.
fn shaped_plan(program: &plan::Scope, test: impl for<'g, 'c> FnOnce(ShapedPlan<'_, 'g, 'c>)) {
    let rendering = plan::render_program(program);
    with_fixture(|fixture| {
        let lines = fixture.parse(&rendering.source);
        fixture.in_cell(|writer, handles| {
            let table = builtins(fixture, writer);
            let shape = Shape::of_program(fixture.program, &lines, table, fixture.scratch())
                .unwrap_or_else(|error| {
                    panic!(
                        "`{}` shapes: {}",
                        rendering.source,
                        error.display(fixture.labels)
                    )
                });
            let nodes: Vec<_> = lines.iter().collect();
            let located = locate(&rendering, Some(shape), &nodes);
            test(ShapedPlan {
                labels: fixture.labels,
                rendering: &rendering,
                located,
                shape,
                writer,
                table,
                handles,
            });
        })
    });
}

struct ShapedPlan<'p, 'g, 'c> {
    labels: &'p LabelInterner,
    rendering: &'p Rendering<'p>,
    located: Located<'g>,
    shape: &'g Shape<'g>,
    writer: Writer<'c>,
    table: &'c Builtins<'g, 'c>,
    handles: &'p [CellHandle],
}

// ---------- activations ----------

/// What a read observes, comparable.
#[derive(PartialEq, Debug)]
enum Observed {
    Number(u64),
    Type(KType),
    Pending(CellHandle),
    Edge(u32),
}

fn observe(binding: Binding<'_, '_>) -> Observed {
    match binding {
        Binding::Bound(Value::Number(number)) => Observed::Number(number.to_bits()),
        Binding::Bound(Value::Type(ty)) => Observed::Type(ty.handle()),
        Binding::Bound(other) => panic!("every slot is bound to a number, found {other:?}"),
        Binding::Pending(handle) => Observed::Pending(handle),
        Binding::Edge(edge) => Observed::Edge(edge.index()),
    }
}

/// What `follow` turns a capture of an enclosing knot edge into.
fn followed(index: u32) -> Value<'static, 'static> {
    Value::Number(-1.0 - f64::from(index))
}

/// Activate `shape` with every slot bound to a fresh number, check its by-name and capture laws, and
/// activate every shape nested in it the way a call or an arm would.
fn activate<'g, 'c>(
    writer: Writer<'c>,
    table: &'c Builtins<'g, 'c>,
    shape: &'g Shape<'g>,
    closure: &'c ClosureBindings<'g, 'c>,
    enclosing: Option<&'c Activation<'g, 'c>>,
    next: &mut f64,
) {
    let plan = KnotPlan::new(64);
    let activation = resident(
        writer,
        Activation::new(writer, shape, closure, table, enclosing),
    );
    for slot in 0..shape.slots() {
        activation
            .bind(Slot(slot as u32), Value::Number(*next))
            .unwrap();
        *next += 1.0;
    }

    // By name at the read's own position lands where the coordinate does.
    for mention in shape.mentions() {
        let at = match mention.class {
            MentionClass::Eager => Position::statement(mention.statement as usize),
            MentionClass::Deferred => shape.end(),
        };
        assert_eq!(
            activation.coordinate_of(mention.name, at),
            Some(mention.coordinate)
        );
        assert_eq!(
            activation.resolve_by_name(mention.name, at).map(observe),
            Some(observe(activation.read(mention.coordinate)))
        );
    }
    // A local is found by name exactly at the positions that see it.
    for slot in 0..shape.slots() {
        let slot = Slot(slot as u32);
        let (_, declared) = shape.slot(shape.slot_name(slot)).unwrap();
        for position in 0..=shape.end().get() {
            let at = Position(position);
            let local = Coordinate {
                hops: 0,
                target: Target::Local(slot),
            };
            assert_eq!(
                activation.coordinate_of(shape.slot_name(slot), at) == Some(local),
                at.sees(declared)
            );
        }
    }

    for (_, nested) in shape.nested_shapes() {
        match nested.kind() {
            ShapeKind::Block => activate(
                writer,
                table,
                nested,
                ClosureBindings::EMPTY,
                Some(activation),
                next,
            ),
            ShapeKind::Callable => {
                let bindings = ClosureBindings::born(
                    writer,
                    nested,
                    activation,
                    |_, index| plan.edge(index).unwrap(),
                    |edge| followed(edge.index()),
                )
                .expect("every enclosing slot is bound");
                // A captured value is the enclosing binding's word; a fellow is an edge.
                for (index, capture) in nested.captures().iter().enumerate() {
                    let held = bindings.get(crate::scope::CaptureSlot(index as u32));
                    match (capture.source, held) {
                        (CaptureSource::Read(source), Capture::Value(value)) => {
                            let expected = match activation.read(source) {
                                Binding::Bound(bound) => observe(Binding::Bound(bound)),
                                Binding::Edge(edge) => {
                                    observe(Binding::Bound(followed(edge.index())))
                                }
                                Binding::Pending(_) => panic!("every enclosing slot is bound"),
                            };
                            assert_eq!(observe(Binding::Bound(value)), expected);
                        }
                        (CaptureSource::Member { index, .. }, Capture::Edge(edge)) => {
                            assert_eq!(edge.index(), index)
                        }
                        (source, _) => panic!("the binding does not follow its source {source:?}"),
                    }
                }
                activate(writer, table, nested, bindings, None, next);
            }
            ShapeKind::Program | ShapeKind::Module => panic!("a plan nests no such shape"),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: crate::tests::case_share(1, 1), ..ProptestConfig::default() })]

    /// A planned program shapes back into its plan — every scope's kind, layout,
    /// components and nested scopes, and every mention's site, class, statement and landing.
    #[test]
    fn a_planned_program_shapes_back_into_its_plan(choices in plan::choices()) {
        let program = Generator::new(&choices).program();
        shaped_plan(&program, |shaped| check(shaped.labels, shaped.rendering, &shaped.located, &[]));
    }

    /// A plan with one refusal injected is refused with exactly that refusal.
    #[test]
    fn an_injected_refusal_is_the_one_reported(
        choices in plan::choices(),
        which in 0..5usize,
        scope in any::<usize>(),
    ) {
        let (program, refusal) = plan::refused(&choices, which, scope);
        let rendering = plan::render_program(&program);
        with_fixture(|fixture| {
            let lines = fixture.parse(&rendering.source);
            fixture.in_cell(|writer, _| {
                let table = builtins(fixture, writer);
                let source = &rendering.source;
                let error = Shape::of_program(fixture.program, &lines, table, fixture.scratch())
                    .err()
                    .unwrap_or_else(|| panic!("`{source}` is refused with {refusal:?}"));
                let labels = fixture.labels;
                let expected = match &refusal {
                    Refusal::Rebind { name, first, second } => ShapeError::Rebind {
                        name: name.symbol(labels),
                        first: Position(*first),
                        second: Position(*second),
                    },
                    Refusal::ShadowsBuiltin { name, at } => ShapeError::ShadowsBuiltin {
                        name: name.symbol(labels),
                        at: Position(*at),
                    },
                    Refusal::Unbound { name } => {
                        let nodes: Vec<_> = lines.iter().collect();
                        let located = locate(&rendering, None, &nodes);
                        let read = rendering
                            .reads
                            .iter()
                            .position(|placed| placed.read.lands == Lands::Nowhere)
                            .expect("the refused read is placed");
                        ShapeError::Unbound {
                            name: name.symbol(labels),
                            site: located.sites[read],
                            statement: rendering.reads[read].statement,
                        }
                    }
                    Refusal::EagerCycle(members) => {
                        let ShapeError::EagerCycle { members: built } = &error else {
                            panic!("`{source}` is refused with {refusal:?}, not {error:?}");
                        };
                        let built: BTreeSet<_> = built.iter().copied().collect();
                        let members = members.iter().map(|name| name.symbol(labels)).collect();
                        assert_eq!(built, members, "`{source}`");
                        return;
                    }
                };
                assert_eq!(error, expected, "`{source}`");
            })
        });
    }

    /// A name re-declared inside a nested scope takes the reads that see the nested binder, and the
    /// enclosing binder keeps its own.
    #[test]
    fn a_redeclared_name_takes_the_reads_nearest_it(choices in plan::choices()) {
        let mut generator = Generator::new(&choices);
        let mut program = generator.program();
        generator.shadow(&mut program);
        shaped_plan(&program, |shaped| check(shaped.labels, shaped.rendering, &shaped.located, &[]));
    }

    /// Over every activation of a planned program, by-name resolution agrees with the
    /// coordinates, a local is visible exactly where its position is seen, and closure bindings
    /// copy the enclosing words.
    #[test]
    fn activations_read_what_their_coordinates_name(choices in plan::choices()) {
        let program = Generator::new(&choices).program();
        shaped_plan(&program, |shaped| {
            activate(shaped.writer, shaped.table, shaped.shape, ClosureBindings::EMPTY, None, &mut 1.0);
        });
    }

    /// A claimed slot reads as its binder until bound, and a callable is born only once
    /// every slot it reads is bound.
    #[test]
    fn a_pending_slot_names_its_binder_and_refuses_a_birth(
        choices in plan::choices(),
        bound in proptest::collection::vec(any::<bool>(), 8),
    ) {
        let program = Generator::new(&choices).program();
        shaped_plan(&program, |shaped| {
            let ShapedPlan { writer, table, shape, handles, rendering, .. } = shaped;
            let activation = Activation::new(writer, shape, ClosureBindings::EMPTY, table, None);
            let is_bound = |slot: Slot| bound[slot.index() % bound.len()];
            for slot in 0..shape.slots() {
                let slot = Slot(slot as u32);
                activation.claim(slot, handles[slot.index()]).unwrap();
                if is_bound(slot) {
                    activation.bind(slot, Value::Number(slot.index() as f64)).unwrap();
                }
            }
            for mention in shape.mentions() {
                let Target::Local(slot) = mention.coordinate.target else { continue };
                let expected = if is_bound(slot) {
                    Observed::Number((slot.index() as f64).to_bits())
                } else {
                    Observed::Pending(handles[slot.index()])
                };
                assert_eq!(observe(activation.read(mention.coordinate)), expected);
            }
            let plan = KnotPlan::new(64);
            for (_, nested) in shape.nested_shapes() {
                if nested.kind() != ShapeKind::Callable {
                    continue;
                }
                let first_pending = nested.captures().iter().find_map(|capture| match capture.source {
                    CaptureSource::Read(Coordinate { target: Target::Local(slot), .. })
                        if !is_bound(slot) =>
                    {
                        Some(ClosureRefused { name: capture.name, pending: handles[slot.index()] })
                    }
                    _ => None,
                });
                let born = ClosureBindings::born(
                    writer,
                    nested,
                    &activation,
                    |_, index| plan.edge(index).unwrap(),
                    |edge| followed(edge.index()),
                );
                assert_eq!(born.err(), first_pending, "`{}`", rendering.source);
            }
        });
    }

    /// An `EVAL` body planned over the names a statement of the program or of an arm inside
    /// it sees shapes back into its plan over that scope's chain, and each of its enclosing reads is
    /// the site's own by-name read.
    #[test]
    fn an_eval_body_resolves_over_its_site(choices in plan::choices()) {
        let mut generator = Generator::new(&choices);
        let program = generator.program();
        let rendering = plan::render_program(&program);

        // A site: the program, or an arm reached through arms alone, and one of its statements.
        let chain_of = |index: usize| {
            let mut chain = vec![index];
            while let Some(parent) = rendering.scopes[*chain.last().unwrap()].parent {
                chain.push(parent);
            }
            chain.reverse();
            chain
        };
        let sites: Vec<usize> = (0..rendering.scopes.len())
            .filter(|index| {
                chain_of(*index)[1..]
                    .iter()
                    .all(|scope| rendering.scopes[*scope].scope.kind == Kind::Arm)
            })
            .collect();
        let site_chain = chain_of(sites[generator.pick(sites.len())]);
        let innermost = rendering.scopes[*site_chain.last().unwrap()].scope;
        let at = generator.pick(innermost.statements.len()) as u32;

        // What the site sees: each scope's parameters and the binders before the statement the
        // chain reads it at, and only the innermost arm's `it`.
        let mut visible = Vec::new();
        let mut it_seen = false;
        for (depth, scope) in site_chain.iter().enumerate().rev() {
            let reads_at = match site_chain.get(depth + 1) {
                Some(child) => rendering.scopes[*child].statement,
                None => at,
            };
            for (name, position) in rendering.scopes[*scope].scope.binders() {
                if name == plan::Name::It {
                    if it_seen {
                        continue;
                    }
                    it_seen = true;
                }
                if position <= reads_at {
                    visible.push((name, depth));
                }
            }
        }
        let body = generator.eval_body(site_chain.len(), &visible);
        let quoted = plan::render_eval(&body, site_chain.len());

        with_fixture(|fixture| {
            let lines = fixture.parse(&rendering.source);
            let eval_lines = fixture.parse(&quoted.source);
            let ExpressionPart::QuotedExpression(quote) = eval_lines[0].parts[0].value else {
                panic!("`{}` is a quote", quoted.source);
            };
            let quote = quote.reference();
            fixture.in_cell(|writer, _| {
                let table = builtins(fixture, writer);
                let program_shape =
                    Shape::of_program(fixture.program, &lines, table, fixture.scratch())
                        .expect("a planned program shapes");
                let nodes: Vec<_> = lines.iter().collect();
                let located = locate(&rendering, Some(program_shape), &nodes);

                // Activate each scope of the chain beside the one it sits in.
                let mut next = 1.0;
                let mut site: Option<&Activation<'_, '_>> = None;
                let mut shapes = Vec::new();
                for scope in &site_chain {
                    let shape = located.shapes[*scope].expect("every planned scope has a shape");
                    let activation = resident(
                        writer,
                        Activation::new(writer, shape, ClosureBindings::EMPTY, table, site),
                    );
                    for slot in 0..shape.slots() {
                        activation.bind(Slot(slot as u32), Value::Number(next)).unwrap();
                        next += 1.0;
                    }
                    site = Some(activation);
                    shapes.push(shape);
                }
                let site = site.expect("the chain holds the program");
                let position = Position::statement(at as usize);
                let shape = Shape::for_eval(fixture.program, quote, site, position, fixture.scratch())
                    .unwrap_or_else(|error| panic!(
                        "`{}` over `{}` shapes: {}",
                        quoted.source,
                        rendering.source,
                        error.display(fixture.labels),
                    ));
                let located = locate(&quoted, Some(shape), &[quote]);
                check(fixture.labels, &quoted, &located, &shapes);

                let evaluated = Activation::new(writer, shape, ClosureBindings::EMPTY, table, Some(site));
                for mention in shape.mentions() {
                    if matches!(mention.coordinate, Coordinate { hops: 0, target: Target::Local(_) | Target::Capture(_) }) {
                        continue;
                    }
                    let by_name = site.resolve_by_name(mention.name, position);
                    assert_eq!(by_name.map(observe), Some(observe(evaluated.read(mention.coordinate))));
                }
            })
        });
    }
}
