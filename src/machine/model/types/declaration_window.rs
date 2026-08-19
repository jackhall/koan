//! The **ambient** declaration window — the record a module body's pre-announced type declarations
//! elaborate against — and the two views every consult path shares with the declarator-local
//! [`RecursiveGroupWindow`].
//!
//! A module body announces its top-level `NEWTYPE` / `UNION` declarations before any of them runs,
//! so a name declared later in the body is already visible: a cross-reference lowers to that
//! member's relative [`TypeNode::Sibling`] handle and the seal rewrites it absolute. That is what
//! lets a plain `MODULE` host a mutually-recursive group.
//!
//! # Why this representation, and not [`RecursiveGroupWindow`]
//!
//! An ambient window rides a scope, so it must cost the region teardown nothing: every field here
//! is a bumped `Copy` run or a [`Cell`] of one, and the record itself sits inline in
//! [`ScopeKind::Module`](crate::machine::core::ScopeKind) — the scope allocation is already the
//! arena-hosted resident, so the window needs no allocation and no `Rc` of its own. Two properties
//! make that possible: an announced member is always [`KKind::NewType`]-schema'd, so its fill is a
//! bare `KType`, and the member set is fixed at the scan, so the run never grows. A declarator that
//! needs neither — a standalone declaration, a generative `:|` mint — keeps the std-owned
//! [`RecursiveGroupWindow`], which carries `TypeConstructor` schemas and grows by threaded
//! discovery. Both seal through the one pure core, [`seal_group`].
//!
//! # Owned members
//!
//! A `UNION`'s variants are announced as members **owned by their binder**: never
//! bare-name-resolvable, never written into `bindings.types`, reachable only through the binder
//! (`:Tree`) or the qualified sigil (`:(Tree Node)`). Two binders may own the same bare tag without
//! colliding — qualified lookup is scoped by the binder's own member list, and the owner is a
//! canonical-order tiebreak that never enters the digest.

use std::cell::Cell;

use crate::machine::core::RegionBrand;

use super::kkind::KKind;
use super::ktype::KType;
use super::node::TypeNode;
use super::recursive_group_window::{
    RecursiveGroupWindow, RelativeSchema, SealBinderInput, SealMemberInput, seal_group,
};
use super::registry::TypeRegistry;

/// One announced member: its declared name and the binder that owns it. `Copy` and pointer-only,
/// so the run bumps into the declaring scope's region.
#[derive(Clone, Copy)]
pub struct AnnouncedMember<'a> {
    /// The declared name — the bare tag for a variant.
    pub name: &'a str,
    /// The binder owning this member, or `None` for a standalone type.
    pub owner: Option<&'a str>,
}

/// One declaring binder and the indices of the members it owns.
#[derive(Clone, Copy)]
pub struct AnnouncedBinder<'a> {
    pub name: &'a str,
    pub members: &'a [usize],
}

/// What an ambient window's seal minted: one absolute handle per member in announcement order and
/// one union handle per binder. Bumped by the sealing statement into the window's own region.
#[derive(Clone, Copy)]
pub struct SealedAnnounced<'a> {
    members: &'a [KType],
    binder_types: &'a [(&'a str, KType)],
}

impl<'a> SealedAnnounced<'a> {
    /// The absolute handle of the member at `index`.
    pub fn member(&self, index: usize) -> Option<KType> {
        self.members.get(index).copied()
    }

    /// The union handle binder `name` denotes.
    pub fn binder_type(&self, name: &str) -> Option<KType> {
        self.binder_types
            .iter()
            .find(|(binder, _)| *binder == name)
            .map(|(_, kt)| *kt)
    }

    /// Every `(name, handle)` a seal installs into `bindings.types`: the standalone members and the
    /// binders. Variants are absent — they are reached through their binder, not by name.
    pub fn installable(&self, members: &[AnnouncedMember<'a>]) -> Vec<(&'a str, KType)> {
        members
            .iter()
            .enumerate()
            .filter(|(_, member)| member.owner.is_none())
            .map(|(index, member)| (member.name, self.members[index]))
            .chain(self.binder_types.iter().copied())
            .collect()
    }
}

/// The owned scan product a module body hands its scope door: the announced members in
/// announcement order and each binder's owned indices. Plain owned data — the door bumps it.
#[derive(Default)]
pub struct AnnouncedData {
    /// `(name, owner)` per member, in announcement order.
    pub members: Vec<(String, Option<String>)>,
    /// `(binder, owned member indices)`.
    pub binders: Vec<(String, Vec<usize>)>,
}

impl AnnouncedData {
    /// Whether the scan announced anything at all; an empty scan mints no window.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Announce a standalone member, yielding its index.
    pub fn announce(&mut self, name: String) -> usize {
        self.members.push((name, None));
        self.members.len() - 1
    }

