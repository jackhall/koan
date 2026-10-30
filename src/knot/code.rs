//! A quote's code as a knot node: its syntax and where its code shape was built — written syntax
//! in program storage, shaped where the program loads, by segment, or composed syntax laid in a
//! region, shaped in its composition, under its root code value ([`CodeSyntax`]) — its content
//! digest, its carried type, and the bindings its code carries — each `$` name's and `$(…)`
//! registration's, and each hole a `USING` filled — both runs of [`Binding`]s sorted by scope, then
//! name. A reader that needs the shape reads it by segment through its step's door. A copy lays
//! composed syntax again in its destination region; written syntax it points at.
//!
//! A quote is born where it is evaluated through the quote door, [`quote`], as a one-node knot
//! whose `$` names are read through the activation the quote sits in. A quote a data member holds
//! whose `$` name reads a fellow member is instead a node of the member's knot, staged here and
//! laid down by the [tie](super::tie()), that mention an edge. [`using`] fills holes from a record
//! or a module into a new one-node knot: a name hole with the member it names, and a keyworded hole
//! with the list of a module's registrations at its key. A hole or a `\` mark names no binding, so
//! neither is held here: a hole is filled only by `USING`, and a `\` name by the `EVAL` that runs
//! the code.

use std::cell::Cell;

use crate::memory::{
    Bump, BumpAllocator, BumpVec, Edge, KnotPlan, Writer, collect, program_storage, resident,
};
use crate::parse::{ExpressionPart, KExpression, Mark, Quoted, Remark, Scope, remarked_whole};
use crate::scope::{
    self, BodyShape, CaptureSlot, CaptureSource, CodeSyntax, ComposedRoot, Facts, FilledSites,
    ShapeKind, Site, Written, filled_sites,
};
use crate::symbols::{BinderSymbol, KeySymbol, SymbolInterner};
use crate::type_lattice::{KType, TypeNode};
use crate::values::digest::{DigestHasher, Tag, syntax_digest};
use crate::values::{self, Chain, CodeView, ContentDigest, Link, List, Seen, Value, Weight};

use super::view::{self, CodeTree, ComposeRefused, Env, composed, materialized};
use super::{KActivationView, KReading, KValue, Knotted, Node};

/// One binding the code carries, at its scope and name.
pub type Binding<'graph, 'cell> = values::Binding<'cell, Knotted<'graph, 'cell>>;

/// What a code value holds of its syntax: flat syntax, beside the digest its content covers of it
/// — a written quote's code digest, as resolved where the program loads, composed code's syntax
/// digest — or a code view, laid flat by whatever reads it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Syntax<'graph, 'cell> {
    Flat {
        syntax: CodeSyntax<'graph, 'cell>,
        code: ContentDigest,
    },
    View(&'cell CodeTree<'graph, 'cell>),
}

/// A quote's code: what one knot node points at, homed beside it for the reason
/// [`Node::Coerced`] is. A view's runs are empty: its environments hold its bindings.
pub struct Code<'graph, 'cell> {
    syntax: Syntax<'graph, 'cell>,
    ktype: KType,
    /// Each `$` name's, filled mark's and `$(…)` registration's binding, sorted by scope, then
    /// name.
    bound: &'cell [Binding<'graph, 'cell>],
    /// What rebuilding the whole knot this node sits in writes, the same on every node; the root
    /// node's also carries the knot's memo bits.
    knot_weight: Cell<Weight>,
}

