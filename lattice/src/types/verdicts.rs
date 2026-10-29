//! The verdict tables: caches of relation answers each tier keeps beside its nodes.
//!
//! A verdict is keyed by `(subject digest, candidate digest, relation)` ([`VerdictKey`]). A
//! verdict over a digest pair is a pure function — once computed it never changes — so verdicts
//! are never load-bearing: a forgotten one costs a re-walk of the relation, never a wrong answer.
//! That leaves each tier free to choose what it keeps.
//!
//! Every table keeps every verdict it is given, as [`VerdictEntry`]s:
//!
//! - The **load's** is read-only and complete for what the load asked: its builder records into a
//!   [`DigestTable`](crate::bump::DigestTable) laid at the first verdict, which evicts nothing, and
//!   the freeze lays it into a [`FrozenTable`](crate::bump::FrozenTable). No run writes it.
//! - A **branch's** is insert-only: a digest table laid in its half at its first verdict and
//!   released with it.
//!
//! The same tables keep a **memo** for a walk a downstream crate runs over type nodes
//! ([`MemoKind`]): a pure answer about types alone — a flag over two types, or a type for one
//! member of a set of types, keyed by the set's [`group_digest`](super::digest::group_digest) and
//! the member's — and as never load-bearing as a verdict.
//!
//! The chain decides which tier records a verdict ([chain.rs](super::chain)); the relations that
//! read and record verdicts live in [`order`](super::order), [`ranking`](super::ranking) and
//! [`unify`](super::unify).

use crate::bump::Keyed;

use super::digest::TypeDigest;
use super::handle::Handle;

/// A walk a downstream crate memoizes in the lattice's tables: each names one pure question over
/// types, which the crate defines and the lattice never reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoKind {
    /// A container's fill: the type a member of a set of types fills to, merged with the rest.
    Fill,
    /// Whether a value held at one type hides a field it would show at another.
    Hides,
}

/// Which question a recorded verdict answers. They never alias — each digest domain is disjoint by
/// construction, and a class verdict names its class — but the enum still keys the table
/// explicitly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Relation {
    /// [`is_subtype_of`](super::order::is_subtype_of), the one order.
    Subtype,
    /// [`fits`](super::order::fits), the relation a question reads.
    Fits,
    /// [`member_at_least`](super::unify::member_at_least).
    MemberAtLeast,
    /// [`class_at_least`](super::ranking::class_at_least) at the class it names.
    ClassAtLeast(u16),
    /// A downstream memo ([`MemoKind`]).
    Memo(MemoKind),
}

impl Relation {
    /// The bits a key's fold mixes in: distinct per relation and per class.
    fn bits(self) -> u64 {
        match self {
            Relation::Subtype => 0,
            Relation::Fits => 1,
            Relation::MemberAtLeast => 2,
            Relation::ClassAtLeast(class) => 3 + u64::from(class),
            Relation::Memo(kind) => (1 << 16) + 3 + kind as u64,
        }
    }
}

/// What a verdict is found by: the two types and the question.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct VerdictKey {
    pub subject: TypeDigest,
    pub candidate: TypeDigest,
    pub relation: Relation,
}

impl VerdictKey {
    /// The key's uniformly distributed word. A digest is already a uniformly distributed hash, so
    /// the two digests' low words fold together — rotated apart, so `(a, b)` and `(b, a)` differ —
    /// with the relation mixed in. Every table compares the whole key, so a fold collision costs a
    /// probe and never a wrong verdict.
    fn fold(self) -> u64 {
        (self.subject.0 as u64).rotate_left(32) ^ (self.candidate.0 as u64) ^ self.relation.bits()
    }
}

/// What an entry answers: a verdict, or a type a memo records, read as its key's relation says.
/// One word, so an entry is no larger for holding a type.
#[derive(Clone, Copy)]
pub(super) struct Answer(u128);

impl Answer {
    pub(super) fn of_verdict(verdict: bool) -> Self {
        Answer(u128::from(verdict))
    }

    pub(super) fn of_type(handle: Handle) -> Self {
        Answer(handle.bits())
    }

    pub(super) fn verdict(self) -> bool {
        self.0 != 0
    }

    pub(super) fn handle(self) -> Handle {
        Handle::from_bits(self.0)
    }
}

/// One verdict a tier keeps: its key beside its answer. Laid in the tier's storage.
#[derive(Clone, Copy)]
pub(super) struct VerdictEntry {
    key: VerdictKey,
    answer: Answer,
}

impl VerdictEntry {
    pub(super) fn new(key: VerdictKey, answer: Answer) -> Self {
        VerdictEntry { key, answer }
    }

    pub(super) fn answer(&self) -> Answer {
        self.answer
    }
}

// A type answer costs an entry nothing: the key's digests already align it to a word.
const _: () = assert!(std::mem::size_of::<VerdictEntry>() == 64);

impl Keyed for VerdictEntry {
    type Key = VerdictKey;

    fn key(&self) -> VerdictKey {
        self.key
    }

    fn word(key: VerdictKey) -> u64 {
        key.fold()
    }
}
