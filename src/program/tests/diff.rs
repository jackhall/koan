//! The **walk diff**: under `walk-diff` every load runs both walks — the one walk into records,
//! then the old passes into the cells — and compares the two backends of [`LoadFacts`] through the
//! read door, site for site ([the diff](../../../roadmap/gradual-build/one-walk-build.md)). A
//! divergence fails the test that loaded the program; one a named move explains is counted and not
//! reported.
//!
//! Types compare by their rendered names, which name a lexical variable by its name rather than
//! its level, so numbering differences do not show.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::scope::{
    BodyShape, InstanceRead, LoadFacts, Mint, Narrowing, Resolved, Site, Slot, Static,
    StaticCallable, StaticRegistered, StaticSolution, StaticType, UseStatics, each_part,
};
use crate::symbols::SymbolInterner;
use crate::type_lattice::{DeclaredType, Interval, Parametric, display_name};
use crate::values::Chain;

/// Every divergence between `cells`, the old passes' facts, and `records`, the one walk's: one
/// line each, naming the body, the site, the accessor and both renderings.
pub(crate) fn diff(
    cells: &LoadFacts<'_>,
    records: &LoadFacts<'_>,
    types: &Chain<'_, '_, '_, '_>,
    symbols: &SymbolInterner,
) -> Vec<String> {
    let render = Render { types, symbols };
    let mut out = Differ {
        lines: Vec::new(),
        moves: BTreeMap::new(),
        shapes: cells.shapes(),
    };
    for shape in cells.shapes().bodies() {
        // No reader reads a refused quote's code but its refusal, which the code itself holds.
        let root = cells
            .shapes()
            .shape_at(cells.shapes().chain_root(shape.segment()));
        let refused = |facts: &LoadFacts<'_>| facts.typing_refusal(root).is_some();
        if root.kind() == crate::scope::ShapeKind::Code && (refused(records) || refused(cells)) {
            if !refused(cells) {
                out.moved(QUOTE);
            }
            continue;
        }
        body(&mut out, &render, cells, records, shape);
    }
    for (kind, count) in out.moves.iter() {
        eprintln!("walk diff: {count} × {kind} (a named move)");
    }
    out.lines
}

/// Whether every element of `inner` is one of `outer`'s.
fn within<T: PartialEq>(inner: &[T], outer: &[T]) -> bool {
    inner.iter().all(|each| outer.contains(each))
}

struct Differ<'graph> {
    lines: Vec<String>,
    moves: BTreeMap<&'static str, usize>,
    shapes: &'graph crate::scope::Shapes<'graph>,
}

/// The named moves the diff counts rather than reports ([D10](../../../roadmap/gradual-build/one-walk-build.md)).
const QUOTE: &str = "a quote reads its `$` names at their static types";
const CLOSED_PATH: &str = "a type over a closed path is closed";
const MIXED_SEAM: &str = "a unit mixing an entry seam with a later one keys nothing through it";
const KEY_GROWTH: &str = "a callable keys on each keyable name a unit beneath it reads";
const LAYOUT: &str = "a closure carries every enclosing type name bound at its birth";
const TAIL_EDGE: &str = "a tail edge into an untyped registration pairs by its written slots";

impl Differ<'_> {
    /// Count a divergence `kind` explains.
    fn moved(&mut self, kind: &'static str) {
        *self.moves.entry(kind).or_default() += 1;
    }

    /// Whether `shape` lies on a quote's code's chain.
    fn in_code(&self, shape: &BodyShape<'_>) -> bool {
        let root = self.shapes.chain_root(shape.segment());
        self.shapes.shape_at(root).kind() == crate::scope::ShapeKind::Code
    }

    fn check(&mut self, shape: &BodyShape<'_>, at: &str, what: &str, old: String, new: String) {
        if old == new {
            return;
        }
        // Typing a quote's code at its birth unit reads its `$` names typed: any of its facts may
        // move, its refusal included.
        if self.in_code(shape) {
            return self.moved(QUOTE);
        }
        // A type the old channel left rigid over a path the walk resolves closed is closed.
        if old.starts_with("rigid ") && !new.starts_with("rigid ") && new != "unknown" {
            return self.moved(CLOSED_PATH);
        }
        {
            self.lines.push(format!(
                "segment {} ({:?}{}) {at} {what}: cells {old} / records {new}",
                shape.segment().index(),
                shape.kind(),
                match self.in_code(shape) {
                    true => " in code",
                    false => "",
                },
            ));
        }
    }
}

struct Render<'e, 'tier, 'pair, 'reg, 'door> {
    types: &'e Chain<'tier, 'pair, 'reg, 'door>,
    symbols: &'e SymbolInterner,
}