impl<'graph: 'cell, 'cell> Code<'graph, 'cell> {
    /// The flat syntax, or `None` for a view.
    pub fn body(&self) -> Option<&'cell KExpression<'cell>> {
        self.flat().map(CodeSyntax::body)
    }

    /// Where this code's flat syntax is and what shapes it, or `None` for a view.
    pub fn flat(&self) -> Option<CodeSyntax<'graph, 'cell>> {
        match self.syntax {
            Syntax::Flat { syntax, .. } => Some(syntax),
            Syntax::View(_) => None,
        }
    }

    /// The view this code is, if it is one.
    pub(crate) fn view(&self) -> Option<&'cell CodeTree<'graph, 'cell>> {
        match self.syntax {
            Syntax::View(tree) => Some(tree),
            Syntax::Flat { .. } => None,
        }
    }

    /// The code shape this code runs, read through `door`: a written quote's by its segment in the
    /// load's table, composed code's in its composition's, which `door` must rest on. A view is
    /// laid flat before anything reads its shape.
    pub fn shape<'step>(&self, door: &scope::Code<'step>) -> &'step BodyShape<'step> {
        match self
            .flat()
            .expect("a view is laid flat before its shape is read")
        {
            CodeSyntax::Written { segment, .. }
            | CodeSyntax::Composed {
                segment: Some(segment),
                ..
            } => door.shape(segment),
            CodeSyntax::Composed { segment: None, .. } => door.shapes().root(),
        }
    }

    /// The carried type: the code's kind, needing the `\` names no binder in the code fills, or the
    /// retype of its binder's annotation where the tie births it at one; a view's kind unbuilt. A
    /// header over the node carries any other retype; `USING` builds code typed by its own syntax.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn bound(&self) -> &'cell [Binding<'graph, 'cell>] {
        self.bound
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight.get()
    }

    /// The knot weight's cell, where a knot's root node keeps its memo bits.
    pub(super) fn knot_weight_cell(&self) -> &Cell<Weight> {
        &self.knot_weight
    }

    /// The values the node's content covers, in the order [`content`](Self::content) asks for
    /// their digests: each binding that holds a value, not an edge — a view's [`listed`](view::listed).
    pub(super) fn content_parts<'a>(&self, out: &mut dyn FnMut(Seen<'a, Knotted<'graph, 'cell>>))
    where
        'cell: 'a,
    {
        self.held(&mut |value| out(Seen::of(value)));
    }

    /// The node's content: its syntax's digest, its carried type, and each binding's scope, name
    /// and digest, a value's answered by `parts`. A view digests as it lays flat, so it equals its
    /// own materialization's content: `parts` answers each value it lists, and each flat binding
    /// takes its value's digest.
    pub(super) fn content(&self, parts: &mut dyn FnMut() -> ContentDigest) -> ContentDigest {
        let digest = |code, run: &[Binding<'graph, 'cell>], parts: &mut dyn FnMut() -> _| {
            let mut hasher = DigestHasher::new(Tag::Code);
            hasher.digest(code).feed(self.ktype).count(run.len());
            for binding in run {
                (hasher.feed(binding.scope).feed(binding.name))
                    .digest(binding.link.digest_from(parts));
            }
            hasher.finished()
        };
        match self.syntax {
            Syntax::Flat { code, .. } => digest(code, self.bound, parts),
            Syntax::View(tree) => {
                let (storage, scratch) = (program_storage(), Bump::new());
                let mut digests = BumpVec::new_in(&scratch);
                view::listed(tree, &mut |_| digests.push(parts()));
                let flat = materialized(storage.brand().writer(), tree, &scratch);
                let mut answers = flat.sources.iter().map(|source| digests[*source]);
                let mut each = || answers.next().expect("a flat binding's value is listed");
                digest(syntax_digest(flat.body), &flat.bindings, &mut each)
            }
        }
    }

    /// What `values` reads of this code's syntax, its bindings beside it: a view laid flat
    /// through `writer`, which the reader owns.
    pub(super) fn code_view<'read>(
        &self,
        writer: Writer<'read, '_>,
    ) -> CodeView<'read, 'cell, Knotted<'graph, 'cell>>
    where
        'cell: 'read,
    {
        match self.syntax {
            Syntax::Flat { syntax, .. } => CodeView {
                body: syntax.body(),
                bound: self.bound,
            },
            Syntax::View(tree) => {
                let scratch = Bump::new();
                let flat = materialized(writer, tree, &scratch);
                CodeView {
                    body: flat.body,
                    bound: collect(writer, flat.bindings.iter().copied()),
                }
            }
        }
    }

    /// Every value this code's bindings hold, in the order [`rebuilt`](Self::rebuilt) asks for
    /// them: a view's [`listed`](view::listed), without laying it flat.
    pub(super) fn held(&self, out: &mut dyn FnMut(KValue<'graph, 'cell>)) {
        let mut each = |run: &[Binding<'graph, 'cell>]| {
            for binding in run {
                if let Link::Value(value) = binding.link {
                    out(value);
                }
            }
        };
        match self.syntax {
            Syntax::Flat { .. } => each(self.bound),
            Syntax::View(tree) => view::listed(tree, out),
        }
    }

    /// This code rebuilt in `writer`'s region — the copy's arm: each binding's value through `copy`,
    /// each edge verbatim, composed syntax laid again, and the shape, type and knot weight carried
    /// over. A view is laid flat, keeping its type.
    pub(super) fn rebuilt<'to>(
        &self,
        writer: Writer<'to, '_>,
        mut copy: impl FnMut(&KValue<'graph, 'cell>) -> KValue<'graph, 'to>,
    ) -> Code<'graph, 'to> {
        let mut run = |source: &[Binding<'graph, 'cell>]| {
            writer.fill(source.len(), |at| {
                let binding = &source[at];
                Binding {
                    link: binding.link.copied(&mut copy),
                    ..*binding
                }
            })
        };
        let (syntax, bound) = match self.syntax {
            Syntax::Flat { syntax, code } => {
                let bound = run(self.bound);
                let syntax = Syntax::Flat {
                    syntax: syntax.copied(writer),
                    code,
                };
                (syntax, bound)
            }
            // Each listed value is copied once, in order, and each flat binding takes its copy.
            Syntax::View(tree) => {
                let scratch = Bump::new();
                let mut copies = BumpVec::new_in(&scratch);
                view::listed(tree, &mut |value| copies.push(copy(&value)));
                let flat = materialized(writer, tree, &scratch);
                let bound = writer.fill(flat.bindings.len(), |at| Binding {
                    link: Link::Value(copies[flat.sources[at]]),
                    ..flat.bindings[at]
                });
                (composed_syntax(flat.body), bound)
            }
        };
        Code {
            syntax,
            ktype: self.ktype,
            bound,
            knot_weight: Cell::new(self.knot_weight.get().without_bits()),
        }
    }
}

