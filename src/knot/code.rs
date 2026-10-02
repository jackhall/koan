//! A quote's code as a knot node: the quote's body as written, the code shape the program built
//! for it where it loads, its carried type, and the bindings its code carries — each `$` name's
//! binding, and each hole a `USING` filled — both runs sorted by name.
//!
//! A quote is born where it is evaluated through the quote door, [`quote`], as a one-node knot
//! whose `$` names are read through the activation the quote sits in. A quote a data member holds
//! whose `$` name reads a fellow member is instead a node of the member's knot, staged here and
//! laid down by the [tie](super::tie()), that mention an edge. [`using`] fills holes from a record
//! or a module into a new one-node knot: a name hole with the member it names, and a keyworded hole
//! with the list of a module's registrations at its key. A hole or a `\` mark names no binding, so
//! neither is held here: a hole is filled only by `USING`, and a `\` name by the `EVAL` that runs
//! the code.

use crate::memory::{BumpAllocator, BumpVec, Edge, KnotPlan, Writer, collect, resident};
use crate::parse::{ExpressionPart, ProgramNode};
use crate::scope::{BodyShape, CaptureSource, ShapeKind, Site};
use crate::symbols::{BinderSymbol, KeySymbol};
use crate::type_lattice::{DispatchTokenElement, KType, TypeNode, TypeRegistry};
use crate::values::{CodeView, Link, List, Value, Weight};

use super::{KActivationView, KValue, Knotted, Node};

/// One name the code binds, beside its binding.
pub type Binding<'graph, 'cell> = (BinderSymbol, Link<'cell, Knotted<'graph, 'cell>>);

/// A quote's code: what one knot node points at, homed beside it for the reason
/// [`Node::Coerced`] is.
pub struct Code<'graph, 'cell> {
    body: ProgramNode<'graph>,
    shape: &'graph BodyShape<'graph>,
    ktype: KType,
    /// Each `$` name's binding, sorted by name.
    bound: &'cell [Binding<'graph, 'cell>],
    /// Each hole a `USING` filled, sorted by name.
    supplied: &'cell [Binding<'graph, 'cell>],
    /// What rebuilding the whole knot this node sits in writes, the same on every node.
    knot_weight: Weight,
}

impl Clone for Code<'_, '_> {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for Code<'_, '_> {}

impl<'graph, 'cell> Code<'graph, 'cell> {
    /// The quote's body as written.
    pub fn body(&self) -> ProgramNode<'graph> {
        self.body
    }

    /// The code shape the program built for this quote where it loads.
    pub fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    /// The carried type: the code's kind, needing the `\` names no binder in the code fills.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn bound(&self) -> &'cell [Binding<'graph, 'cell>] {
        self.bound
    }

    pub fn supplied(&self) -> &'cell [Binding<'graph, 'cell>] {
        self.supplied
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight
    }

    /// What `values` reads of this code.
    pub(super) fn view(&self) -> CodeView<'cell, Knotted<'graph, 'cell>> {
        CodeView {
            body: self.body.reference(),
            bound: self.bound,
            supplied: self.supplied,
        }
    }

    /// Every value this code's bindings hold, bound then supplied, in the order
    /// [`rebuilt`](Self::rebuilt) asks for them.
    pub(super) fn held(&self, out: &mut dyn FnMut(KValue<'graph, 'cell>)) {
        for (_, link) in self.bound.iter().chain(self.supplied) {
            if let Link::Value(value) = link {
                out(*value);
            }
        }
    }

    /// This code rebuilt in `writer`'s region — the copy's arm: each binding's value through `copy`,
    /// each edge verbatim, and the body, shape, type and knot weight carried over.
    pub(super) fn rebuilt<'to>(
        &self,
        writer: Writer<'to>,
        mut copy: impl FnMut(&KValue<'graph, 'cell>) -> KValue<'graph, 'to>,
    ) -> Code<'graph, 'to> {
        let mut run = |source: &'cell [Binding<'graph, 'cell>]| {
            writer.fill(source.len(), |at| {
                let (name, link) = &source[at];
                (*name, link.copied(&mut copy))
            })
        };
        let bound = run(self.bound);
        let supplied = run(self.supplied);
        Code {
            body: self.body,
            shape: self.shape,
            ktype: self.ktype,
            bound,
            supplied,
            knot_weight: self.knot_weight,
        }
    }
}

/// What a run of bindings writes: the run and what each value points at.
fn run_weight(run: &[Binding<'_, '_>]) -> Weight {
    run.iter().fold(
        Weight::run::<Binding<'_, '_>>(run.len()),
        |weight, (_, link)| weight.plus(link.referent_weight()),
    )
}

