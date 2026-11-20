//! A callable's **stamp**: the requests that built the chain of settlements it was born under, so a
//! call of it whose depot slot was evicted can rebuild that chain and be born warm
//! ([build/README.md § Resumption](../build/README.md#resumption)).
//!
//! An [`Ask`] is what a miss settles: a body and what each name its key lists binds. A
//! [`SiteRequest`] is one call site's ask and key laid with a layer, which a call at that site hands
//! the drain in place of computing them from its values. A [`Level`]
//! is one settlement's whole request — its key, its ask, the types its binding carries, and each
//! module of other composed code it pins, whose root rebuilds that code's composition. A level is
//! laid once per frame, in the frame's own region, and only for a frame that seeks a settlement
//! and whose callee's body can birth a callable: the frame's region does not exist while its asker
//! runs, so the asker hands the request by value ([`Sought`]) to the drain, which hands it back to
//! the frame's first step to lay once the frame is born under that settlement. The stamp of a callable born in the frame is that
//! level over the stamp of the frame's callee ([`Stamp`]), two words a closure holds by value, so a
//! birth lays nothing for it. Neither is content: a closure's digest and equality skip its stamp,
//! as they skip its depot slot.

use smallvec::SmallVec;

use crate::memory::{Lays, Writer, collect, resident};
use crate::type_lattice::{DeclaredType, KType, Parametric};
use crate::values::digest::{retyped_content, type_digest};
use crate::values::{Chain, ContentDigest, DepotRef, KeyedTypes, Knotted, LayerKey, Value, Weight};

use super::shape::{Segment, Slot};

/// What a miss settles: the callee's body, and what each name its key lists binds there.
#[derive(PartialEq, Eq, Debug)]
pub struct Ask {
    pub body: Segment,
    pub bound: SmallVec<[Asked; 1]>,
}

/// What the keyed name at `slot` binds: a type, or a module's carried type over the closed tree
/// its bare member's content digest names, `retyped` where that type is not the member's own. A
/// module born of code composed at run time names its composition by syntax digest, where a miss
/// under a composition finds its tree if no layer below does, and pins it. The slot rides in each
/// arm, in the padding the digests leave, so an ask stays small enough to hand on by value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Asked {
    Type {
        slot: Slot,
        named: KType,
    },
    Module {
        slot: Slot,
        carried: KType,
        content: ContentDigest,
        retyped: bool,
        composition: Option<ContentDigest>,
    },
}

impl Asked {
    /// The slot of the keyed name this binds.
    pub fn slot(&self) -> Slot {
        match *self {
            Asked::Type { slot, .. } | Asked::Module { slot, .. } => slot,
        }
    }

    /// What the key digests at this name: what the callee's frame binds there — a type as its type
    /// value, a module as its member, beside the type the frame binds it at where that is not the
    /// member's own.
    pub fn digest(&self, types: &Chain<'_, '_, '_, '_>) -> ContentDigest {
        match *self {
            Asked::Type { named, .. } => type_digest(named, KType::of_kind(named.kind_of(types))),
            Asked::Module {
                carried,
                content,
                retyped,
                ..
            } => match retyped {
                true => retyped_content(DeclaredType::Type(carried), content),
                false => content,
            },
        }
    }

    /// This binding at the keyed name at `slot`: what a site passing it on binds its callee's
    /// name to.
    pub fn at(self, slot: Slot) -> Asked {
        match self {
            Asked::Type { named, .. } => Asked::Type { slot, named },
            Asked::Module {
                carried,
                content,
                retyped,
                composition,
                ..
            } => Asked::Module {
                slot,
                carried,
                content,
                retyped,
                composition,
            },
        }
    }

    /// The type this binds the name at.
    fn ktype(&self) -> KType {
        match *self {
            Asked::Type { named, .. } => named,
            Asked::Module { carried, .. } => carried,
        }
    }
}

/// One settlement's request by value, before it is laid as a [`Level`]: its key, what a miss
/// settles, and the types its binding carries, in the order its key digests them.
#[derive(PartialEq, Eq, Debug)]
pub struct Sought {
    pub key: LayerKey,
    pub ask: Ask,
    pub keyed: SmallVec<[KeyedTypes; 1]>,
}

/// One call site's request, laid with a layer: the key and the ask of the binding the site passes
/// under the layer's binding, and the types that binding carries.
#[derive(Clone, Copy, Debug)]
pub struct SiteRequest<'a> {
    pub key: LayerKey,
    pub body: Segment,
    pub bound: &'a [Asked],
    pub keyed: &'a [KeyedTypes],
}

impl<'a> SiteRequest<'a> {
    /// `sought` laid through `lay`.
    pub fn laid(lay: impl Lays<'a>, sought: &Sought) -> Self {
        SiteRequest {
            key: sought.key,
            body: sought.ask.body,
            bound: collect(lay, sought.ask.bound.iter().copied()),
            keyed: collect(lay, sought.keyed.iter().copied()),
        }
    }

    /// This request by value: what a call hands the drain.
    pub fn sought(&self) -> Sought {
        Sought {
            key: self.key,
            ask: Ask {
                body: self.body,
                bound: self.bound.iter().copied().collect(),
            },
            keyed: self.keyed.iter().copied().collect(),
        }
    }

    /// Every type it names.
    pub fn each_type(&self, out: &mut dyn FnMut(DeclaredType<Parametric>)) {
        each_type_of(self.bound, self.keyed, out);
    }
}