/// Flat composed syntax over the root `body`, its content its syntax digest.
fn composed_syntax<'graph, 'cell>(body: &'cell KExpression<'cell>) -> Syntax<'graph, 'cell> {
    let code = syntax_digest(body);
    Syntax::Flat {
        syntax: CodeSyntax::Composed {
            body,
            root: ComposedRoot {
                syntax: body,
                digest: code,
            },
            segment: None,
        },
        code,
    }
}

/// What a run of bindings writes: the run and what each value points at.
fn run_weight(run: &[Binding<'_, '_>]) -> Weight {
    run.iter().fold(
        Weight::run::<Binding<'_, '_>>(run.len()),
        |weight, binding| weight.plus(binding.link.referent_weight()),
    )
}

/// A code node, read and not yet written.
pub(super) struct Staged<'graph, 'cell, 'x> {
    pub syntax: CodeSyntax<'graph, 'cell>,
    pub code: ContentDigest,
    pub bound: BumpVec<'x, Binding<'graph, 'cell>>,
    /// The carried type: the code's own, or the annotation's retype its binder births it at.
    pub ktype: KType,
}

impl<'graph, 'cell> Staged<'graph, 'cell, '_> {
    /// This node's bound run laid down in `writer`'s region, beside what it and the resident code
    /// add to the knot's weight.
    pub(super) fn laid_down(
        &self,
        writer: Writer<'cell, '_>,
    ) -> (&'cell [Binding<'graph, 'cell>], Weight) {
        let bound = collect(writer, self.bound.iter().copied());
        let weight = (run_weight(bound).plus(Weight::flat::<Code<'graph, 'cell>>()))
            .plus(self.syntax.weight());
        (bound, weight)
    }

    /// The resident code over `bound`, in a knot weighing `knot_weight`.
    pub(super) fn tied(
        &self,
        writer: Writer<'cell, '_>,
        bound: &'cell [Binding<'graph, 'cell>],
        knot_weight: Weight,
    ) -> Node<'graph, 'cell> {
        Node::Code(resident(
            writer,
            Code {
                syntax: Syntax::Flat {
                    syntax: self.syntax,
                    code: self.code,
                },
                ktype: self.ktype,
                bound,
                knot_weight: Cell::new(knot_weight),
            },
        ))
    }
}

