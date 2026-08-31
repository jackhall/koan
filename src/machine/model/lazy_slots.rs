//! Lazy-slot model: which child slots of a builtin form stay raw instead of evaluating.
//!
//! A bare `(…)` evaluates before its parent dispatches — everywhere except a lazy slot of a fixed
//! builtin form. Which slots those are is a **seal-time** fact, not a dispatch-time one: the static
//! table here ([`LAZY_SLOT_SPECS`]) is the single source of truth, a `KExpression`'s seal stamps the
//! matched entry onto the node, and the scheduler reads the stamp to decide child submission. So
//! dispatch selects among overloads over values that have already landed, and a reader can tell
//! locally whether a group runs.
//!
//! Recognition is by full untyped bucket key, sound for the same reason
//! [`BINDER_SPECS`](crate::machine::model::binder::BINDER_SPECS) is: builtin buckets are
//! unshadowable, so a node whose key matches an entry can only ever resolve to that builtin's
//! overloads. Lazy-slot declaration is therefore available only to builtin registration — a user
//! `FN` signature never receives a raw unquoted group, and a `:KExpression` parameter of one is an
//! ordinary eager value parameter satisfied by a `#(…)` literal or any `KExpression`-valued
//! expression.
//!
//! The stamp records which part *kinds* stay raw per slot index rather than a per-index boolean,
//! because one bucket mixes raw capture and eager sub-dispatch at the same index across overloads:
//! `NEWTYPE <name> = <repr>` captures a `:(…)` or `:{…}` at index 3 raw while a bare `(…)` there
//! evaluates. The [`LAZY_SLOT_SPECS`] entries are pinned to the live builtin signatures by the
//! spec⟺registration consistency test: index `i` of bucket `k` carries kind `K` iff some builtin
//! overload registered under `k` types slot `i` with `K`'s slot type.

use crate::machine::model::ast::Part;
use crate::machine::model::key_spec::{KEYWORDS, KeyElementSpec, key_matches_parts};
use crate::source::Spanned;

use KeyElementSpec::{Keyword as Kw, Slot};

/// The part kinds a lazy slot can capture raw, as a set. One kind per raw-capture slot type:
/// `CODE` for a `:KExpression` slot (an `(…)` group or a `#(…)` quote — one spelling in a lazy
/// slot, both captured raw), `TYPE_EXPR` for `:SigiledTypeExpr` (`:(…)`), `RECORD_TYPE` for
/// `:RecordType` (`:{…}`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LazyKinds(u8);

impl LazyKinds {
    pub const EMPTY: LazyKinds = LazyKinds(0);
    pub const CODE: LazyKinds = LazyKinds(1);
    pub const TYPE_EXPR: LazyKinds = LazyKinds(1 << 1);
    pub const RECORD_TYPE: LazyKinds = LazyKinds(1 << 2);

    pub const fn with(self, other: LazyKinds) -> LazyKinds {
        LazyKinds(self.0 | other.0)
    }

    pub const fn contains(self, other: LazyKinds) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A builtin form with at least one lazy slot: the untyped bucket key it dispatches under, and the
/// raw-capture kinds each of its lazy slot indices carries. Slots are listed in ascending index
/// order; an index absent from the run carries [`LazyKinds::EMPTY`] and evaluates.
pub struct LazySlotSpec {
    /// Full untyped bucket key — ALL keywords in position, never just the lead keyword.
    pub key: &'static [KeyElementSpec],
    pub slots: &'static [(usize, LazyKinds)],
}

impl LazySlotSpec {
    /// The kinds that stay raw at `index`, empty when the slot evaluates. A linear scan over a run
    /// of at most four pairs, which is cheaper than any index-keyed structure at this size.
    pub fn kinds_at(&self, index: usize) -> LazyKinds {
        self.slots
            .iter()
            .find(|(i, _)| *i == index)
            .map_or(LazyKinds::EMPTY, |(_, kinds)| *kinds)
    }
}

/// The [`LAZY_SLOT_SPECS`] entry `parts` matches, or `None` for a form with no lazy slot — every
/// user-defined bucket among them. The one table probe; a node resolves its stamp through here at
/// seal.
pub fn lazy_slot_spec_for<'a, P: Part<'a>>(parts: &[Spanned<P>]) -> Option<&'static LazySlotSpec> {
    LAZY_SLOT_SPECS
        .iter()
        .find(|spec| key_matches_parts(spec.key, parts))
}