impl Render<'_, '_, '_, '_, '_> {
    fn ty(&self, ktype: impl Into<DeclaredType<Parametric>>) -> String {
        display_name(ktype, self.types, self.symbols).to_string()
    }

    fn interval(&self, typed: Interval) -> String {
        format!("[{}, {}]", self.ty(typed.lower), self.ty(typed.upper))
    }

    fn bound(&self, bound: DeclaredType<Interval>) -> String {
        match bound {
            DeclaredType::Type(typed) => self.interval(typed),
            DeclaredType::Scheme(scheme) => self.ty(scheme),
        }
    }

    fn static_type(&self, typed: StaticType<'_>) -> String {
        match typed {
            Static::Unknown => "unknown".into(),
            Static::Closed(closed) => self.ty(closed),
            Static::Rigid { value, .. } => format!("rigid {}", self.ty(value)),
        }
    }

    fn callable(&self, typed: StaticCallable<'_>) -> String {
        match typed {
            Static::Unknown => "unknown".into(),
            Static::Closed(callable) => self.ty(callable.ktype),
            Static::Rigid { value, .. } => format!("rigid {}", self.ty(value.ktype)),
        }
    }

    fn registered(&self, typed: StaticRegistered<'_>) -> String {
        match typed {
            Static::Unknown => "unknown".into(),
            Static::Closed(registered) => self.ty(registered.shape),
            Static::Rigid { value, .. } => format!("rigid {}", self.ty(value.shape)),
        }
    }

    fn solution(&self, solution: StaticSolution<'_>) -> String {
        match solution {
            StaticSolution::Closed(closed) => {
                let each: Vec<_> = closed.iter().map(|each| self.ty(*each)).collect();
                each.join(", ")
            }
            StaticSolution::Rigid { value, .. } => {
                let each: Vec<_> = value.iter().map(|each| self.ty(*each)).collect();
                format!("rigid {}", each.join(", "))
            }
        }
    }

    fn uses(&self, fixed: UseStatics<'_>) -> String {
        let mut out = match fixed.narrowing {
            Narrowing::Full => "full".to_string(),
            Narrowing::Kept(kept) => format!("kept {kept:?}"),
            Narrowing::Selected(at) => format!("selected {at:?}"),
        };
        for each in fixed.contributions {
            let _ = write!(out, " / {}", self.static_type(*each));
        }
        out
    }
}

/// Whether `shape` is born of a registration the old passes typed `Unknown`.
fn untyped_registration(cells: &LoadFacts<'_>, shape: &BodyShape<'_>) -> bool {
    let shapes = cells.shapes();
    let Some((parent, _)) = shapes.parent(shape.segment()) else {
        return false;
    };
    let parent = shapes.shape_at(parent);
    (parent.registrations().iter()).any(|registration| {
        parent
            .births(registration.slot)
            .is_some_and(|born| born.segment() == shape.segment())
            && matches!(
                cells.registered_type(parent, registration.slot),
                Static::Unknown
            )
    })
}