/// The site and the code shape of the quote `part`, if `part` is one `shape` holds.
fn shape_of<'step>(
    shape: &BodyShape<'step>,
    part: &ExpressionPart<'_>,
) -> Option<(Site, &'step BodyShape<'step>)> {
    let ExpressionPart::QuotedExpression(_) = part else {
        return None;
    };
    let site = shape.site(part);
    let code = shape.nested(site)?;
    (code.kind() == ShapeKind::Code).then_some((site, code))
}

/// Read the quote of `syntax` and code shape `shape` into `scratch`: each `$` name's binding read
/// through `activation`, each `Member` source minted by `edge`.
pub(super) fn staged<'graph: 'cell, 'cell, 'x>(
    syntax: CodeSyntax<'graph, 'cell>,
    shape: &BodyShape<'_>,
    activation: &KActivationView<'graph, 'cell>,
    scratch: BumpAllocator<'x>,
    mut edge: impl FnMut(u32) -> Edge,
) -> Staged<'graph, 'cell, 'x> {
    let mut bound = BumpVec::new_in(scratch);
    for capture in shape.captures() {
        let link = match capture.source {
            CaptureSource::Read(coordinate) => Link::Value(activation.read(coordinate)),
            CaptureSource::Member { index, .. } => Link::Edge(edge(index)),
            // A nested quote's `$` names read its encloser; only code built alone binds at entry.
            CaptureSource::Hole | CaptureSource::Offered | CaptureSource::Entry { .. } => continue,
        };
        bound.push(Binding {
            scope: capture.scope(),
            name: capture.name,
            link,
        });
    }
    bound.sort_unstable_by_key(Binding::key);
    // A quote of composed code is its syntax under the root it lies in, not its code as resolved
    // where it was composed.
    let code = match syntax {
        CodeSyntax::Written { .. } => shape.code_digest(),
        CodeSyntax::Composed { body, .. } => syntax_digest(body),
    };
    Staged {
        syntax,
        code,
        bound,
        ktype: shape.code_type(),
    }
}

/// Lay code of `syntax` carried at `ktype` down as a one-node knot in `writer`'s region, its runs
/// holding value words only.
fn one_node<'graph: 'cell, 'cell>(
    writer: Writer<'cell, '_>,
    (syntax, ktype): (Syntax<'graph, 'cell>, KType),
    bound: &[Binding<'graph, 'cell>],
    types: &Chain<'_, '_, '_, '_>,
) -> Knotted<'graph, 'cell> {
    let bound = collect(writer, bound.iter().copied());
    let knot_weight = Weight::flat::<usize>()
        .plus(Weight::flat::<Node<'graph, 'cell>>())
        .plus(Weight::flat::<Code<'graph, 'cell>>())
        .plus(run_weight(bound))
        .plus(match syntax {
            Syntax::Flat { syntax, .. } => syntax.weight(),
            Syntax::View(tree) => tree.weight(),
        });
    let knot = KnotPlan::new(1).tie(writer, |_| {
        Node::Code(resident(
            writer,
            Code {
                syntax,
                ktype,
                bound,
                knot_weight: Cell::new(knot_weight),
            },
        ))
    });
    Knotted::of(super::stamped(knot, types), 0)
}

/// The quote door: birth the quote `part` that `activation`'s shape holds as a one-node knot in
/// `writer`'s region, each `$` name read through `activation`, its syntax kept as `written` holds
/// it. The shape was built where the program loaded, so nothing is refused here: an error in the
/// code waits for the `EVAL` that runs it.
///
/// A quote whose `$` name reads a fellow member is born inside its binder's knot by the tie, which
/// never asks for it, so no binding here is an edge.
pub fn quote<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    activation: &KReading<'_, 'graph, 'cell>,
    written: Written<'graph, 'cell, '_>,
    part: &ExpressionPart<'_>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<Knotted<'graph, 'cell>, ComposeRefused> {
    let (site, shape) = shape_of(activation.shape(), part)
        .expect("a quote is born at the site of a code shape its shape holds");
    let syntax = written.quote(activation.segment(), site);
    let staged = staged(syntax, shape, activation, scratch, |_| {
        unreachable!("only the tie births a quote that reads a fellow member")
    });
    // Code holding a splice composes here, its own bindings the environment of its own parts.
    if shape.composing() {
        let root = composed(writer, syntax.body(), &staged.bound, types, scratch)?;
        return Ok(viewed(writer, CodeTree::of(writer, root), types));
    }
    let syntax = Syntax::Flat {
        syntax: staged.syntax,
        code: staged.code,
    };
    Ok(one_node(
        writer,
        (syntax, staged.ktype),
        &staged.bound,
        types,
    ))
}