    /// Announce `binder`'s variants — one owned member per tag — and register the binder.
    pub fn announce_binder(&mut self, binder: String, tags: Vec<String>) {
        let owned = tags
            .into_iter()
            .map(|tag| {
                self.members.push((tag, Some(binder.clone())));
                self.members.len() - 1
            })
            .collect();
        self.binders.push((binder, owned));
    }

    /// Whether a standalone member of this name is already announced.
    pub fn declares(&self, name: &str) -> bool {
        self.members
            .iter()
            .any(|(member, owner)| owner.is_none() && member == name)
    }

    /// Whether `name` is already a declaring binder.
    pub fn binds(&self, name: &str) -> bool {
        self.binders.iter().any(|(binder, _)| binder == name)
    }
}

/// The ambient declaration window a module body's announced types elaborate against. Drop-free:
/// two bumped `Copy` runs and two `Cell`s of bumped references, so the scope carrying it costs
/// region teardown nothing.
pub struct AnnouncedWindow<'a> {
    members: &'a [AnnouncedMember<'a>],
    binders: &'a [AnnouncedBinder<'a>],
    /// Every member's fill, replaced whole on each fill. One bumped run per fill keeps the whole
    /// window `Copy`-storable — a per-member `Cell` could not be bumped at all — and the member
    /// count is a handful, so the copies are free.
    fills: Cell<&'a [Option<KType>]>,
    /// What the last fill's seal minted. `None` while the window is open.
    sealed: Cell<Option<&'a SealedAnnounced<'a>>>,
}

impl<'a> AnnouncedWindow<'a> {
    /// Bump `data`'s names and index runs into `brand`'s region and open the window over them. The
    /// caller is the scope-construction door, so `brand` is the region the carrying scope lives in.
    pub fn bump(brand: RegionBrand<'a>, data: &AnnouncedData) -> AnnouncedWindow<'a> {
        AnnouncedWindow {
            members: brand
                .allocator()
                .slice_from_iter(data.members.iter().map(|(name, owner)| AnnouncedMember {
                    name: brand.allocator().text(name),
                    owner: owner.as_deref().map(|o| brand.allocator().text(o)),
                })),
            binders: brand
                .allocator()
                .slice_from_iter(data.binders.iter().map(|(name, owned)| AnnouncedBinder {
                    name: brand.allocator().text(name),
                    members: brand.allocator().slice(owned),
                })),
            // Reserved and filled straight from the repeat, so an unfilled window costs the region
            // one run and the heap nothing.
            fills: Cell::new(
                brand
                    .allocator()
                    .slice_from_iter(std::iter::repeat_n(None, data.members.len())),
            ),
            sealed: Cell::new(None),
        }
    }

    /// The announced members, in announcement order.
    pub fn members(&self) -> &'a [AnnouncedMember<'a>] {
        self.members
    }

    /// Index of the standalone member named `name`. An owned member never answers here.
    pub fn member_index(&self, name: &str) -> Option<usize> {
        self.members
            .iter()
            .position(|m| m.owner.is_none() && m.name == name)
    }

    /// Index of the member `binder` owns under the bare tag `tag`.
    pub fn variant_index(&self, binder: &str, tag: &str) -> Option<usize> {
        let owned = self.binders.iter().find(|b| b.name == binder)?;
        owned
            .members
            .iter()
            .copied()
            .find(|index| self.members[*index].name == tag)
    }

    /// Whether `name` is a declaring binder of this window.
    pub fn binds(&self, name: &str) -> bool {
        self.binders.iter().any(|b| b.name == name)
    }

    /// The relative type binder `name` denotes while the window is open: the union of the members
    /// it owns, each as its sibling handle.
    pub fn binder_union(&self, name: &str, types: &TypeRegistry) -> Option<KType> {
        let binder = self.binders.iter().find(|b| b.name == name)?;
        Some(
            types.union_of(
                binder
                    .members
                    .iter()
                    .map(|index| types.intern(TypeNode::Sibling(*index)))
                    .collect(),
            ),
        )
    }

    /// Whether the member at `index` has had its declaration's finalize run.
    pub fn member_is_filled(&self, index: usize) -> bool {
        self.fills.get().get(index).is_some_and(Option::is_some)
    }

