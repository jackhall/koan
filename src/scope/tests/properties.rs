//! The laws: every shape built from a generated program agrees with the model's own analysis, and
//! every activation of it reads what its coordinates name.

use std::collections::BTreeSet;

use proptest::prelude::*;
use proptest::sample::Index;

use crate::memory::{CellHandle, KnotPlan, Writer};
use crate::parse::{BinderSymbol, ExpressionPart, KExpression, LabelInterner};
use crate::scope::{
    Activation, Binding, Builtins, Capture, CaptureSource, ClosureBindings, ClosureRefused,
    Coordinate, MentionClass, Position, Shape, ShapeError, ShapeKind, Site, Slot, Target,
};
use crate::values::{Value, resident};

use super::model::{self, Class, Expr, Kind, ModelError, ModelScope, READS, Resolved, Statement};
use super::{builtins, value_name, with_fixture};

// ---------- names ----------

fn symbol(labels: &LabelInterner, text: &str) -> BinderSymbol {
    BinderSymbol::Value(value_name(text, labels))
}

/// The model's spelling of a value name the builder resolved.
fn text(labels: &LabelInterner, name: BinderSymbol) -> &'static str {
    READS
        .iter()
        .copied()
        .find(|text| name == symbol(labels, text))
        .expect("every value name is one the model reads")
}

fn kind(kind: ShapeKind) -> Kind {
    match kind {
        ShapeKind::Program => Kind::Program,
        ShapeKind::Callable => Kind::Callable,
        ShapeKind::Block => Kind::Block,
        ShapeKind::Module => panic!("the model renders no module"),
    }
}

fn class(class: MentionClass) -> Class {
    match class {
        MentionClass::Eager => Class::Eager,
        MentionClass::Deferred => Class::Deferred,
    }
}

fn model_error(labels: &LabelInterner, error: &ShapeError) -> ModelError {
    match error {
        ShapeError::Rebind { name, .. } => ModelError::Rebind(text(labels, *name)),
        ShapeError::Unbound {
            name, statement, ..
        } => ModelError::Unbound(text(labels, *name), *statement),
        ShapeError::EagerCycle { members } => {
            ModelError::EagerCycle(members.iter().map(|member| text(labels, *member)).collect())
        }
        other => panic!("the model renders nothing refused as {other:?}"),
    }
}

// ---------- sites ----------

/// Where the walk stands in the parsed program: a part, or a node.
#[derive(Clone, Copy)]
enum At<'n, 'g> {
    Part(&'n ExpressionPart<'g>),
    Node(&'n KExpression<'g>),
}

/// Step through parentheses the renderer adds: a parenthesized part is its node, and a formless
/// single-part node is its part.
fn settle<'n, 'g: 'n>(mut at: At<'n, 'g>) -> At<'n, 'g> {
    loop {
        at = match at {
            At::Part(ExpressionPart::Expression(node)) => At::Node(node.reference()),
            At::Node(node) if node.cache().form().is_none() && node.parts.len() == 1 => {
                At::Part(&node.parts[0].value)
            }
            settled => return settled,
        }
    }
}