/// Code composed at run time from `syntax`, laid in `writer`'s region, carried at `ktype`, `bound`
/// holding each binding sorted by scope, then name: a one-node knot there, with no shape. Its content is
/// its syntax digest over its bindings.
pub fn composed_code<'graph: 'cell, 'cell>(
    writer: Writer<'cell, '_>,
    syntax: &'cell KExpression<'cell>,
    ktype: KType,
    bound: &[Binding<'graph, 'cell>],
    types: &Chain<'_, '_, '_, '_>,
) -> Knotted<'graph, 'cell> {
    one_node(writer, (composed_syntax(syntax), ktype), bound, types)
}

/// The code view `tree` as a one-node knot in `writer`'s region, carried at its kind unbuilt.
pub(crate) fn viewed<'graph: 'cell, 'cell>(
    writer: Writer<'cell, '_>,
    tree: &'cell CodeTree<'graph, 'cell>,
    types: &Chain<'_, '_, '_, '_>,
) -> Knotted<'graph, 'cell> {
    let ktype = types.code_unbuilt(tree.kind());
    one_node(writer, (Syntax::View(tree), ktype), &[], types)
}

/// The root of `member`'s code as a view: a view's own, or one over a flat code value's root,
/// laid through `writer`, its marks resolving through the member's bindings.
pub(super) fn root_view<'graph: 'cell, 'cell>(
    writer: Writer<'cell, '_>,
    member: Knotted<'graph, 'cell>,
) -> &'cell CodeTree<'graph, 'cell> {
    let node = member.code().expect("a code member holds code");
    match node.syntax {
        Syntax::View(tree) => tree,
        Syntax::Flat { syntax, .. } => CodeTree::of(
            writer,
            CodeTree::over(writer, syntax.body(), Env::Code(member)),
        ),
    }
}

/// How many parts the root of `member`'s code holds: what `LENGTH` answers.
pub(crate) fn code_length(member: Knotted<'_, '_>) -> usize {
    let node = member.code().expect("a code member holds code");
    match node.syntax {
        Syntax::View(tree) => tree.len(),
        Syntax::Flat { syntax, .. } => syntax.body().parts.len(),
    }
}

/// `code i`: part `at` of the root of `member`'s code as an unbuilt view in `writer`'s region — a
/// group part as the node it wraps, any other part as a one-part node — or `None` past the last.
pub(crate) fn code_part<'graph: 'cell, 'cell>(
    writer: Writer<'cell, '_>,
    member: Knotted<'graph, 'cell>,
    at: usize,
    types: &Chain<'_, '_, '_, '_>,
) -> Option<Knotted<'graph, 'cell>> {
    let tree = root_view(writer, member);
    (at < tree.len()).then(|| viewed(writer, CodeTree::of(writer, tree.part(writer, at)), types))
}

/// `code {from, to}`: the root of `member`'s code's parts `[from, to)`, within its length, read
/// backward where `backward` is set, as an unbuilt view in `writer`'s region.
pub(crate) fn code_slice<'graph: 'cell, 'cell>(
    writer: Writer<'cell, '_>,
    member: Knotted<'graph, 'cell>,
    range: (usize, usize),
    backward: bool,
    types: &Chain<'_, '_, '_, '_>,
) -> Knotted<'graph, 'cell> {
    let tree = root_view(writer, member);
    viewed(
        writer,
        CodeTree::of(writer, tree.slice(writer, range, backward)),
        types,
    )
}