/// A code node, read and not yet written.
pub(super) struct Staged<'graph, 'cell, 'x> {
    pub body: ProgramNode<'graph>,
    pub shape: &'graph BodyShape<'graph>,
    pub bound: BumpVec<'x, Binding<'graph, 'cell>>,
}

impl<'graph, 'cell> Staged<'graph, 'cell, '_> {
    /// This node's bound run laid down in `writer`'s region, beside what it and the resident code
    /// add to the knot's weight.
    pub(super) fn laid_down(
        &self,
        writer: Writer<'cell>,
    ) -> (&'cell [Binding<'graph, 'cell>], Weight) {
        let bound = collect(writer, self.bound.iter().copied());
        let weight = run_weight(bound).plus(Weight::flat::<Code<'graph, 'cell>>());
        (bound, weight)
    }

    /// The resident code over `bound`, in a knot weighing `knot_weight`.
    pub(super) fn tied(
        &self,
        writer: Writer<'cell>,
        bound: &'cell [Binding<'graph, 'cell>],
        knot_weight: Weight,
    ) -> Node<'graph, 'cell> {
        Node::Code(resident(
            writer,
            Code {
                body: self.body,
                shape: self.shape,
                ktype: self.shape.code_type(),
                bound,
                supplied: &[],
                knot_weight,
            },
        ))
    }
}

/// The code shape of the quote `part`, if `part` is one `shape` holds.
fn shape_of<'graph>(
    shape: &BodyShape<'graph>,
    part: &ExpressionPart<'graph>,
) -> Option<(ProgramNode<'graph>, &'graph BodyShape<'graph>)> {
    let ExpressionPart::QuotedExpression(body) = part else {
        return None;
    };
    let code = shape.nested(Site::of(part))?;
    (code.kind() == ShapeKind::Code).then_some((*body, code))
}

/// Read the quote `body` of code shape `shape` into `scratch`: each `$` name's binding read through
/// `activation`, each `Member` source minted by `edge`.
pub(super) fn staged<'graph, 'cell, 'x>(
    body: ProgramNode<'graph>,
    shape: &'graph BodyShape<'graph>,
    activation: &KActivationView<'graph, 'cell>,
    scratch: BumpAllocator<'x>,
    mut edge: impl FnMut(u32) -> Edge,
) -> Staged<'graph, 'cell, 'x> {
    let mut bound = BumpVec::new_in(scratch);
    for capture in shape.captures() {
        let link = match capture.source {
            CaptureSource::Read(coordinate) => Link::Value(activation.read(coordinate)),
            CaptureSource::Member { index, .. } => Link::Edge(edge(index)),
            CaptureSource::Hole | CaptureSource::Offered => continue,
        };
        bound.push((capture.name, link));
    }
    bound.sort_unstable_by_key(|(name, _)| *name);
    Staged { body, shape, bound }
}

/// Lay `code` down as a one-node knot in `writer`'s region, its runs holding value words only.
fn one_node<'graph, 'cell>(
    writer: Writer<'cell>,
    body: ProgramNode<'graph>,
    shape: &'graph BodyShape<'graph>,
    bound: &[Binding<'graph, 'cell>],
    supplied: &[Binding<'graph, 'cell>],
) -> Knotted<'graph, 'cell> {
    let bound = collect(writer, bound.iter().copied());
    let supplied = collect(writer, supplied.iter().copied());
    let knot_weight = Weight::flat::<usize>()
        .plus(Weight::flat::<Node<'graph, 'cell>>())
        .plus(Weight::flat::<Code<'graph, 'cell>>())
        .plus(run_weight(bound))
        .plus(run_weight(supplied));
    let knot = KnotPlan::new(1).tie(writer, |_| {
        Node::Code(resident(
            writer,
            Code {
                body,
                shape,
                ktype: shape.code_type(),
                bound,
                supplied,
                knot_weight,
            },
        ))
    });
    Knotted::of(knot, 0)
}

/// The quote door: birth the quote `part` that `activation`'s shape holds as a one-node knot in
/// `writer`'s region, each `$` name read through `activation`. The shape was built where the
/// program loaded, so nothing is refused here: an error in the code waits for the `EVAL` that runs
/// it.
///
/// A quote whose `$` name reads a fellow member is born inside its binder's knot by the tie, which
/// never asks for it, so no binding here is an edge.
pub fn quote<'graph, 'cell>(
    writer: Writer<'cell>,
    activation: &KActivationView<'graph, 'cell>,
    part: &'graph ExpressionPart<'graph>,
    scratch: BumpAllocator<'_>,
) -> Knotted<'graph, 'cell> {
    let (body, shape) = shape_of(activation.shape(), part)
        .expect("a quote is born at the site of a code shape its shape holds");
    let staged = staged(body, shape, activation, scratch, |_| {
        unreachable!("only the tie births a quote that reads a fellow member")
    });
    one_node(writer, body, shape, &staged.bound, &[])
}