/// Every type a binding names: the type each keyed name binds at, and each keyed argument's types.
fn each_type_of(
    bound: &[Asked],
    keyed: &[KeyedTypes],
    out: &mut dyn FnMut(DeclaredType<Parametric>),
) {
    for asked in bound {
        out(asked.ktype().into());
    }
    for keyed in keyed {
        out(keyed.carried.into());
        if let Some(named) = keyed.named {
            out(named.into());
        }
    }
}

/// One settlement's request, laid in a region: its key, the body it settles and what each keyed
/// name binds, the types the binding carries, each module of other composed code whose composition
/// it pins, and what its chain rests on: the root, or a layer the load laid, which is never
/// evicted and so is no level, where a replay of the chain starts.
#[derive(Clone, Copy)]
pub struct Level<'cell, X> {
    pub base: DepotRef,
    pub key: LayerKey,
    pub body: Segment,
    pub bound: &'cell [Asked],
    pub keyed: &'cell [KeyedTypes],
    pub modules: &'cell [Value<'cell, X>],
}

impl<'cell, X: Knotted> Level<'cell, X> {
    /// `sought`'s level, pinning the compositions of `modules`, its chain resting on `base`, laid
    /// in `writer`'s region.
    pub fn laid(
        writer: Writer<'cell, '_>,
        sought: &Sought,
        modules: &[Value<'cell, X>],
        base: DepotRef,
    ) -> &'cell Self {
        resident(
            writer,
            Level {
                base,
                key: sought.key,
                body: sought.ask.body,
                bound: collect(writer, sought.ask.bound.iter().copied()),
                keyed: collect(writer, sought.keyed.iter().copied()),
                modules: collect(writer, modules.iter().copied()),
            },
        )
    }

    /// This level's request by value: what a replay of it seeks.
    pub fn sought(&self) -> Sought {
        Sought {
            key: self.key,
            ask: Ask {
                body: self.body,
                bound: self.bound.iter().copied().collect(),
            },
            keyed: self.keyed.iter().copied().collect(),
        }
    }
}

/// A callable's stamp: the level of the settlement it was born under, over the stamp of the callee
/// whose frame that was.
#[derive(Clone, Copy)]
pub struct Stamp<'cell, X> {
    pub level: &'cell Level<'cell, X>,
    pub below: Option<&'cell Stamp<'cell, X>>,
}

impl<'cell, X: Knotted> Stamp<'cell, X> {
    /// Each level, from the outermost — the settlement nearest the chain's base — in.
    pub fn levels(&self) -> SmallVec<[&'cell Level<'cell, X>; 4]> {
        let mut levels: SmallVec<[_; 4]> = std::iter::successors(Some(self), |stamp| stamp.below)
            .map(|stamp| stamp.level)
            .collect();
        levels.reverse();
        levels
    }

    /// What the chain rests on, where a replay of it starts: its outermost level's base, which
    /// every level inner to it copies.
    pub fn base(&self) -> DepotRef {
        let mut outermost = self;
        while let Some(below) = outermost.below {
            outermost = below;
        }
        outermost.level.base
    }

    /// Every module value a level holds, innermost level first, in the order
    /// [`copied`](Self::copied) asks for them.
    pub fn held(&self, out: &mut dyn FnMut(Value<'cell, X>)) {
        for stamp in std::iter::successors(Some(self), |stamp| stamp.below) {
            stamp.level.modules.iter().for_each(|module| out(*module));
        }
    }

    /// Every type a level names, which a call that replays it reads.
    pub fn each_type(&self, out: &mut dyn FnMut(DeclaredType<Parametric>)) {
        for stamp in std::iter::successors(Some(self), |stamp| stamp.below) {
            each_type_of(stamp.level.bound, stamp.level.keyed, out);
        }
    }

    /// This stamp laid again in `writer`'s region, each module value through `copy`.
    pub fn copied<'to, Y: Knotted>(
        &self,
        writer: Writer<'to, '_>,
        copy: &mut impl FnMut(&Value<'cell, X>) -> Value<'to, Y>,
    ) -> Stamp<'to, Y> {
        let level = self.level;
        let modules = writer.fill(level.modules.len(), |at| copy(&level.modules[at]));
        let level = resident(
            writer,
            Level {
                base: level.base,
                key: level.key,
                body: level.body,
                bound: collect(writer, level.bound.iter().copied()),
                keyed: collect(writer, level.keyed.iter().copied()),
                modules,
            },
        );
        let below = (self.below).map(|below| resident(writer, below.copied(writer, copy)));
        Stamp { level, below }
    }

    /// What [`copied`](Self::copied) writes past the stamp's own two words: each level and its
    /// runs, and what each module value points at.
    pub fn weight(&self) -> Weight {
        std::iter::successors(Some(self), |stamp| stamp.below).fold(
            Weight::ZERO,
            |weight, stamp| {
                let level = stamp.level;
                let modules = (level.modules.iter()).fold(Weight::ZERO, |weight, module| {
                    weight
                        .plus(Weight::flat::<Value<'_, X>>())
                        .plus(module.weight())
                });
                weight
                    .plus(Weight::flat::<Level<'_, X>>())
                    .plus(Weight::flat::<Stamp<'_, X>>())
                    .plus(Weight::run::<Asked>(level.bound.len()))
                    .plus(Weight::run::<KeyedTypes>(level.keyed.len()))
                    .plus(modules)
            },
        )
    }
}
