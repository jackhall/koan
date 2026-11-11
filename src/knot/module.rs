//! Modules as values: the node a module is, the view `:|` builds, the coercion that births a
//! view's members, the readers of a module through the type it carries, and the binding a
//! `USING … SCOPE` block enters on.
//!
//! A module's node holds its self-signature, its members in layout order, its knot weight and its
//! content — never an activation, since a view has no body to activate. Its content is what it is,
//! not what it holds: a body-born module's code over what it captures, and a view's application
//! over its underlying — the module under every header and view — and what its fit solved there.
//! [`ModuleContent`] owns that recipe, and an opaque view's carriers are keyed on it. An opaque
//! view keeps its underlying and the record of how it was built beside its content, so a view of
//! it is built over the same underlying. A view's content is computed where the
//! view is built. A body-born module's is computed where a demand first reaches it — a `:|` over
//! it ([`view::fill_at_view`]) or a digest walk — and kept. The module holds its body and its
//! closure for life, as a function does, so a copy computes the content its source would, and a
//! module only surfaced, read or passed on digests nothing. A module is always a one-node knot:
//! a mention reached from a module binder's root is eager whatever body it sits in. [`birth`] is
//! where a body-born one comes from, [`Module::tie`] the one private-field door a body-born module
//! and a view both go through, so there is one representation and one copy.
//!
//! Everything else here reads a module by name. Its spine is **layout order** — value members
//! sorted by name, then type members sorted by name — which a body shape's slots, a signature's
//! tables and a `USING` block's parameters all agree on, so a member is reached by index and never
//! by a search through the value it sits in. [`layout`] owns that rule; nothing else here spells it.
//!
//! A module seen at a signature type — a parameter's, a frame's return, `:!` — is a header over its
//! bare node at the application it fits ([`Retyped`](crate::values::Retyped)), and every reader in
//! [`layout`] reads it through the type it carries: a member at the type that application declares
//! it at. A **view** narrows: [`view::ascribe`] checks the source satisfies the signature, keeps
//! only what the signature names, built over its underlying's members, and lays a module node of
//! its own down through the same door. Each abstract member is hidden behind a carrier keyed on
//! content, and every member is born **coerced** to it: data is re-tagged through the admission
//! barrier, containers are rebuilt cell by cell, a nested module is viewed in turn, and a function
//! is wrapped in a [`Coerced`] barrier node a call will later go through.
//!
//! What this layer does *not* do is evaluate anything: dispatch evaluates
//! `m :| Sig` and `m.f` through the doors here.
//!
//! This module names its [knot vocabulary](crate::knot) through the facade and never its sibling
//! [`function`](super::function).
//!
//! See [module/README.md](module/README.md).

mod birth;
pub mod coerce;
pub mod layout;
pub mod surface;
pub mod view;

#[cfg(test)]
mod tests;

pub use birth::{body_activation, tie_member};

use std::cell::Cell;

use crate::memory::{Knot, KnotPlan, Writer, collect, resident};
use crate::scope::{BodyAt, BodyRef, ClosureBindings, ViewRecord};

use crate::type_lattice::{ContentKey, DeclaredType, KType, Parametric};
pub use crate::values::digest::Operator;
use crate::values::digest::{DigestHasher, Tag, body_content, view_content};
use crate::values::{Chain, ContentDigest, Knotted as _, Link, Seen, Value, Weight};
use coerce::Hidden;

use super::{KValue, Knotted, Node, composed};

/// A module: what one knot node holds.
pub struct Module<'graph, 'cell> {
    ktype: KType,
    /// The bound members, in layout order.
    members: &'cell [KValue<'graph, 'cell>],
    /// What rebuilding the whole knot this node sits in writes, the same on every node; the root
    /// node's also carries the knot's memo bits.
    knot_weight: Cell<Weight>,
    /// What the module is: its knot digests over it. It sits beside the node, as a barrier does, so
    /// the module arm keeps the node's width.
    content: &'cell Content<'graph, 'cell>,
}

/// What a module is born with, which [`Module::tie`] lays beside its node: a view's content,
/// computed where the view is built, beside its record and its underlying for an opaque view, or a
/// body-born module's body and the closure it ran over, from which its content is computed on
/// demand. An inward re-view keeps no record: it is a transparent view of itself.
pub enum Born<'graph, 'cell, 'h> {
    View {
        content: ModuleContent,
        view: Option<(ViewRecord<'h>, Knotted<'graph, 'cell>)>,
    },
    Body {
        body: BodyAt<'graph, 'cell, 'cell>,
        closure: &'cell ClosureBindings<'cell, Knotted<'graph, 'cell>>,
    },
}