    /// Every still-unfilled member, as `(name, owner)`. A consumer parks on exactly this set's
    /// producers; a variant's producer is its owning binder's, since a variant stamps none itself.
    pub fn unfilled_members(&self) -> Vec<(&'a str, Option<&'a str>)> {
        let fills = self.fills.get();
        self.members
            .iter()
            .enumerate()
            .filter(|(index, _)| fills[*index].is_none())
            .map(|(_, member)| (member.name, member.owner))
            .collect()
    }

    /// What the seal minted, or `None` while the window is open.
    pub fn sealed(&self) -> Option<&'a SealedAnnounced<'a>> {
        self.sealed.get()
    }

    /// Whether every announced member has filled and the window has computed its identities.
    pub fn is_sealed(&self) -> bool {
        self.sealed.get().is_some()
    }

    /// Fill member `index`'s representation and, if that was the last unfilled member, seal. Seals
    /// at the **last fill** rather than at module close, so a consumer parked on the group wakes as
    /// early as the identities exist. Returns what the seal minted on the fill that seals (and on
    /// any later call once sealed), `None` while members remain open.
    ///
    /// `brand` is the carrying scope's region: the seal's own runs are bumped beside the window.
    pub fn fill(
        &self,
        index: usize,
        repr: KType,
        brand: RegionBrand<'a>,
        types: &TypeRegistry,
    ) -> Option<&'a SealedAnnounced<'a>> {
        // The replacement run is filled from the old one with `index`'s slot swapped, so the fill
        // needs no owned copy to mutate — and every read below is off the bumped run.
        let fills = brand.allocator().slice_from_iter(
            self.fills
                .get()
                .iter()
                .enumerate()
                .map(|(slot, fill)| if slot == index { Some(repr) } else { *fill }),
        );
        self.fills.set(fills);
        if let Some(sealed) = self.sealed.get() {
            return Some(sealed);
        }
        if fills.iter().any(Option::is_none) {
            return None;
        }
        let inputs: Vec<SealMemberInput<'_>> = self
            .members
            .iter()
            .zip(fills.iter())
            .map(|(member, fill)| SealMemberInput {
                name: member.name,
                owner: member.owner,
                kind: KKind::NewType,
                schema: RelativeSchema::NewType(fill.expect("every member filled")),
            })
            .collect();
        let binder_inputs: Vec<SealBinderInput<'_>> = self
            .binders
            .iter()
            .map(|binder| SealBinderInput {
                name: binder.name,
                members: binder.members,
            })
            .collect();
        // An ambient window is never generative: a `:|` mint opens its own declarator-local one.
        let group = seal_group(&inputs, &binder_inputs, None, types);
        // `binder_types` is filled first because `members` consumes `group`'s run, and a field
        // initializer runs in source order.
        let sealed = brand.allocator().value(SealedAnnounced {
            binder_types: brand.allocator().slice_from_iter(
                group
                    .binder_types
                    .iter()
                    .zip(self.binders.iter())
                    .map(|((_, kt), binder)| (binder.name, *kt)),
            ),
            members: brand.allocator().slice_from_iter(group.members),
        });
        self.sealed.set(Some(sealed));
        Some(sealed)
    }
}

/// The declaration window a declarator elaborates and seals against, held by value for the whole
/// declaration: the ambient one when the enclosing module body announced this name, else a fresh
/// window this declaration owns outright. A consult borrows a [`WindowView`] out of it per phase —
/// a view is never held across the move into a deferral.
pub enum DeclWindow<'a> {
    Ambient(&'a AnnouncedWindow<'a>),
    Owned(RecursiveGroupWindow),
}

impl<'a> DeclWindow<'a> {
    /// The borrowed view every consult path reads.
    pub fn view(&self) -> WindowView<'_, 'a> {
        match self {
            DeclWindow::Ambient(window) => WindowView::Announced(window),
            DeclWindow::Owned(window) => WindowView::Local(window),
        }
    }

    /// Fill member `index`'s representation, returning whether that fill sealed the window. Every
    /// member either representation admits here is [`KKind::NewType`]-schema'd, which is what lets
    /// the ambient window hold its fills as bare handles.
    pub fn fill(
        &self,
        index: usize,
        repr: KType,
        brand: RegionBrand<'a>,
        types: &TypeRegistry,
    ) -> bool {
        match self {
            DeclWindow::Ambient(window) => window.fill(index, repr, brand, types).is_some(),
            DeclWindow::Owned(window) => window
                .fill_member(index, RelativeSchema::NewType(repr), types)
                .is_some(),
        }
    }
}