const CODE: LazyKinds = LazyKinds::CODE;
const TYPE_EXPR: LazyKinds = LazyKinds::TYPE_EXPR;
const RECORD_TYPE: LazyKinds = LazyKinds::RECORD_TYPE;

/// The single source of truth for the builtin forms with lazy slots. One entry per distinct untyped
/// bucket key, pinned against the live builtin registration table by the spec⟺registration
/// consistency test — an entry whose builtin was renamed, re-shaped, or dropped fails the suite,
/// and so does a builtin that grows a raw-capture slot without an entry here.
pub static LAZY_SLOT_SPECS: &[LazySlotSpec] = &[
    // MATCH <scrutinee> -> <result type> WITH <branches>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.match_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.with),
            Slot,
        ],
        slots: &[(5, CODE)],
    },
    // TRY <body> -> <result type> WITH <branches>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.try_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.with),
            Slot,
        ],
        slots: &[(1, CODE), (5, CODE)],
    },
    // CATCH <body>
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.catch), Slot],
        slots: &[(1, CODE)],
    },
    // USING <module> SCOPE <body>
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.using), Slot, Kw(&KEYWORDS.scope), Slot],
        slots: &[(3, CODE)],
    },
    // CLOSE OVER <captures> <body>
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.close), Kw(&KEYWORDS.over), Slot, Slot],
        slots: &[(2, CODE), (3, CODE)],
    },
    // <field list> FROM <record>
    LazySlotSpec {
        key: &[Slot, Kw(&KEYWORDS.from), Slot],
        slots: &[(0, CODE)],
    },
    // SIG <name> = <body>
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.sig), Slot, Kw(&KEYWORDS.equals), Slot],
        slots: &[(3, CODE)],
    },
    // UNION <name> = <variants>
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.union), Slot, Kw(&KEYWORDS.equals), Slot],
        slots: &[(3, CODE)],
    },
    // MODULE <name> = <body>
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.module), Slot, Kw(&KEYWORDS.equals), Slot],
        slots: &[(3, CODE)],
    },
    // TYPE <declaration>
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.type_), Slot],
        slots: &[(1, CODE)],
    },
    // NEWTYPE <declaration> — the constructor family.
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.newtype), Slot],
        slots: &[(1, CODE)],
    },
    // NEWTYPE <name> = <representation>: a `:(…)` or `:{…}` representation is captured raw, while a
    // bare `(…)` there evaluates — the mixed index the kind set exists for.
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.newtype), Slot, Kw(&KEYWORDS.equals), Slot],
        slots: &[(3, TYPE_EXPR.with(RECORD_TYPE))],
    },
    // GROUP <name> FOLD LEFT|RIGHT = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(5, CODE)],
    },
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(5, CODE)],
    },
    // GROUP <name> PAIRWISE FOLD <combiner> LEFT|RIGHT = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(4, CODE), (7, CODE)],
    },
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(4, CODE), (7, CODE)],
    },
    // FN <signature> -> <return type>: the type-language declaration form.
    LazySlotSpec {
        key: &[Kw(&KEYWORDS.fn_), Slot, Kw(&KEYWORDS.arrow), Slot],
        slots: &[(1, CODE)],
    },
    // FN <signature> -> <return type> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(1, CODE), (3, TYPE_EXPR), (5, CODE)],
    },
    // LET <name> = FN <signature> -> <return type> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(4, CODE), (6, TYPE_EXPR), (8, CODE)],
    },
    // OP <symbol> OVER <operand> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(1, CODE), (3, TYPE_EXPR), (5, CODE)],
    },
    // OP <symbol> OVER <operand> -> <result> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(1, CODE), (3, TYPE_EXPR), (5, TYPE_EXPR), (7, CODE)],
    },
    // UNARY OP <symbol> OVER <operand> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(2, CODE), (4, TYPE_EXPR), (6, CODE)],
    },
    // UNARY OP <symbol> OVER <operand> -> <result> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(2, CODE), (4, TYPE_EXPR), (6, TYPE_EXPR), (8, CODE)],
    },
    // LET <name> = OP <symbol> OVER <operand> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(4, CODE), (6, TYPE_EXPR), (8, CODE)],
    },
    // LET <name> = OP <symbol> OVER <operand> -> <result> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(4, CODE), (6, TYPE_EXPR), (8, TYPE_EXPR), (10, CODE)],
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(5, CODE), (7, TYPE_EXPR), (9, CODE)],
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> -> <result> = <body>
    LazySlotSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        slots: &[(5, CODE), (7, TYPE_EXPR), (9, TYPE_EXPR), (11, CODE)],
    },
];

#[cfg(test)]
mod tests;