/// The sites the model's walk meets in one body, in its own order: each mention's, and each nested
/// body's with the body and the model statements it renders.
struct Sites<'m, 'g> {
    mentions: Vec<Site>,
    bodies: Vec<(Site, &'g KExpression<'g>, &'m [Statement])>,
}

fn sites<'m, 'n, 'g: 'n>(
    statements: &'m [Statement],
    nodes: impl Iterator<Item = &'n KExpression<'g>>,
) -> Sites<'m, 'g> {
    let mut sites = Sites {
        mentions: Vec::new(),
        bodies: Vec::new(),
    };
    let nodes: Vec<_> = nodes.collect();
    assert_eq!(nodes.len(), statements.len(), "one node per statement");
    for (statement, node) in statements.iter().zip(nodes) {
        match statement {
            Statement::Let(_, rhs) => {
                expression_sites(rhs, At::Part(&node.parts[3].value), &mut sites)
            }
            Statement::Function { body, .. } => body_site(&node.parts[9].value, body, &mut sites),
            Statement::Bare(expr) => expression_sites(expr, At::Node(node), &mut sites),
        }
    }
    sites
}

fn body_site<'m, 'g>(part: &ExpressionPart<'g>, body: &'m [Statement], sites: &mut Sites<'m, 'g>) {
    let ExpressionPart::Expression(node) = part else {
        panic!("a body is a parenthesized node");
    };
    sites.bodies.push((Site::of(part), node.reference(), body));
}

fn expression_sites<'m, 'n, 'g: 'n>(expr: &'m Expr, at: At<'n, 'g>, sites: &mut Sites<'m, 'g>) {
    match (expr, settle(at)) {
        (Expr::Name(_), At::Part(part @ ExpressionPart::Identifier(_))) => {
            sites.mentions.push(Site::of(part))
        }
        (Expr::Number | Expr::Quote(_), _) => {}
        (Expr::List(items), At::Part(ExpressionPart::ListLiteral(parts))) => {
            assert_eq!(items.len(), parts.len());
            for (item, part) in items.iter().zip(parts.iter()) {
                expression_sites(item, At::Part(part), sites);
            }
        }
        (Expr::Call(arguments), At::Node(node)) => {
            assert_eq!(arguments.len() + 1, node.parts.len());
            for (argument, part) in arguments.iter().zip(&node.parts[1..]) {
                expression_sites(argument, At::Part(&part.value), sites);
            }
        }
        (Expr::Lambda { body, .. }, At::Node(node)) => body_site(&node.parts[5].value, body, sites),
        (Expr::Arm { scrutinee, body }, At::Node(node)) => {
            expression_sites(scrutinee, At::Part(&node.parts[1].value), sites);
            let ExpressionPart::Expression(branches) = node.parts[5].value else {
                panic!("the branches are a parenthesized node");
            };
            body_site(&branches.reference().parts[2].value, body, sites);
        }
        (Expr::Eval(inner), At::Node(node)) => {
            expression_sites(inner, At::Part(&node.parts[1].value), sites)
        }
        (expr, _) => panic!("the rendering of {expr:?} parsed to another shape"),
    }
}

// ---------- law 1–3: the shape against the model ----------

/// Where a coordinate lands: a builtin, or a binder of the shape at `level` of the chain.
#[derive(PartialEq, Eq, Debug)]
enum Found {
    Builtin,
    Binder { level: usize, name: &'static str },
}

/// Follow `coordinate`, read at `level`, back through the chain's block steps and captures.
fn follow(
    labels: &LabelInterner,
    chain: &[(&Shape<'_>, &ModelScope)],
    level: usize,
    coordinate: Coordinate,
) -> Found {
    let mut at = level;
    for _ in 0..coordinate.hops {
        assert_eq!(
            chain[at].0.kind(),
            ShapeKind::Block,
            "a hop steps out of a block"
        );
        at -= 1;
    }
    let shape = chain[at].0;
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
            name: text(labels, shape.slot_name(slot)),
        },
        Target::Capture(slot) => {
            assert_eq!(
                shape.kind(),
                ShapeKind::Callable,
                "only a callable captures"
            );
            match shape.captures()[slot.index()].source {
                CaptureSource::Read(source) => follow(labels, chain, at - 1, source),
                CaptureSource::Member { component, index } => {
                    let enclosing = chain[at - 1].0;
                    let member = enclosing.components()[component as usize].members[index as usize];
                    Found::Binder {
                        level: at - 1,
                        name: text(labels, enclosing.slot_name(member)),
                    }
                }
            }
        }
    }
}

/// Check the chain's innermost shape against its model scope, then every shape nested in it.
fn check<'m, 'n, 'g: 'n>(
    labels: &LabelInterner,
    chain: &mut Vec<(&'g Shape<'g>, &'m ModelScope)>,
    statements: &'m [Statement],
    nodes: Vec<&'n KExpression<'g>>,
) {
    let level = chain.len() - 1;
    let (shape, scope) = chain[level];
    assert_eq!(kind(shape.kind()), scope.kind);
    assert_eq!(shape.statements(), scope.statements);
    assert_eq!(shape.keeps_defining_scope(), scope.keeps_defining_scope);

    // Law 2: value slots in symbol order at their binders' positions, and nothing else.
    assert!(shape.types().is_empty());
    assert_eq!(shape.slots(), scope.binders.len());
    let mut layout: Vec<_> = scope
        .binders
        .iter()
        .map(|(name, position)| (value_name(name, labels), *position))
        .collect();
    layout.sort();
    for (index, (name, position)) in layout.into_iter().enumerate() {
        assert_eq!(
            shape.value_slot(name),
            Some((Slot(index as u32), Position(position)))
        );
    }

    // Law 3: the components partition, and which of them hold only deferred mentions.
    let built: BTreeSet<(BTreeSet<&str>, bool)> = shape
        .components()
        .iter()
        .map(|component| {
            let members = component
                .members
                .iter()
                .map(|slot| text(labels, shape.slot_name(*slot)))
                .collect();
            (members, component.deferred_only)
        })
        .collect();
    let expected: BTreeSet<(BTreeSet<&str>, bool)> = scope
        .components()
        .into_iter()
        .map(|(members, deferred_only, _)| (members, deferred_only))
        .collect();
    assert_eq!(built, expected);
    for slot in 0..shape.slots() {
        assert!(
            shape
                .component_of(Slot(slot as u32))
                .members
                .contains(&Slot(slot as u32))
        );
    }

    // Law 1: one mention at each site the model reads, with its class and binder, and no other.
    let sites = sites(statements, nodes.into_iter());
    assert_eq!(sites.mentions.len(), scope.mentions.len());
    let values = shape
        .mentions()
        .iter()
        .filter(|mention| matches!(mention.name, BinderSymbol::Value(_)));
    assert_eq!(values.count(), scope.mentions.len());
    for mention in shape.mentions() {
        if let BinderSymbol::Type(_) = mention.name {
            assert!(matches!(mention.coordinate.target, Target::Builtin(_)));
        }
    }
    for (expected, site) in scope.mentions.iter().zip(&sites.mentions) {
        let mention = shape.mention(*site).expect("a mention at the model's site");
        assert_eq!(mention.name, symbol(labels, expected.name));
        assert_eq!(mention.statement, expected.statement);
        assert_eq!(class(mention.class), expected.class);
        let found = match expected.resolved {
            Resolved::Builtin => Found::Builtin,
            Resolved::Binder { up, name } => Found::Binder {
                level: level - up,
                name,
            },
        };
        assert_eq!(follow(labels, chain, level, mention.coordinate), found);
    }

    assert_eq!(shape.nested_shapes().len(), scope.children.len());
    for (child, (site, body, statements)) in scope.children.iter().zip(sites.bodies) {
        let nested = shape.nested(site).expect("a shape at the model's body");
        let binder = scope.statement_binder[child.parent_statement as usize];
        for capture in nested.captures() {
            let fellow = binder.is_some_and(|binder| {
                scope
                    .component_of(binder)
                    .contains(text(labels, capture.name))
            });
            match capture.source {
                CaptureSource::Member { .. } => assert!(fellow, "a member capture is a fellow's"),
                CaptureSource::Read(Coordinate {
                    hops: 0,
                    target: Target::Local(_),
                }) => assert!(!fellow, "a fellow's capture is a member"),
                CaptureSource::Read(_) => {}
            }
        }
        chain.push((nested, child));
        check(
            labels,
            chain,
            statements,
            body.body_statements().map(|(node, _)| node).collect(),
        );
        chain.pop();
    }
}

// ---------- laws 4–7: activations ----------

/// What a read observes, comparable.
#[derive(PartialEq, Debug)]
enum Observed {
    Number(u64),
    Pending(CellHandle),
    Edge(u32),
}

fn observe(binding: Binding<'_, '_>) -> Observed {
    match binding {
        Binding::Bound(Value::Number(number)) => Observed::Number(number.to_bits()),
        Binding::Bound(other) => panic!("every value name is bound to a number, found {other:?}"),
        Binding::Pending(handle) => Observed::Pending(handle),
        Binding::Edge(edge) => Observed::Edge(edge.index()),
    }
}

/// What `follow` turns a capture of an enclosing knot edge into.
fn followed(index: u32) -> Value<'static, 'static> {
    Value::Number(-1.0 - f64::from(index))
}

/// Activate `shape` with every slot bound to a fresh number, check laws 4 and 6 over it, and
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

    // Law 4: by name at the read's own position lands where the coordinate does.
    for mention in shape.mentions() {
        if let BinderSymbol::Type(_) = mention.name {
            continue;
        }
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
                // Law 6: a captured value is the enclosing binding's word; a fellow is an edge.
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
            ShapeKind::Program | ShapeKind::Module => panic!("the model nests no such shape"),
        }
    }
}

/// Every path from a program to a scope reached through arms alone, as child indices.
fn block_paths(scope: &ModelScope, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    out.push(path.clone());
    for (index, child) in scope.children.iter().enumerate() {
        if child.kind == Kind::Block {
            path.push(index);
            block_paths(child, path, out);
            path.pop();
        }
    }
}

fn body() -> impl Strategy<Value = Vec<Statement>> {
    proptest::collection::vec(model::statement(), 1..3)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: crate::tests::case_share(1, 1), ..ProptestConfig::default() })]

    /// Laws 1–3: the builder refuses what the model refuses, first error first, and otherwise
    /// agrees with it on every mention, layout, component and nested shape.
    #[test]
    fn the_builder_agrees_with_the_model(program in model::program()) {
        let source = model::render_program(&program);
        let expected = model::analyze_program(&program);
        with_fixture(|fixture| {
            let lines = fixture.parse(&source);
            fixture.in_cell(|writer, _| {
                let table = builtins(fixture, writer);
                let built = Shape::of_program(fixture.program, &lines, table, fixture.scratch());
                match (built, &expected) {
                    (Err(error), Err(expected)) => {
                        assert_eq!(&model_error(fixture.labels, &error), expected, "`{source}`")
                    }
                    (Ok(shape), Ok(scope)) => {
                        check(fixture.labels, &mut vec![(shape, scope)], &program, lines.iter().collect())
                    }
                    (built, expected) => panic!(
                        "`{source}`: built {:?}, the model says {expected:?}",
                        built.map(|_| ())
                    ),
                }
            })
        });
    }

    /// Laws 4 and 6: over every activation of a shaped program, by-name resolution agrees with the
    /// coordinates, a local is visible exactly where its position is seen, and closure bindings
    /// copy the enclosing words.
    #[test]
    fn activations_read_what_their_coordinates_name(program in model::shaped_program()) {
        let source = model::render_program(&program);
        with_fixture(|fixture| {
            let lines = fixture.parse(&source);
            fixture.in_cell(|writer, _| {
                let table = builtins(fixture, writer);
                let shape = Shape::of_program(fixture.program, &lines, table, fixture.scratch())
                    .expect("the model shapes the program");
                activate(writer, table, shape, ClosureBindings::EMPTY, None, &mut 1.0);
            })
        });
    }

    /// Law 5: a claimed slot reads as its binder until bound, and a callable is born only once
    /// every slot it reads is bound.
    #[test]
    fn a_pending_slot_names_its_binder_and_refuses_a_birth(
        program in model::shaped_program(),
        bound in proptest::collection::vec(any::<bool>(), 8),
    ) {
        let source = model::render_program(&program);
        with_fixture(|fixture| {
            let lines = fixture.parse(&source);
            fixture.in_cell(|writer, handles| {
                let table = builtins(fixture, writer);
                let shape = Shape::of_program(fixture.program, &lines, table, fixture.scratch())
                    .expect("the model shapes the program");
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
                    assert_eq!(born.err(), first_pending, "`{source}`");
                }
            })
        });
    }

    /// Law 7: an `EVAL` body read at a statement of a program or of an arm inside it shapes as the
    /// model analyses it over that scope's chain, and each of its outer reads is the site's own
    /// by-name read.
    #[test]
    fn an_eval_body_resolves_over_its_site(
        program in model::shaped_program(),
        body in body(),
        scope_pick in any::<Index>(),
        statement_pick in any::<Index>(),
    ) {
        let source = model::render_program(&program);
        let quoted = format!("#{}", model::render_body(&body));
        let program_scope = model::analyze_program(&program).expect("the model shapes the program");
        let mut paths = Vec::new();
        block_paths(&program_scope, &mut Vec::new(), &mut paths);
        let path = scope_pick.get(&paths);
        with_fixture(|fixture| {
            let lines = fixture.parse(&source);
            let eval_lines = fixture.parse(&quoted);
            let ExpressionPart::QuotedExpression(quote) = eval_lines[0].parts[0].value else {
                panic!("`{quoted}` is a quote");
            };
            let quote = quote.reference();
            fixture.in_cell(|writer, _| {
                let table = builtins(fixture, writer);
                let program_shape =
                    Shape::of_program(fixture.program, &lines, table, fixture.scratch())
                        .expect("the model shapes the program");

                // Walk down the path, activating each scope beside the one it sits in.
                let mut next = 1.0;
                let mut bind_all = |activation: &Activation<'_, '_>| {
                    for slot in 0..activation.shape().slots() {
                        activation.bind(Slot(slot as u32), Value::Number(next)).unwrap();
                        next += 1.0;
                    }
                };
                let mut site = resident(
                    writer,
                    Activation::new(writer, program_shape, ClosureBindings::EMPTY, table, None),
                );
                bind_all(site);
                let mut chain: Vec<(&Shape<'_>, &ModelScope)> = vec![(program_shape, &program_scope)];
                let mut statements: &[Statement] = &program;
                let mut nodes: Vec<&KExpression<'_>> = lines.iter().collect();
                for &step in path {
                    let (shape, scope) = *chain.last().unwrap();
                    let found = sites(statements, nodes.into_iter());
                    let (at, body_node, body_statements) = found.bodies[step];
                    let nested = shape.nested(at).unwrap();
                    site = resident(
                        writer,
                        Activation::new(writer, nested, ClosureBindings::EMPTY, table, Some(site)),
                    );
                    bind_all(site);
                    chain.push((nested, &scope.children[step]));
                    statements = body_statements;
                    nodes = body_node.body_statements().map(|(node, _)| node).collect();
                }

                let (_, innermost) = *chain.last().unwrap();
                let at = statement_pick.index(innermost.statements as usize) as u32;
                let mut outer: Vec<(&ModelScope, u32)> = chain
                    .windows(2)
                    .map(|pair| (pair[0].1, pair[1].1.parent_statement))
                    .collect();
                outer.push((innermost, at));
                let expected = model::analyze_eval(&outer, &body);
                let built = Shape::for_eval(
                    fixture.program,
                    quote,
                    site,
                    Position::statement(at as usize),
                    fixture.scratch(),
                );
                match (built, &expected) {
                    (Err(error), Err(expected)) => assert_eq!(
                        &model_error(fixture.labels, &error),
                        expected,
                        "`{source}` / `{quoted}` at {path:?}:{at}"
                    ),
                    (Ok(shape), Ok(scope)) => {
                        let evaluated =
                            Activation::new(writer, shape, ClosureBindings::EMPTY, table, Some(site));
                        for mention in shape.mentions() {
                            if matches!(mention.name, BinderSymbol::Type(_))
                                || matches!(mention.coordinate, Coordinate { hops: 0, target: Target::Local(_) | Target::Capture(_) })
                            {
                                continue;
                            }
                            let by_name = site.resolve_by_name(mention.name, Position::statement(at as usize));
                            assert_eq!(by_name.map(observe), Some(observe(evaluated.read(mention.coordinate))));
                        }
                        chain.push((shape, scope));
                        check(
                            fixture.labels,
                            &mut chain,
                            &body,
                            quote.body_statements().map(|(node, _)| node).collect(),
                        );
                    }
                    (built, expected) => panic!(
                        "`{source}` / `{quoted}` at {path:?}:{at}: built {:?}, the model says {expected:?}",
                        built.map(|_| ())
                    ),
                }
            })
        });
    }
}