/// `member`'s code laid flat in `writer`'s region where it is a view, at the view's type: what an
/// `EVAL` or a `USING` runs. Flat code is itself.
pub(crate) fn flattened<'graph: 'cell, 'cell>(
    writer: Writer<'cell, '_>,
    member: Knotted<'graph, 'cell>,
    types: &Chain<'_, '_, '_, '_>,
) -> Knotted<'graph, 'cell> {
    let node = member.code().expect("a code member holds code");
    let Some(tree) = node.view() else {
        return member;
    };
    let scratch = Bump::new();
    let flat = materialized(writer, tree, &scratch);
    let syntax = composed_syntax(flat.body);
    one_node(writer, (syntax, node.ktype), &flat.bindings, types)
}

/// Why a `USING` refused to fill a code's holes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsingRefused {
    /// The module's registrations at `key` rank its slots other than the code's own candidates at
    /// `key` do.
    Ranking { key: KeySymbol },
    /// A filled use's holder would nest the code deeper than the syntax depth limit.
    TooDeep,
}

impl UsingRefused {
    /// The refusal as the error `USING` raises words it, its names read through `symbols`.
    pub fn display(self, symbols: &SymbolInterner) -> impl std::fmt::Display + '_ {
        UsingRefusal {
            refused: self,
            symbols,
        }
    }
}

/// [`UsingRefused::display`]'s view.
struct UsingRefusal<'x> {
    refused: UsingRefused,
    symbols: &'x SymbolInterner,
}

impl std::fmt::Display for UsingRefusal<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.refused {
            UsingRefused::Ranking { key } => write!(
                f,
                "{} is ranked two ways",
                self.symbols.display(key.symbol())
            ),
            UsingRefused::TooDeep => write!(
                f,
                "composed code nests deeper than the syntax depth limit of {}",
                crate::parse::MAX_SYNTAX_DEPTH
            ),
        }
    }
}

/// `code USING source`: composed code in `writer`'s region whose holes `source` fills, each a
/// filled mark in a scope of its own, and whose other holes stay holes. A name hole takes the field
/// of `source` — a record, or a module's member — that it names; a keyworded hole takes the list
/// of a module's registrations at its key, and stays open where the module has none. A field
/// naming no hole is ignored, and a hole an earlier `USING` filled is a filled mark, no hole, so it
/// is never rebound. The code is built, at the type its shape, read through `door`, gives it. Code
/// whose shape carries a refusal comes back unchanged — `facts` say which does: its holes are
/// unknown, and the `EVAL` that runs it reports the refusal.
///
/// The syntax is copied from the syntax its shapes were built over, each filled hole marked where
/// it is written ([`filled_sites`]); each binding of `code` that is an edge is resolved to the
/// member it names, so the new knot's run holds value words only.
///
/// Refused, writing nothing, where a module's registrations at a hole's key rank their slots other
/// than the code's own candidates at the key do.
pub fn using<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    door: &scope::Code<'_>,
    facts: &Facts<'_>,
    code: Knotted<'graph, 'cell>,
    source: KValue<'graph, 'cell>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<Knotted<'graph, 'cell>, UsingRefused> {
    let node = code
        .code()
        .expect("`USING` fills the holes of a quote's code");
    let shape = node.shape(door);
    if facts.typing_refusal(types, shape).is_some() {
        return Ok(code);
    }
    // The fills' scope is one no binding of the code holds.
    let scope = Scope {
        number: (node.bound().iter())
            .map(|binding| binding.scope.number + 1)
            .max()
            .unwrap_or(0),
        pinned: false,
    };
    let mut fills: BumpVec<'_, Binding<'graph, 'cell>> = BumpVec::new_in(scratch);
    let mut holes = BumpVec::new_in(scratch);
    let mut keyed = BumpVec::new_in(scratch);
    for (index, capture) in shape.captures().iter().enumerate() {
        if capture.source != CaptureSource::Hole {
            continue;
        }
        let value = match capture.name {
            BinderSymbol::Key(key) => {
                keyed.clear();
                self::keyed(writer, shape, key, source, types, scratch, &mut keyed)?;
                (!keyed.is_empty())
                    .then(|| Value::List(List::of_candidates(writer, keyed.iter().copied(), types)))
            }
            name => super::field(writer, source, name, types, scratch),
        };
        if let Some(value) = value {
            holes.push(CaptureSlot(index as u32));
            fills.push(Binding {
                scope,
                name: capture.name,
                link: Link::Value(value),
            });
        }
    }
    if fills.is_empty() {
        return Ok(built(writer, door, code, types));
    }
    let written = (door.shapes())
        .written(shape.segment())
        .expect("code is written");
    let mut filling = Filling {
        sites: filled_sites(door.shapes(), shape, &holes, scratch),
        scope: scope.number,
    };
    let body = remarked_whole(writer, &written, &mut filling);
    // Each filled use's holder is a level of its own.
    if body.depth() > crate::parse::MAX_SYNTAX_DEPTH {
        return Err(UsingRefused::TooDeep);
    }
    let resolved = |binding: &Binding<'graph, 'cell>| Binding {
        link: Link::Value(binding.link.resolve(code)),
        ..*binding
    };
    let mut bound = BumpVec::with_capacity_in(node.bound().len() + fills.len(), scratch);
    bound.extend(node.bound().iter().map(resolved));
    bound.extend(fills.iter().copied());
    bound.sort_unstable_by_key(Binding::key);
    Ok(composed_code(
        writer,
        body,
        shape.code_type(),
        &bound,
        types,
    ))
}