/// Why a `USING` refused to fill a code's holes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsingRefused {
    /// The module's registrations at `key` rank its slots other than the code's own candidates at
    /// `key` do.
    Ranking { key: KeySymbol },
}

/// `code USING source`: a new one-node knot in `writer`'s region whose holes `source` fills, and
/// whose other holes stay holes. A name hole takes the field of `source` — a record, or a module's
/// member — that it names; a keyworded hole takes the list of a module's registrations at its key,
/// and stays open where the module has none. A field naming no hole is ignored, and a hole an
/// earlier `USING` filled is no hole, so it is never rebound. Code whose shape carries a refusal
/// comes back unchanged: its holes are unknown, and the `EVAL` that runs it reports the refusal.
/// Each binding of `code` that is an edge is resolved to the member it names, so the new knot's
/// runs hold value words only.
///
/// Refused, writing nothing, where a module's registrations at a hole's key rank their slots other
/// than the code's own candidates at the key do.
pub fn using<'graph, 'cell>(
    writer: Writer<'cell>,
    code: Knotted<'graph, 'cell>,
    source: KValue<'graph, 'cell>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<Knotted<'graph, 'cell>, UsingRefused> {
    let node = code
        .code()
        .expect("`USING` fills the holes of a quote's code");
    let shape = node.shape();
    if shape.refusal().is_some() {
        return Ok(code);
    }
    let resolved = |(name, link): &Binding<'graph, 'cell>| (*name, Link::Value(link.resolve(code)));
    let mut supplied: BumpVec<'_, Binding<'graph, 'cell>> = BumpVec::new_in(scratch);
    supplied.extend(node.supplied().iter().map(resolved));
    let before = supplied.len();
    let mut keyed = BumpVec::new_in(scratch);
    for capture in shape.captures() {
        if capture.source != CaptureSource::Hole
            || node
                .supplied()
                .iter()
                .any(|(name, _)| *name == capture.name)
        {
            continue;
        }
        let value = match capture.name {
            BinderSymbol::Key(key) => {
                keyed.clear();
                self::keyed(shape, key, source, types, scratch, &mut keyed)?;
                (!keyed.is_empty())
                    .then(|| Value::List(List::of_candidates(writer, keyed.iter().copied())))
            }
            name => super::field(writer, source, name, types, scratch),
        };
        if let Some(value) = value {
            supplied.push((capture.name, Link::Value(value)));
        }
    }
    if supplied.len() == before {
        return Ok(code);
    }
    supplied.sort_unstable_by_key(|(name, _)| *name);
    let mut bound = BumpVec::with_capacity_in(node.bound().len(), scratch);
    bound.extend(node.bound().iter().map(resolved));
    Ok(one_node(writer, node.body(), shape, &bound, &supplied))
}

/// Push onto `found` each registration `source` holds whose registered shape's key is `key`. A
/// function's key and ranking are read off its registered shape; a ranking other than the one
/// `shape`'s own candidates at `key` carry is refused.
fn keyed<'graph, 'cell>(
    shape: &BodyShape<'graph>,
    key: KeySymbol,
    source: KValue<'graph, 'cell>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    found: &mut BumpVec<'_, KValue<'graph, 'cell>>,
) -> Result<(), UsingRefused> {
    for member in super::registrations(source, types, scratch) {
        let registered = member
            .as_callable()
            .and_then(Knotted::function)
            .and_then(|function| function.registered_shape())
            .expect("a registration member is the function born for it");
        let TypeNode::ExpressionShape {
            elements, classes, ..
        } = types.node(registered)
        else {
            unreachable!("a registered shape is an expression shape")
        };
        let run = elements.iter().map(|element| match element {
            DispatchTokenElement::Keyword(keyword) => Some(*keyword),
            DispatchTokenElement::Slot(_) => None,
        });
        if KeySymbol::of(run.clone()) != key {
            continue;
        }
        // The lattice stores written order as no classes; the shape spells every class out.
        let ranking = if classes.is_empty() {
            let slots = run.filter(Option::is_none).count();
            scratch.alloc_slice_fill_iter((0..slots).map(|class| class as u8))
        } else {
            classes
        };
        if !shape.ranks_alike(key, ranking) {
            return Err(UsingRefused::Ranking { key });
        }
        found.push(*member);
    }
    Ok(())
}