/// A module's content beside its node: the content once known, an opaque view's record and
/// underlying, and a body-born module's body and closure, kept for the module's life so a copy
/// carries what the content is computed from.
struct Content<'graph, 'cell> {
    known: Cell<Option<ModuleContent>>,
    view: Option<(ViewRecord<'cell>, Knotted<'graph, 'cell>)>,
    body: Option<(
        BodyAt<'graph, 'cell, 'cell>,
        &'cell ClosureBindings<'cell, Knotted<'graph, 'cell>>,
    )>,
}

impl<'graph, 'cell> Module<'graph, 'cell> {
    /// A module over `members`, already in layout order under `ktype`, born as `born`, laid down as
    /// a one-node knot in `writer`'s region. The one private-field constructor the tie and the view
    /// door share; the module is node `0`.
    ///
    /// A member that is itself a knot member carries its whole knot's weight, since a crossing
    /// rebuilds that knot whole; the content beside the node weighs in too, with a body-born
    /// module's closure, and an opaque view's underlying, since a crossing copies it. The content
    /// is what the module is — a body's code over what it captures, or a view's application over
    /// its underlying and `from*` — never its members, which that content determines.
    pub fn tie(
        writer: Writer<'cell, '_>,
        ktype: KType,
        members: &[KValue<'graph, 'cell>],
        born: Born<'graph, 'cell, '_>,
        types: &Chain<'_, '_, '_, '_>,
    ) -> Knot<'cell, Node<'graph, 'cell>> {
        let (known, view, body) = match born {
            Born::View { content, view } => (Some(content), view, None),
            Born::Body { body, closure } => (None, None, Some((body, closure))),
        };
        let view = view.map(|(record, underlying)| (record.laid(writer), underlying));
        let flat = Weight::flat::<usize>()
            .plus(Weight::flat::<Node<'graph, 'cell>>())
            .plus(Weight::flat::<Content<'graph, 'cell>>());
        let flat = body.map_or(flat, |(_, closure)| flat.plus(closure.weight()));
        let flat = view.map_or(flat, |(record, underlying)| {
            (flat.plus(underlying.weight()))
                .plus(Weight::run::<(KType, KType)>(record.hidden.len()))
        });
        let knot_weight =
            (members.iter()).fold(flat, |weight, member| weight.plus(member.weight()));
        let members = collect(writer, members.iter().copied());
        let content = resident(
            writer,
            Content {
                known: Cell::new(known),
                view,
                body,
            },
        );
        let knot = KnotPlan::new(1).tie(writer, |_| {
            Node::Module(Module {
                ktype,
                members,
                knot_weight: Cell::new(knot_weight),
                content,
            })
        });
        super::stamped(knot, types)
    }

