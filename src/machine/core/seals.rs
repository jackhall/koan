//! The registration bundles a dispatch or operator write takes: everything a bucket entry needs
//! about a callable or an operator group, computed at seal time — the one moment the carrier is
//! open under its home pin — so no write verb ever opens a carrier itself.

use crate::machine::core::{DeliveredFunction, SealedFunction};
use crate::machine::model::{DeliveredOperatorGroup, SealedOperatorGroup};

use crate::machine::model::{DispatchToken, OperatorGroup};

use crate::parse::{KeywordSymbol, UntypedKey};

use super::scope::Scope;

/// Everything a dispatch-bucket registration needs about a callable, computed at seal time — the
/// one moment the callable is open under its home pin — so no write verb ever opens a carrier.
/// `sealed` is what the `functions` bucket stores; the rest is plain data with no region lifetime.
///
/// A keyworded expression becomes dispatchable by registering one of these: the dispatch bucket is
/// private to its tables and its write verb (`write_overload`) takes an `OverloadSeal` by value, so
/// a bucket entry cannot exist without one. Binding a function *value* (`LET g = (f)`) publishes
/// nothing here: a value binding is called by name.
pub(crate) struct OverloadSeal<'a> {
    /// The dormant callable carrier the dispatch bucket stores.
    pub sealed: SealedFunction<'a>,
    /// `signature.untyped_key()` — the bucket this callable belongs in.
    pub key: UntypedKey,
    /// `signature.dispatch_token()` — the stored form of the duplicate-overload predicate, and
    /// what the `DuplicateOverload` diagnostic renders the colliding overload from.
    pub token: DispatchToken,
}

impl<'a> OverloadSeal<'a> {
    /// The bundle for a callable **fresh from its witnessed birth** — the `FN` / `OP` registration
    /// doors and the builtin seeds. Nothing is minted here: the description the bucket stores is the
    /// one [`KFunction::alloc_captured`] composed, naming the callable's home region as its host and
    /// its one member, so the reach claim the bucket carries is the birth's derived fact rather than
    /// a restatement at the registration site.
    ///
    /// `scope` must be the defining scope — the region the callable was born into. The envelope's
    /// coverage is lodged there ([`Delivered::rest_in`](crate::memory::Delivered::rest_in)),
    /// which the library's self rule makes free for a value already resident in it. Everything the
    /// bucket write keys on is read inside the envelope's own open and travels as plain data.
    pub(crate) fn of_delivered(scope: &'a Scope<'a>, cell: &DeliveredFunction) -> Self {
        let (key, token) = cell.open(|f| (f.signature.untyped_key(), f.signature.dispatch_token()));
        OverloadSeal {
            sealed: cell.rest_in(scope.brand().handle()),
            key,
            token,
        }
    }
}

/// Everything an operator-registry registration needs about a group, computed at seal time — the
/// one moment the record is open — so no write verb ever opens a carrier. `sealed` is what the
/// `operators` table stores; the rest is plain data with no region lifetime. The operator-table twin
/// of [`OverloadSeal`].
///
/// One of these backs a whole `GROUP` declaration: every powerset key of the install names the same
/// record, so the write applies this one bundle across all of its probe keys.
#[derive(Clone)]
pub(crate) struct GroupSeal<'a> {
    /// The dormant group carrier the registry entry stores.
    pub sealed: SealedOperatorGroup<'a>,
    /// The record's address — the upsert's **cheap** identity arm. Every powerset key of one
    /// declaration shares one pointee, so re-registering a key that is already installed compares
    /// equal here without ever touching the record.
    pub address: usize,
    /// `OperatorGroup::declaration_key()` — the stored form of the upsert's **structural** arm, for
    /// the two-`OP`-statements case where one declaration allocates two records.
    pub declaration: String,
    /// What re-birthing the record itself costs a copy — the `OperatorGroup` struct beside its
    /// member slice. Read here, where the record is open, so the cost memo prices an operator
    /// install without reopening a carrier. A powerset install shares one record across every
    /// subset entry, so the registry writes charge this once per record rather than once per key.
    pub record_bytes: u64,
}

impl<'a> GroupSeal<'a> {
    /// The bundle for a group record **fresh from its yoked birth** — the `GROUP` binder, the `OP`
    /// declaration doors, and the builtin seeds, each of which births its record at the very region
    /// it registers against ([`Scope::birth_operator_group`](crate::machine::core::Scope)). Nothing
    /// is minted here: the yoke brand is the compile-time proof that the record is region-pure —
    /// [`OperatorGroup::alloc`](crate::machine::model::OperatorGroup::alloc) re-homes every byte it stores at the brand it is handed, so no
    /// foreign borrow can inhabit the built value — and the description the birth composed says
    /// exactly that: hosted at the declaring region, with no members. The seal rests that envelope.
    ///
    /// `scope` must be the declaring scope, the region the record was born into; resting there is
    /// free under the library's self rule. The address and the declaration key are both read inside
    /// the envelope's own open and travel as region-free data.
    pub(crate) fn of_delivered(scope: &'a Scope<'a>, cell: &DeliveredOperatorGroup) -> Self {
        let (address, declaration, record_bytes) = cell.open(|group| {
            (
                std::ptr::from_ref(group) as usize,
                group.declaration_key(),
                (size_of::<OperatorGroup<'static>>()
                    + group.member_symbols().count() * size_of::<KeywordSymbol>())
                    as u64,
            )
        });
        GroupSeal {
            sealed: cell.rest_in(scope.brand().handle()),
            address,
            declaration,
            record_bytes,
        }
    }
}