/// Compare every fact of `shape` the two backends hold.
fn body(
    out: &mut Differ<'_>,
    render: &Render<'_, '_, '_, '_, '_>,
    cells: &LoadFacts<'_>,
    records: &LoadFacts<'_>,
    shape: &BodyShape<'_>,
) {
    let untyped = untyped_registration(cells, shape);
    for index in 0..shape.slots() {
        let slot = Slot(index as u32);
        let at = format!("slot {index}");
        // A type binder over a path the walk resolves closed declares closed where the old
        // channel left it unknown.
        let mut over_path = false;
        if let Some(declaration) = shape.declarations(slot) {
            each_part(declaration, |part| {
                over_path |= (shape.spelled().iter()).any(|path| std::ptr::eq(path.part, part));
            });
            let (old, new) = (
                cells.declared_type(shape, slot),
                records.declared_type(shape, slot),
            );
            match (old, new) {
                (Static::Unknown, Static::Closed(_)) if over_path => out.moved(CLOSED_PATH),
                _ => out.check(
                    shape,
                    &at,
                    "declared",
                    render.static_type(old),
                    render.static_type(new),
                ),
            }
        }
        if shape.registration(slot).is_some() {
            let (old, new) = (
                cells.registered_type(shape, slot),
                records.registered_type(shape, slot),
            );
            out.check(
                shape,
                &at,
                "registered",
                render.registered(old),
                render.registered(new),
            );
        }
        let (old, new) = (
            cells.binder_type(shape, slot),
            records.binder_type(shape, slot),
        );
        let (old, new) = (
            format!("{:?}", old.map(|bound| render.bound(bound))),
            format!("{:?}", new.map(|bound| render.bound(bound))),
        );
        match over_path && old == r#"Some("[Never, Type]")"# {
            true if old != new => out.moved(CLOSED_PATH),
            _ => out.check(shape, &at, "binder", old, new),
        }
    }
    for index in 0..shape.body().len() {
        let (old, new) = (
            cells.statement_type(shape, index),
            records.statement_type(shape, index),
        );
        out.check(
            shape,
            &format!("statement {index}"),
            "type",
            format!("{:?}", old.map(|typed| render.interval(typed))),
            format!("{:?}", new.map(|typed| render.interval(typed))),
        );
    }
    for expression in shape.type_expressions() {
        let site = expression.site;
        let (old, new) = (
            cells.typed_expression(shape, site),
            records.typed_expression(shape, site),
        );
        out.check(
            shape,
            &format!("expression {}", expression.order),
            "typed",
            render.static_type(old),
            render.static_type(new),
        );
    }
    for (index, (site, _)) in shape.candidate_lists().iter().enumerate() {
        let (old, new) = (
            cells.use_statics(shape, *site),
            records.use_statics(shape, *site),
        );
        out.check(
            shape,
            &format!("use {index}"),
            "statics",
            render.uses(old),
            render.uses(new),
        );
    }
    let mut sites = Vec::new();
    for node in shape.body() {
        sites.push(Site::of_node(node));
        each_part(node, |part| {
            sites.push(Site::of(part));
            if let crate::parse::ExpressionPart::Expression(inner) = part {
                sites.push(Site::of_node(inner.reference()));
            }
        });
    }
    for (index, site) in sites.iter().enumerate() {
        let at = format!("part {index}");
        let (old, new) = (
            cells.value_type(shape, *site),
            records.value_type(shape, *site),
        );
        out.check(
            shape,
            &at,
            "value",
            format!("{:?}", old.map(|typed| render.interval(typed))),
            format!("{:?}", new.map(|typed| render.interval(typed))),
        );
        let (old, new) = (cells.settled(shape, *site), records.settled(shape, *site));
        out.check(shape, &at, "settled", old.to_string(), new.to_string());
        let read = |read: Option<InstanceRead<'_>>| match read {
            None => "none".to_string(),
            Some(InstanceRead::AsIs) => "as is".to_string(),
            Some(InstanceRead::Solved(solution)) => render.solution(solution),
        };
        let (old, new) = (
            cells.instance_at(shape, *site),
            records.instance_at(shape, *site),
        );
        out.check(shape, &at, "instance", read(old), read(new));
        let named = |named: &[StaticType<'_>]| {
            let each: Vec<_> = named.iter().map(|each| render.static_type(*each)).collect();
            each.join(", ")
        };
        let (old, new) = (
            cells.named_contributions(shape, *site),
            records.named_contributions(shape, *site),
        );
        out.check(shape, &at, "named", named(old), named(new));
        let (old, new) = (cells.mints_at(shape, *site), records.mints_at(shape, *site));
        out.check(shape, &at, "mints", old.to_string(), new.to_string());
        let (old, new) = (cells.mint_at(shape, *site), records.mint_at(shape, *site));
        match (old, new) {
            (Some(Mint::Varying), Some(Mint::Invariant)) if untyped => out.moved(TAIL_EDGE),
            _ => out.check(shape, &at, "mint", format!("{old:?}"), format!("{new:?}")),
        }
        let (old, new) = (cells.warm_at(shape, *site), records.warm_at(shape, *site));
        out.check(shape, &at, "warm", old.to_string(), new.to_string());
        let (old, new) = (
            cells.view_tree(shape, *site),
            records.view_tree(shape, *site),
        );
        out.check(
            shape,
            &at,
            "view tree",
            format!("{:?}", old.map(|tree| tree.print())),
            format!("{:?}", new.map(|tree| tree.print())),
        );
    }
    // The layout holds every enclosing type name bound at the birth; what the content composes of
    // it is what the old passes captured.
    let (old, new) = (cells.type_captures(shape), records.type_captures(shape));
    let composed: Vec<_> = (new.iter().zip(records.contract(shape)))
        .filter(|(_, composes)| **composes)
        .map(|(capture, _)| *capture)
        .collect();
    match old == new || (within(&composed, old) && within(old, &composed)) {
        true if old != new => out.moved(LAYOUT),
        true => {}
        false => out.check(
            shape,
            "body",
            "type captures",
            format!("{old:?}"),
            format!("{composed:?} of {new:?}"),
        ),
    }
    let (old, new) = (cells.digested_count(shape), records.digested_count(shape));
    out.check(shape, "body", "digested", old.to_string(), new.to_string());
    for index in 0..shape.slots() {
        let slot = Slot(index as u32);
        let (old, new) = (
            cells.digested_index(shape, slot),
            records.digested_index(shape, slot),
        );
        out.check(
            shape,
            &format!("slot {index}"),
            "digested at",
            format!("{old:?}"),
            format!("{new:?}"),
        );
    }
    for (index, site) in sites.iter().enumerate() {
        let (old, new) = (
            cells.view_slots(shape, *site),
            records.view_slots(shape, *site),
        );
        out.check(
            shape,
            &format!("part {index}"),
            "view slots",
            format!("{old:?}"),
            format!("{new:?}"),
        );
        let (old, new) = (cells.handoffs(shape, *site), records.handoffs(shape, *site));
        out.check(
            shape,
            &format!("part {index}"),
            "handoffs",
            format!("{old:?}"),
            format!("{new:?}"),
        );
    }
    let (old, new) = (cells.class(shape), records.class(shape));
    out.check(
        shape,
        "body",
        "class",
        format!("{old:?}"),
        format!("{new:?}"),
    );
    let (old, new) = (cells.caches(shape), records.caches(shape));
    out.check(shape, "body", "caches", old.to_string(), new.to_string());
    // The old passes gave up on such a body's tail edges: its parameters varied and keyed nothing.
    let caching = |caching: Option<&crate::scope::Caching<'_>>, moved: bool| {
        caching.map(|caching| {
            format!(
                "invariant {:?} keyable {:?} escapes {} mints {:?} warm {:?}",
                caching.invariant,
                match moved {
                    true => &[],
                    false => caching.keyable,
                },
                caching.escapes,
                (caching.mints.iter())
                    .map(|(_, mint)| match (moved, *mint) {
                        (true, Mint::Varying) => Mint::Invariant,
                        (_, mint) => mint,
                    })
                    .collect::<Vec<_>>(),
                caching.warm.len(),
            )
        })
    };
    let (old, new) = (
        format!("{:?}", caching(cells.caching(shape), false)),
        format!("{:?}", caching(records.caching(shape), false)),
    );
    let moved = (
        format!("{:?}", caching(cells.caching(shape), true)),
        format!("{:?}", caching(records.caching(shape), true)),
    );
    match untyped && old != new && moved.0 == moved.1 {
        true => out.moved(TAIL_EDGE),
        false => out.check(shape, "body", "caching", old, new),
    }
    if shape.kind() == crate::scope::ShapeKind::Callable {
        let (old, new) = (cells.callable_type(shape), records.callable_type(shape));
        out.check(
            shape,
            "body",
            "callable",
            render.callable(old),
            render.callable(new),
        );
        let levels = |levels: &[Parametric]| {
            let each: Vec<_> = levels.iter().map(|each| render.ty(*each)).collect();
            each.join(", ")
        };
        let (old, new) = (cells.group_levels(shape), records.group_levels(shape));
        out.check(shape, "body", "group levels", levels(old), levels(new));
        let (old, new) = (cells.born_instance(shape), records.born_instance(shape));
        out.check(
            shape,
            "body",
            "born instance",
            format!("{:?}", old.map(|solution| render.solution(solution))),
            format!("{:?}", new.map(|solution| render.solution(solution))),
        );
        let named = |keys: Option<&[Slot]>| {
            keys.map(|keys| {
                let each: Vec<_> = (keys.iter())
                    .map(|slot| render.symbols.resolve(shape.slot_name(*slot).symbol()))
                    .collect();
                format!("{each:?}")
            })
        };
        let (old, new) = (
            cells.specialization_keys(shape),
            records.specialization_keys(shape),
        );
        let (some, held) = (old.unwrap_or(&[]), new.unwrap_or(&[]));
        match (old == new, within(held, some), within(some, held)) {
            (true, ..) => {}
            (false, true, _) => out.moved(MIXED_SEAM),
            (false, _, true) => out.moved(KEY_GROWTH),
            _ => {
                let (old, new) = (format!("{:?}", named(old)), format!("{:?}", named(new)));
                out.check(shape, "body", "keys", old, new);
            }
        }
    }
    if shape.kind() == crate::scope::ShapeKind::Code {
        let refusal = |refusal: Option<&crate::scope::ShapeError<'_>>| {
            refusal.map(|error| error.display(render.symbols, render.types).to_string())
        };
        let (old, new) = (
            refusal(cells.typing_refusal(shape)),
            refusal(records.typing_refusal(shape)),
        );
        out.check(
            shape,
            "body",
            "refusal",
            format!("{old:?}"),
            format!("{new:?}"),
        );
    }
    for (index, spelled) in shape.spelled().iter().enumerate() {
        let site = Site::of(spelled.part);
        let resolved = |facts: &LoadFacts<'_>| {
            let read = facts.path_read(shape, site)?;
            let root = facts.shapes().shape_at(read.root);
            Some(match facts.resolution(root, read.level)?.resolved {
                Resolved::Closed(closed) => render.ty(closed),
                Resolved::Class(_) => "class".to_string(),
            })
        };
        // A spelling the old channel never read in `Typing` mode records no read there.
        if let (Some(old), Some(new)) = (resolved(cells), resolved(records)) {
            out.check(shape, &format!("path {index}"), "resolved", old, new);
        }
    }
}