    /// The module's self-signature, or the signature a view was built at.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// The members, in layout order.
    pub fn members(&self) -> &'cell [KValue<'graph, 'cell>] {
        self.members
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight.get()
    }

    /// The knot weight's cell, where a knot's root node keeps its memo bits.
    pub(super) fn knot_weight_cell(&self) -> &Cell<Weight> {
        &self.knot_weight
    }

    /// What the module is, where a demand has computed it: always for a view.
    pub fn content(&self) -> Option<ModuleContent> {
        self.content.known.get()
    }

    /// An opaque view's record: the application it was built at and its two substitutions. `None`
    /// for a transparent view of itself — a body-born module, or an inward re-view.
    pub fn view_record(&self) -> Option<ViewRecord<'cell>> {
        self.content.view.map(|(record, _)| record)
    }

    /// The module an opaque view is built over: a body-born module or an inward re-view, never a
    /// view of one. `None` for a module that is its own underlying.
    pub fn underlying(&self) -> Option<Knotted<'graph, 'cell>> {
        self.content.view.map(|(_, underlying)| underlying)
    }

    /// A body-born module's closure: what its content is computed over, kept for the module's
    /// life.
    pub fn captures(&self) -> Option<&'cell ClosureBindings<'cell, Knotted<'graph, 'cell>>> {
        self.content.body.map(|(_, closure)| closure)
    }

    /// A body-born module's body's record, the root of the composed code it lies in on it: `None`
    /// for a view.
    pub fn body(&self) -> Option<&'cell BodyRef<'cell, 'cell>> {
        self.content.body.map(|(body, _)| body.get())
    }

    /// A body-born module's body and closure while no demand has computed its content.
    pub fn pending(
        &self,
    ) -> Option<(
        &'cell BodyRef<'cell, 'cell>,
        &'cell ClosureBindings<'cell, Knotted<'graph, 'cell>>,
    )> {
        (self.content.body)
            .filter(|_| self.content.known.get().is_none())
            .map(|(body, closure)| (body.get(), closure))
    }

    /// Keep `content`, computed by a demand, as what the module is.
    pub fn fill(&self, content: ModuleContent) {
        debug_assert!(
            self.content
                .known
                .get()
                .is_none_or(|known| known == content),
            "a module's content is one fact"
        );
        self.content.known.set(Some(content));
    }

    /// The values the node's content covers while it is pending, in the order
    /// [`content_digest`](Self::content_digest) asks for their digests: each capture the code
    /// digest does not name that holds a value, as a function's. Nothing once it is known.
    pub(super) fn content_parts<'a>(&self, out: &mut dyn FnMut(Seen<'a, Knotted<'graph, 'cell>>))
    where
        'cell: 'a,
    {
        if let Some((body, closure)) = self.pending() {
            for (_, link) in composed(body, closure.links()) {
                if let Link::Value(value) = link {
                    out(Seen::of(*value));
                }
            }
        }
    }

    /// The node's content digest: the known content's, or a pending one computed over its captures,
    /// a value's digest answered by `parts`, and kept.
    pub(super) fn content_digest(&self, parts: &mut dyn FnMut() -> ContentDigest) -> ContentDigest {
        if let Some(known) = self.content.known.get() {
            return known.digest();
        }
        let (body, closure) = self.content.body.expect("a view's content is known");
        let captures = composed(body.get(), closure.links())
            .map(|(index, link)| (index, link.digest_from(parts)));
        let content = ModuleContent::body(body.get().code, captures);
        self.fill(content);
        content.digest()
    }

    /// This module over `members`, `closure` and `underlying` rebuilt in `writer`'s region — the
    /// copy's arm. The signature, the knot weight, the content and a view's record are facts about
    /// what the module holds, which the copy preserves; a body-born module's closure and a view's
    /// underlying arrive copied.
    pub(super) fn rebuilt<'to>(
        &self,
        writer: Writer<'to, '_>,
        members: &'to [KValue<'graph, 'to>],
        closure: Option<&'to ClosureBindings<'to, Knotted<'graph, 'to>>>,
        underlying: Option<Knotted<'graph, 'to>>,
    ) -> Module<'graph, 'to> {
        let body = (self.content.body.map(|(body, _)| body.copied(writer))).zip(closure);
        Module {
            ktype: self.ktype,
            members,
            knot_weight: Cell::new(self.knot_weight.get().without_bits()),
            content: resident(
                writer,
                Content {
                    known: Cell::new(self.content.known.get()),
                    view: (self.view_record())
                        .zip(underlying)
                        .map(|(record, underlying)| (record.laid(writer), underlying)),
                    body,
                },
            ),
        }
    }
}

/// What a module is, as its knot digests it: a body-born module's code over what it captures, or a
/// view's operator and application over its underlying and `from*`. Its constructors are the one
/// recipe for either, and an opaque view's carriers are keyed on its bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ModuleContent(ContentDigest);

impl ModuleContent {
    /// A body-born module's content: its body's `code` digest, then each capture the code digest
    /// does not name, beside its slot, as [`composed`](super::composed) yields them.
    pub fn body(
        code: ContentDigest,
        captures: impl Iterator<Item = (usize, ContentDigest)>,
    ) -> ModuleContent {
        ModuleContent(body_content(code, captures))
    }

    /// A view's content: its `operator` and the `application` it was built at, over its
    /// underlying's digest `source` and `from`, the substitution its fit solved over the
    /// underlying, interned. An inward re-view is keyed over the module it re-views.
    pub fn view(
        operator: Operator,
        application: KType,
        source: ContentDigest,
        from: KType,
    ) -> ModuleContent {
        ModuleContent(view_content(operator, application, source, Some(from)))
    }

    /// The key an opaque view's carriers are made under: its own content, so two views of equal
    /// content share their carriers.
    pub fn carrier_key(&self) -> ContentKey {
        ContentKey(self.0.bits())
    }

    /// The digest the module's knot reads as its node's content.
    pub(super) fn digest(self) -> ContentDigest {
        self.0
    }
}

/// A function member behind an opaque view's barrier: what one knot node points at.
///
/// A call goes through the barrier before `underlying` runs: its arguments cross
/// [`inward`](coerce::inward) and its return [`outward`](coerce::outward).
pub struct Coerced<'graph, 'cell> {
    underlying: Knotted<'graph, 'cell>,
    ktype: DeclaredType<KType>,
    declared: DeclaredType<Parametric>,
    from: KType,
    to: KType,
    hidden: Hidden<'cell>,
    /// What rebuilding the whole knot this node sits in writes, the same on every node; the root
    /// node's also carries the knot's memo bits.
    knot_weight: Cell<Weight>,
}