/// `USING`'s copy of written syntax: each name it fills a filled mark, and each use it fills a
/// filled use, at the fills' scope, pinned where a quote value holds it, so it reads the root's
/// binding there.
struct Filling<'x> {
    sites: FilledSites<'x>,
    scope: u32,
}

impl Remark for Filling<'_> {
    fn scope(&mut self, scope: Scope, _: Quoted) -> Scope {
        scope
    }

    fn filled(&mut self, part: &ExpressionPart<'_>, at: Quoted) -> Option<Mark> {
        self.sites.name(part).then_some(Mark::Filled(Scope {
            number: self.scope,
            pinned: at.now,
        }))
    }

    fn filled_use(
        &mut self,
        node: &KExpression<'_>,
        at: Quoted,
        keys: &mut dyn FnMut(KeySymbol),
    ) -> Option<Scope> {
        let mut filled = false;
        for key in self.sites.keys(node) {
            keys(key);
            filled = true;
        }
        filled.then_some(Scope {
            number: self.scope,
            pinned: at.now,
        })
    }
}

/// `code`, composed at run time and laid flat, carried at the exact type its shape gives it, which
/// `door` reads under its composition: a new one-node knot in `writer`'s region over the same
/// syntax and bindings, or `code` itself where it is carried there already. Code whose shape kept
/// an error builds too: the `EVAL` that runs it reports the error.
pub fn built<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    door: &scope::Code<'_>,
    code: Knotted<'graph, 'cell>,
    types: &Chain<'_, 'cell, '_, '_>,
) -> Knotted<'graph, 'cell> {
    let node = code.code().expect("`BUILD` builds a quote's code");
    let ktype = node.shape(door).code_type();
    if ktype == Value::Knotted(code).concrete_ktype() {
        return code;
    }
    let scratch = Bump::new();
    let resolved = |binding: &Binding<'graph, 'cell>| Binding {
        link: Link::Value(binding.link.resolve(code)),
        ..*binding
    };
    let mut bound = BumpVec::with_capacity_in(node.bound().len(), &scratch);
    bound.extend(node.bound().iter().map(resolved));
    one_node(writer, (node.syntax, ktype), &bound, types)
}

/// Push onto `found` each function the module `source` offers at `key` — a registration, an
/// overload a view carries, or one a header retypes in `writer`'s region. A ranking other than the one `shape`'s own candidates at `key` carry
/// is refused.
fn keyed<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    shape: &BodyShape<'_>,
    key: KeySymbol,
    source: KValue<'graph, 'cell>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
    found: &mut BumpVec<'_, KValue<'graph, 'cell>>,
) -> Result<(), UsingRefused> {
    for member in super::module::layout::functions_at(writer, source, key, None, types, scratch) {
        let registered = super::module::layout::registered_shape(member)
            .expect("a registration member is a function");
        // Only the ranking is read, which a scheme's body spells as a type's does.
        let TypeNode::ExpressionShape { classes, .. } = registered.view(types).body else {
            unreachable!("a registered shape is an expression shape")
        };
        let ranking = classes.copied_into(scratch);
        if !shape.ranks_alike(key, ranking) {
            return Err(UsingRefused::Ranking { key });
        }
        found.push(member);
    }
    Ok(())
}