/// A borrowed declaration window, ambient or declarator-local — the currency the elaborator and the
/// field-list walker consult. `Copy`, so it re-borrows freely inside one elaboration.
#[derive(Clone, Copy)]
pub enum WindowView<'v, 'a> {
    Announced(&'v AnnouncedWindow<'a>),
    Local(&'v RecursiveGroupWindow),
}

impl<'v, 'a> WindowView<'v, 'a> {
    /// Whether the window has computed its members' identities.
    pub fn is_sealed(self) -> bool {
        match self {
            WindowView::Announced(w) => w.is_sealed(),
            WindowView::Local(w) => w.is_sealed(),
        }
    }

    /// Index of the standalone member named `name`.
    pub fn member_index(self, name: &str) -> Option<usize> {
        match self {
            WindowView::Announced(w) => w.member_index(name),
            WindowView::Local(w) => w.member_index(name),
        }
    }

    /// Index of the member `binder` owns under the bare tag `tag`.
    pub fn variant_index(self, binder: &str, tag: &str) -> Option<usize> {
        match self {
            WindowView::Announced(w) => w.variant_index(binder, tag),
            WindowView::Local(w) => w.variant_index(binder, tag),
        }
    }

    /// Whether `name` is a declaring binder of this window.
    pub fn binds(self, name: &str) -> bool {
        match self {
            WindowView::Announced(w) => w.binds(name),
            WindowView::Local(w) => w.binds(name),
        }
    }

    /// The relative union binder `name` denotes while the window is open.
    pub fn binder_union(self, name: &str, types: &TypeRegistry) -> Option<KType> {
        match self {
            WindowView::Announced(w) => w.binder_union(name, types),
            WindowView::Local(w) => w.binder_union(name, types),
        }
    }

    /// Whether the member at `index` has had its declaration's finalize run.
    pub fn member_is_filled(self, index: usize) -> bool {
        match self {
            WindowView::Announced(w) => w.member_is_filled(index),
            WindowView::Local(w) => w.member_is_filled(index),
        }
    }

    /// The absolute handle of member `index`, once sealed.
    pub fn sealed_member(self, index: usize) -> Option<KType> {
        match self {
            WindowView::Announced(w) => w.sealed().and_then(|s| s.member(index)),
            WindowView::Local(w) => w.sealed().and_then(|s| s.members.get(index).copied()),
        }
    }

    /// The absolute union handle binder `name` denotes, once sealed.
    pub fn sealed_binder(self, name: &str) -> Option<KType> {
        match self {
            WindowView::Announced(w) => w.sealed().and_then(|s| s.binder_type(name)),
            WindowView::Local(w) => w.sealed().and_then(|s| s.binder_type(name)),
        }
    }

    /// Every still-unfilled member as `(name, owner)`. A consumer parks on exactly this set's
    /// producers; the owner is who carries the placeholder, since a variant stamps none itself.
    pub fn unfilled_members(self) -> Vec<(String, Option<String>)> {
        match self {
            WindowView::Announced(w) => w
                .unfilled_members()
                .into_iter()
                .map(|(name, owner)| (name.to_string(), owner.map(str::to_string)))
                .collect(),
            WindowView::Local(w) => w.unfilled_members(),
        }
    }

    /// Every `(name, handle)` this window's seal installs into `bindings.types`: the standalone
    /// members and the binders. Empty while the window is open.
    pub fn installable(self) -> Vec<(String, KType)> {
        match self {
            WindowView::Announced(w) => match w.sealed() {
                Some(sealed) => sealed
                    .installable(w.members())
                    .into_iter()
                    .map(|(name, kt)| (name.to_string(), kt))
                    .collect(),
                None => Vec::new(),
            },
            WindowView::Local(w) => w.installable(),
        }
    }

    /// Every announced member name a declarator threads, so a sub-dispatched sigil body resolves
    /// co-declared references against this window before it leaves for the standalone dispatcher.
    /// Owned members are excluded: a variant is reached only through the qualified sigil, which the
    /// field walker lowers in place.
    pub fn threadable_names(self) -> Vec<String> {
        match self {
            WindowView::Announced(w) => w
                .members()
                .iter()
                .filter(|m| m.owner.is_none())
                .map(|m| m.name.to_string())
                .chain(w.binders.iter().map(|b| b.name.to_string()))
                .collect(),
            WindowView::Local(w) => w.bare_reachable_names(),
        }
    }

    /// Announce-if-missing: the relative handle naming `name`, minting the member when this window
    /// discovers its own members as it walks. Only a declarator-local window grows.
    pub fn sibling(self, name: &str, kind: KKind, types: &TypeRegistry) -> Option<KType> {
        match self {
            WindowView::Announced(_) => None,
            WindowView::Local(w) => Some(w.sibling(name, kind, types)),
        }
    }
}