impl<'graph, 'cell> Coerced<'graph, 'cell> {
    /// A function member behind an opaque view's barrier, laid down as a one-node knot in
    /// `writer`'s region; the member is node `0`. Its six fields sit in a resident struct the node
    /// points at rather than in the node itself, so the arm is one word wide and the module arm
    /// keeps setting the node's width.
    ///
    /// `underlying` carries its whole knot's weight, since a crossing rebuilds that knot whole.
    /// `hidden` is laid beside it, for a crossing to read the view's carriers back.
    #[allow(clippy::too_many_arguments)]
    pub fn tie(
        writer: Writer<'cell, '_>,
        underlying: Knotted<'graph, 'cell>,
        ktype: DeclaredType<KType>,
        declared: DeclaredType<Parametric>,
        from: KType,
        to: KType,
        hidden: Hidden<'_>,
        types: &Chain<'_, '_, '_, '_>,
    ) -> Knot<'cell, Node<'graph, 'cell>> {
        debug_assert!(
            underlying.function().is_some() || underlying.coerced().is_some(),
            "only a function, or a function already behind a barrier, is coerced"
        );
        let knot_weight = Weight::flat::<usize>()
            .plus(Weight::flat::<Node<'graph, 'cell>>())
            .plus(Weight::flat::<Coerced<'graph, 'cell>>())
            .plus(Weight::run::<(KType, KType)>(hidden.carriers.len()))
            .plus(underlying.weight());
        let hidden = hidden.laid(writer);
        let knot = KnotPlan::new(1).tie(writer, |_| {
            Node::Coerced(resident(
                writer,
                Coerced {
                    underlying,
                    ktype,
                    declared,
                    from,
                    to,
                    hidden,
                    knot_weight: Cell::new(knot_weight),
                },
            ))
        });
        super::stamped(knot, types)
    }

    /// The function the call runs once it is through the barrier — itself a function, or a
    /// barrier a body-born module bound, as `MODULE n = ((LET succ = c.succ))` binds one of `c`'s.
    pub fn underlying(&self) -> Knotted<'graph, 'cell> {
        self.underlying
    }

    /// The function type at the view's substitution: what a caller sees — a scheme where the
    /// member is quantified.
    pub fn ktype(&self) -> DeclaredType<KType> {
        self.ktype
    }

    /// The slot type the view's signature declares, over its head parameters, which the coercion
    /// walk recurses on.
    pub fn declared(&self) -> DeclaredType<Parametric> {
        self.declared
    }

    /// A `Signature` handle whose manifest members are the source module's bindings.
    pub fn from(&self) -> KType {
        self.from
    }

    /// A `Signature` handle whose manifest members are the view's bindings.
    pub fn to(&self) -> KType {
        self.to
    }

    /// The view's carriers beside what each hides, and on which side of the barrier.
    pub fn hidden(&self) -> Hidden<'cell> {
        self.hidden
    }

    pub fn knot_weight(&self) -> Weight {
        self.knot_weight.get()
    }

    /// The knot weight's cell, where a knot's root node keeps its memo bits.
    pub(super) fn knot_weight_cell(&self) -> &Cell<Weight> {
        &self.knot_weight
    }

    /// The value the node's content covers: the function member it stands before.
    pub(super) fn content_parts<'a>(&self, out: &mut dyn FnMut(Seen<'a, Knotted<'graph, 'cell>>))
    where
        'cell: 'a,
    {
        out(Seen::of(Value::Knotted(self.underlying)));
    }

    /// The node's content: what stands before its function, its digest answered by `parts`, at
    /// the types the two sides read.
    pub(super) fn content(&self, parts: &mut dyn FnMut() -> ContentDigest) -> ContentDigest {
        DigestHasher::new(Tag::Barrier)
            .digest(parts())
            .feed(self.ktype)
            .feed(self.declared)
            .feed(self.from)
            .feed(self.to)
            .finished()
    }

    /// This barrier over `underlying` rebuilt in `writer`'s region — the copy's arm. The types and
    /// the knot weight ride over.
    pub(super) fn rebuilt<'to>(
        &self,
        writer: Writer<'to, '_>,
        underlying: Knotted<'graph, 'to>,
    ) -> Coerced<'graph, 'to> {
        Coerced {
            underlying,
            ktype: self.ktype,
            declared: self.declared,
            from: self.from,
            to: self.to,
            hidden: self.hidden.laid(writer),
            knot_weight: Cell::new(self.knot_weight.get().without_bits()),
        }
    }
}
