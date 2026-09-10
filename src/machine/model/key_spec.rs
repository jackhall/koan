//! The builtin form table: every fixed form the machine recognizes, spelled once.
//!
//! A builtin form is recognized by its **full untyped bucket key**, every keyword pinned in
//! position. That recognition is sound because builtin buckets are unshadowable: a node whose key
//! matches a table entry can only ever resolve to that builtin's overloads, and a key the table
//! marks [`reserved`](Form::reserved) is refused to user registration for the same reason.
//!
//! [`FORMS`] is the one table. Each entry carries every fact the machine reads off a form — the
//! binder it installs, the slots that stay raw, whether the shape is reserved — under one
//! [`FormId`] tag. A node resolves its entry once, at construction ([`NodeCache`]), and every
//! later reader indexes by the tag rather than re-walking a key: the close-inference rules
//! ([`CLOSE_RULES`](crate::machine::model::close_inference)) and the miss diagnostics
//! ([`MISS_DIAGNOSTICS`](crate::machine::model::miss_diagnostics::MISS_DIAGNOSTICS)) are
//! `(FormId, …)` pairs and hold no key of their own.
//!
//! [`NodeCache`]: crate::machine::model::ast::NodeCache

use crate::machine::model::KeyElement;
use crate::machine::model::binder::{
    BinderFacts, BinderSurface, fn_def_binder_bucket, identifier_part_binder_name,
    op_def_binder_bucket, type_decl_binder_name, type_part_binder_name,
};
use crate::machine::model::labels::{KeywordSymbol, StaticName};
use crate::machine::model::lazy_slots::LazyKinds;

/// The fixed tokens the builtin forms are spelled with, each declared once and minted once. Every
/// [`FORMS`] entry names its keywords out of this group, and the binder module's reserved-symbol
/// list and its `FN` / `UNARY` position reads compare against the same memoized symbols, so the
/// spelling of a surface token is written in exactly one place.
pub(crate) struct SurfaceKeywords {
    pub(crate) let_: StaticName<KeywordSymbol>,
    pub(crate) type_: StaticName<KeywordSymbol>,
    pub(crate) module: StaticName<KeywordSymbol>,
    pub(crate) group: StaticName<KeywordSymbol>,
    pub(crate) fold: StaticName<KeywordSymbol>,
    pub(crate) left: StaticName<KeywordSymbol>,
    pub(crate) right: StaticName<KeywordSymbol>,
    pub(crate) pairwise: StaticName<KeywordSymbol>,
    pub(crate) equals: StaticName<KeywordSymbol>,
    pub(crate) sig: StaticName<KeywordSymbol>,
    pub(crate) union: StaticName<KeywordSymbol>,
    pub(crate) newtype: StaticName<KeywordSymbol>,
    pub(crate) fn_: StaticName<KeywordSymbol>,
    pub(crate) expr: StaticName<KeywordSymbol>,
    /// The two tokens of a quantifier group's head, `EXPR FOR ALL (<names>) …`.
    pub(crate) for_: StaticName<KeywordSymbol>,
    pub(crate) all: StaticName<KeywordSymbol>,
    pub(crate) arrow: StaticName<KeywordSymbol>,
    pub(crate) op: StaticName<KeywordSymbol>,
    pub(crate) over: StaticName<KeywordSymbol>,
    pub(crate) unary: StaticName<KeywordSymbol>,
    pub(crate) val: StaticName<KeywordSymbol>,
    pub(crate) match_: StaticName<KeywordSymbol>,
    pub(crate) with: StaticName<KeywordSymbol>,
    pub(crate) try_: StaticName<KeywordSymbol>,
    pub(crate) catch: StaticName<KeywordSymbol>,
    pub(crate) using: StaticName<KeywordSymbol>,
    pub(crate) scope: StaticName<KeywordSymbol>,
    pub(crate) close: StaticName<KeywordSymbol>,
    pub(crate) from: StaticName<KeywordSymbol>,
    /// The head of the parse of `$(…)`; also its spelled-out surface.
    pub(crate) eval: StaticName<KeywordSymbol>,
    /// The head of the parse of `<record>.<field>`.
    pub(crate) attr: StaticName<KeywordSymbol>,
    /// The pattern-guard sigil `:|`.
    pub(crate) guard: StaticName<KeywordSymbol>,
    /// The otherwise-guard sigil `:!`.
    pub(crate) otherwise: StaticName<KeywordSymbol>,
}

pub(crate) static KEYWORDS: SurfaceKeywords = SurfaceKeywords {
    let_: crate::static_name!(KeywordSymbol, "LET"),
    type_: crate::static_name!(KeywordSymbol, "TYPE"),
    module: crate::static_name!(KeywordSymbol, "MODULE"),
    group: crate::static_name!(KeywordSymbol, "GROUP"),
    fold: crate::static_name!(KeywordSymbol, "FOLD"),
    left: crate::static_name!(KeywordSymbol, "LEFT"),
    right: crate::static_name!(KeywordSymbol, "RIGHT"),
    pairwise: crate::static_name!(KeywordSymbol, "PAIRWISE"),
    equals: crate::static_name!(KeywordSymbol, "="),
    sig: crate::static_name!(KeywordSymbol, "SIG"),
    union: crate::static_name!(KeywordSymbol, "UNION"),
    newtype: crate::static_name!(KeywordSymbol, "NEWTYPE"),
    fn_: crate::static_name!(KeywordSymbol, "FN"),
    expr: crate::static_name!(KeywordSymbol, "EXPR"),
    for_: crate::static_name!(KeywordSymbol, "FOR"),
    all: crate::static_name!(KeywordSymbol, "ALL"),
    arrow: crate::static_name!(KeywordSymbol, "->"),
    op: crate::static_name!(KeywordSymbol, "OP"),
    over: crate::static_name!(KeywordSymbol, "OVER"),
    unary: crate::static_name!(KeywordSymbol, "UNARY"),
    val: crate::static_name!(KeywordSymbol, "VAL"),
    match_: crate::static_name!(KeywordSymbol, "MATCH"),
    with: crate::static_name!(KeywordSymbol, "WITH"),
    try_: crate::static_name!(KeywordSymbol, "TRY"),
    catch: crate::static_name!(KeywordSymbol, "CATCH"),
    using: crate::static_name!(KeywordSymbol, "USING"),
    scope: crate::static_name!(KeywordSymbol, "SCOPE"),
    close: crate::static_name!(KeywordSymbol, "CLOSE"),
    from: crate::static_name!(KeywordSymbol, "FROM"),
    eval: crate::static_name!(KeywordSymbol, "EVAL"),
    attr: crate::static_name!(KeywordSymbol, "ATTR"),
    guard: crate::static_name!(KeywordSymbol, ":|"),
    otherwise: crate::static_name!(KeywordSymbol, ":!"),
};

/// One element of a static bucket key: a fixed keyword token or a slot. [`FORMS`] is `static`, so a
/// keyword rests as one of the [`KEYWORDS`] names and matching compares its memoized symbol against
/// the symbol a stored key carries — a table probe is a walk over short runs that hashes nothing
/// past each name's first touch.
pub enum KeyElementSpec {
    Keyword(&'static StaticName<KeywordSymbol>),
    Slot,
}

impl KeyElementSpec {
    /// True iff `element` fills this position: a spec keyword against the key's own symbol, a spec
    /// slot against any non-keyword position.
    fn matches(&self, element: KeyElement) -> bool {
        match (self, element) {
            (KeyElementSpec::Keyword(name), KeyElement::Keyword(symbol)) => name.symbol() == symbol,
            (KeyElementSpec::Slot, KeyElement::Slot) => true,
            _ => false,
        }
    }
}

/// True iff `spec` matches `key` element-for-element. The one comparison in the tree: every table
/// probe feeds it a node's stored key, and the parser's pre-freeze admission feeds it the key
/// elements its parts run spells.
pub fn key_matches(
    spec: &[KeyElementSpec],
    key: impl ExactSizeIterator<Item = KeyElement>,
) -> bool {
    spec.len() == key.len()
        && spec
            .iter()
            .zip(key)
            .all(|(element, actual)| element.matches(actual))
}

/// One builtin form, recognized by its full bucket key. Every key is spelled here and nowhere else.
pub struct Form {
    pub id: FormId,
    /// Full untyped bucket key — ALL keywords in position, never just the lead keyword.
    pub key: &'static [KeyElementSpec],
    /// What the form installs when submitted as a statement. `None` for a form that binds nothing.
    pub binder: Option<BinderFacts>,
    /// The slots that stay raw, ascending by index. Empty for a form with no lazy slot.
    ///
    /// The stamp records which part *kinds* stay raw per slot rather than a per-index boolean,
    /// because one bucket mixes raw capture and eager sub-dispatch at the same index across
    /// overloads: `NEWTYPE <name> = <repr>` captures a `:(…)` or `:{…}` at index 3 raw while a bare
    /// `(…)` there evaluates.
    pub lazy_slots: &'static [(usize, LazyKinds)],
    /// True when nothing registers under `key`: the shape is a diagnosable mistake, and the
    /// overload write door refuses a user registration there.
    pub reserved: bool,
}

impl Form {
    /// The kinds that stay raw at `index`, empty when the slot evaluates. A linear scan over a run
    /// of at most four pairs, which is cheaper than any index-keyed structure at this size.
    pub fn lazy_kinds_at(&self, index: usize) -> LazyKinds {
        self.lazy_slots
            .iter()
            .find(|(slot, _)| *slot == index)
            .map_or(LazyKinds::EMPTY, |(_, kinds)| *kinds)
    }
}

/// One variant per [`FORMS`] entry, in table order — the tag every other table keys by, so a rule
/// or a diagnostic names a form without respelling its key. The table-order property pins each tag
/// to its index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormId {
    LetValue,
    TypeDeclaration,
    Module,
    GroupFoldLeft,
    GroupFoldRight,
    GroupPairwiseFoldLeft,
    GroupPairwiseFoldRight,
    Sig,
    Union,
    NewTypeDefinition,
    NewTypeDeclaration,
    Val,
    Lambda,
    LambdaType,
    CombinedLambda,
    ExpressionDefinition,
    ExpressionHead,
    QuantifiedExpressionDefinition,
    QuantifiedExpressionHead,
    CombinedExpression,
    CombinedQuantifiedExpression,
    OperatorDefinition,
    OperatorDefinitionReturning,
    UnaryOperatorDefinition,
    UnaryOperatorDefinitionReturning,
    OperatorHead,
    OperatorHeadReturning,
    UnaryOperatorHead,
    UnaryOperatorHeadReturning,
    CombinedOperator,
    CombinedOperatorReturning,
    CombinedUnaryOperator,
    CombinedUnaryOperatorReturning,
    GroupHeadFoldLeft,
    GroupHeadFoldRight,
    GroupHeadPairwiseFoldLeft,
    GroupHeadPairwiseFoldRight,
    Match,
    MatchOver,
    Try,
    Catch,
    UsingScope,
    CloseOver,
    Close,
    Projection,
    Attribute,
    Eval,
}

/// The [`FORMS`] entry `key` matches, or `None` for every user-defined bucket. The one table probe:
/// a node resolves its entry here at construction and caches it, and every later reader goes
/// through the cache.
pub fn form_for(key: impl ExactSizeIterator<Item = KeyElement> + Clone) -> Option<&'static Form> {
    FORMS.iter().find(|form| key_matches(form.key, key.clone()))
}

use KeyElementSpec::{Keyword as Kw, Slot};

const CODE: LazyKinds = LazyKinds::CODE;
const TYPE_EXPR: LazyKinds = LazyKinds::TYPE_EXPR;
const RECORD_TYPE: LazyKinds = LazyKinds::RECORD_TYPE;
/// What a type-position slot captures raw: a `:(…)` type expression or a `:{…}` record type. A
/// bare `(…)` at the same index evaluates.
const RAW_TYPE: LazyKinds = TYPE_EXPR.with(RECORD_TYPE);

/// The single source of truth for the builtin forms. One entry per distinct untyped bucket key, in
/// [`FormId`] order; the keys are pinned against the live builtin registration table by the
/// table⟺registration property, so an entry whose builtin was renamed, re-shaped, or dropped fails
/// the suite, and so does a builtin that grows a raw-capture slot without an entry here.
pub static FORMS: &[Form] = &[
    // ---------- the name binders ----------
    //
    // LET <name> = <value>: value-name overload then type-alias overload.
    Form {
        id: FormId::LetValue,
        key: &[Kw(&KEYWORDS.let_), Slot, Kw(&KEYWORDS.equals), Slot],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name, type_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[],
        reserved: false,
    },
    // TYPE <name> — SIG-body-only abstract-type declarator (bare and higher-kinded share the key).
    Form {
        id: FormId::TypeDeclaration,
        key: &[Kw(&KEYWORDS.type_), Slot],
        binder: Some(BinderFacts {
            names: &[type_decl_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(1, CODE)],
        reserved: false,
    },
    // MODULE <name> = <body> (a module is a value, so the name slot is an `Identifier`; a
    // Type-token name registers nothing and takes the miss table's respelling diagnostic).
    Form {
        id: FormId::Module,
        key: &[Kw(&KEYWORDS.module), Slot, Kw(&KEYWORDS.equals), Slot],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(3, CODE)],
        reserved: false,
    },
    // GROUP <name> FOLD LEFT = <body>.
    Form {
        id: FormId::GroupFoldLeft,
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(5, CODE)],
        reserved: false,
    },
    // GROUP <name> FOLD RIGHT = <body>.
    Form {
        id: FormId::GroupFoldRight,
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(5, CODE)],
        reserved: false,
    },
    // GROUP <name> PAIRWISE FOLD <combiner> LEFT = <body>.
    Form {
        id: FormId::GroupPairwiseFoldLeft,
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
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(4, CODE), (7, CODE)],
        reserved: false,
    },
    // GROUP <name> PAIRWISE FOLD <combiner> RIGHT = <body>.
    Form {
        id: FormId::GroupPairwiseFoldRight,
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
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(4, CODE), (7, CODE)],
        reserved: false,
    },
    // SIG <name> = <body>.
    Form {
        id: FormId::Sig,
        key: &[Kw(&KEYWORDS.sig), Slot, Kw(&KEYWORDS.equals), Slot],
        binder: Some(BinderFacts {
            names: &[type_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(3, CODE)],
        reserved: false,
    },
    // UNION <name> = <schema>.
    Form {
        id: FormId::Union,
        key: &[Kw(&KEYWORDS.union), Slot, Kw(&KEYWORDS.equals), Slot],
        binder: Some(BinderFacts {
            names: &[type_part_binder_name],
            bucket: None,
            surface: BinderSurface::UnionDef,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(3, CODE)],
        reserved: false,
    },
    // NEWTYPE <name> = <repr> (scalar / sigil / record reprs share the key). The repr slot takes a
    // type but is deliberately unmasked: a bare `(…)` there already works by evaluation, so
    // flipping it would change a working spelling's route for nothing. A `:(…)` or `:{…}` there is
    // captured raw while a bare `(…)` evaluates — the mixed index the kind set exists for.
    Form {
        id: FormId::NewTypeDefinition,
        key: &[Kw(&KEYWORDS.newtype), Slot, Kw(&KEYWORDS.equals), Slot],
        binder: Some(BinderFacts {
            names: &[type_part_binder_name],
            bucket: None,
            surface: BinderSurface::NewTypeDef,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(3, RAW_TYPE)],
        reserved: false,
    },
    // NEWTYPE <decl> — constructor family (keyword set {NEWTYPE}, disjoint from the `= _` forms).
    Form {
        id: FormId::NewTypeDeclaration,
        key: &[Kw(&KEYWORDS.newtype), Slot],
        binder: Some(BinderFacts {
            names: &[type_decl_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[(1, CODE)],
        reserved: false,
    },
    // VAL <name> <ty> — a declaration form with no install channel. It records into the decl
    // scope's slot collector, not a binding map any name lookup can see, so it installs nothing; it
    // appears here so the one-place specification of the declaration forms is complete.
    Form {
        id: FormId::Val,
        key: &[Kw(&KEYWORDS.val), Slot, Slot],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        lazy_slots: &[],
        reserved: false,
    },
    // ---------- the function and expression declarators ----------
    //
    // FN <record schema> -> <return type> = <body> — the lambda. It binds nothing: it has no name
    // and no head to key a bucket on. Its binder facts are here for the type slot alone, so a bare
    // `(…)` return spelling rewrites to a sigiled type expression as it does on every other form.
    // The signature slot resolves: a `:{…}` record is a type the lane can evaluate where it stands,
    // unlike a head, whose tokens name nothing until the definition binds them.
    Form {
        id: FormId::Lambda,
        key: &[
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: None,
            type_slots: &[3],
        }),
        lazy_slots: &[(3, RAW_TYPE), (5, CODE)],
        reserved: false,
    },
    // FN <record schema> -> <return type> — the lambda type expression, no body.
    Form {
        id: FormId::LambdaType,
        key: &[Kw(&KEYWORDS.fn_), Slot, Kw(&KEYWORDS.arrow), Slot],
        binder: None,
        lazy_slots: &[],
        reserved: false,
    },
    // LET <name> = FN <signature> -> <return type> = <body> — reserved: a combined statement
    // installs a dispatch bucket, and no `FN` signature has a head to key one on, so nothing
    // registers here. The lazy stamp is what keeps its miss a miss, holding the body raw so the
    // statement reports the shape it got rather than an error from evaluating a body whose
    // parameters no binder ever bound.
    Form {
        id: FormId::CombinedLambda,
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
        binder: None,
        lazy_slots: &[(4, CODE), (6, RAW_TYPE), (8, CODE)],
        reserved: true,
    },
    // EXPR <head> -> <return type> = <body> (every EXPR definition overload shares this key).
    Form {
        id: FormId::ExpressionDefinition,
        key: &[
            Kw(&KEYWORDS.expr),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(fn_def_binder_bucket),
            surface: BinderSurface::Other,
            name_slot: None,
            type_slots: &[3],
        }),
        lazy_slots: &[(1, CODE), (3, RAW_TYPE), (5, CODE)],
        reserved: false,
    },
    // EXPR <head> -> <return type> — the bodyless head, whose carrier is the head's expression
    // shape. Only the head captures raw: it is the declarator's own operand and must reach it
    // unevaluated, while the return slot is an ordinary kind expectation, as on every bodyless head.
    Form {
        id: FormId::ExpressionHead,
        key: &[Kw(&KEYWORDS.expr), Slot, Kw(&KEYWORDS.arrow), Slot],
        binder: None,
        lazy_slots: &[(1, CODE)],
        reserved: false,
    },
    // EXPR FOR ALL <names> <head> -> <return type> = <body>.
    Form {
        id: FormId::QuantifiedExpressionDefinition,
        key: &[
            Kw(&KEYWORDS.expr),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            Slot,
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(fn_def_binder_bucket),
            surface: BinderSurface::Other,
            name_slot: None,
            type_slots: &[6],
        }),
        lazy_slots: &[(3, CODE), (4, CODE), (6, RAW_TYPE), (8, CODE)],
        reserved: false,
    },
    // EXPR FOR ALL <names> <head> -> <return type> — the quantified bodyless head. The names group
    // captures raw beside the head: its tokens name nothing yet, so the lane must not try to
    // resolve them. The return stages too, because it may name a quantifier and so resolves against
    // the group's own scope rather than the surrounding one.
    Form {
        id: FormId::QuantifiedExpressionHead,
        key: &[
            Kw(&KEYWORDS.expr),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            Slot,
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(3, CODE), (4, CODE), (6, RAW_TYPE)],
        reserved: false,
    },
    // LET <name> = FN EXPR <head> -> <return type> = <body> — a combined statement, one binder
    // filling both channels: the LET value name and the bucket key the declaration's body
    // registers under.
    Form {
        id: FormId::CombinedExpression,
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.expr),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(fn_def_binder_bucket),
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[7],
        }),
        lazy_slots: &[(5, CODE), (7, RAW_TYPE), (9, CODE)],
        reserved: false,
    },
    // LET <name> = FN EXPR FOR ALL <names> <head> -> <return type> = <body>.
    Form {
        id: FormId::CombinedQuantifiedExpression,
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.expr),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            Slot,
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(fn_def_binder_bucket),
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[10],
        }),
        lazy_slots: &[(7, CODE), (8, CODE), (10, RAW_TYPE), (12, CODE)],
        reserved: false,
    },
    // ---------- the operator declarators ----------
    //
    // OP <symbol> OVER <operand> = <body>.
    Form {
        id: FormId::OperatorDefinition,
        key: &[
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[3],
        }),
        lazy_slots: &[(1, CODE), (3, RAW_TYPE), (5, CODE)],
        reserved: false,
    },
    // OP <symbol> OVER <operand> -> <return type> = <body>.
    Form {
        id: FormId::OperatorDefinitionReturning,
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
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[3, 5],
        }),
        lazy_slots: &[(1, CODE), (3, RAW_TYPE), (5, RAW_TYPE), (7, CODE)],
        reserved: false,
    },
    // UNARY OP <symbol> OVER <operand> = <body> — reserved: the result segment is mandatory, so
    // this shape's only reading is the mistake the miss table names.
    Form {
        id: FormId::UnaryOperatorDefinition,
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(2, CODE), (4, RAW_TYPE), (6, CODE)],
        reserved: true,
    },
    // UNARY OP <symbol> OVER <operand> -> <return type> = <body>.
    Form {
        id: FormId::UnaryOperatorDefinitionReturning,
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
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[4, 6],
        }),
        lazy_slots: &[(2, CODE), (4, RAW_TYPE), (6, RAW_TYPE), (8, CODE)],
        reserved: false,
    },
    // The SIG-body operator heads — the three definition surfaces minus their `= <body>`. Like
    // `VAL` they install nothing: a head records into the decl scope's collectors, not into a
    // binding map any name lookup can see. They carry binder facts for the `type_slots` mask (so a
    // bare `(LIST OF Elt)` operand reads as a type expression, exactly as it does in the
    // definition) and for the `OperatorDef` marker, which is what a SIG group's member scan keys
    // on. Only the quoted symbol captures raw: the operand and result are ordinary kind
    // expectations, so a `:(…)` there sub-dispatches to a type eagerly, exactly as the bodyless
    // `FN` head's return slot does.
    //
    // OP <symbol> OVER <operand>.
    Form {
        id: FormId::OperatorHead,
        key: &[Kw(&KEYWORDS.op), Slot, Kw(&KEYWORDS.over), Slot],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[3],
        }),
        lazy_slots: &[(1, CODE)],
        reserved: false,
    },
    // OP <symbol> OVER <operand> -> <result>.
    Form {
        id: FormId::OperatorHeadReturning,
        key: &[
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[3, 5],
        }),
        lazy_slots: &[(1, CODE)],
        reserved: false,
    },
    // UNARY OP <symbol> OVER <operand> — the head form, missing its result. Reserved for the same
    // reason the definition form is: the shape has no other reading, and a user form claiming the
    // key would turn the pointed message into a typed miss under its own bucket.
    Form {
        id: FormId::UnaryOperatorHead,
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
        ],
        binder: None,
        lazy_slots: &[],
        reserved: true,
    },
    // UNARY OP <symbol> OVER <operand> -> <result>.
    Form {
        id: FormId::UnaryOperatorHeadReturning,
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
        ],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[4, 6],
        }),
        lazy_slots: &[(2, CODE)],
        reserved: false,
    },
    // LET <name> = OP <symbol> OVER <operand> = <body>.
    Form {
        id: FormId::CombinedOperator,
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
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: Some(1),
            type_slots: &[6],
        }),
        lazy_slots: &[(4, CODE), (6, RAW_TYPE), (8, CODE)],
        reserved: false,
    },
    // LET <name> = OP <symbol> OVER <operand> -> <return type> = <body>.
    Form {
        id: FormId::CombinedOperatorReturning,
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
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: Some(1),
            type_slots: &[6, 8],
        }),
        lazy_slots: &[(4, CODE), (6, RAW_TYPE), (8, RAW_TYPE), (10, CODE)],
        reserved: false,
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> = <body> — reserved, the combined twin of the
    // missing-result mistake.
    Form {
        id: FormId::CombinedUnaryOperator,
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
        binder: None,
        lazy_slots: &[(5, CODE), (7, RAW_TYPE), (9, CODE)],
        reserved: true,
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> -> <return type> = <body> — the two-bucket
    // maximum: a value name and both keys a `UNARY OP` body registers under.
    Form {
        id: FormId::CombinedUnaryOperatorReturning,
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
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: Some(1),
            type_slots: &[7, 9],
        }),
        lazy_slots: &[(5, CODE), (7, RAW_TYPE), (9, RAW_TYPE), (11, CODE)],
        reserved: false,
    },
    // ---------- the SIG-body group heads: the definition spellings minus the name slot ----------
    //
    // GROUP FOLD LEFT = <heads>.
    Form {
        id: FormId::GroupHeadFoldLeft,
        key: &[
            Kw(&KEYWORDS.group),
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(4, CODE)],
        reserved: false,
    },
    // GROUP FOLD RIGHT = <heads>.
    Form {
        id: FormId::GroupHeadFoldRight,
        key: &[
            Kw(&KEYWORDS.group),
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(4, CODE)],
        reserved: false,
    },
    // GROUP PAIRWISE FOLD <combiner> LEFT = <heads>.
    Form {
        id: FormId::GroupHeadPairwiseFoldLeft,
        key: &[
            Kw(&KEYWORDS.group),
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(3, CODE), (6, CODE)],
        reserved: false,
    },
    // GROUP PAIRWISE FOLD <combiner> RIGHT = <heads>.
    Form {
        id: FormId::GroupHeadPairwiseFoldRight,
        key: &[
            Kw(&KEYWORDS.group),
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(3, CODE), (6, CODE)],
        reserved: false,
    },
    // ---------- the control forms ----------
    //
    // MATCH <scrutinee> -> <result type> WITH <branches>.
    Form {
        id: FormId::Match,
        key: &[
            Kw(&KEYWORDS.match_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.with),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(5, CODE)],
        reserved: false,
    },
    // MATCH <scrutinee> OVER <union> -> <result type> WITH <branches>.
    Form {
        id: FormId::MatchOver,
        key: &[
            Kw(&KEYWORDS.match_),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.with),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(7, CODE)],
        reserved: false,
    },
    // TRY <body> -> <result type> WITH <branches>.
    Form {
        id: FormId::Try,
        key: &[
            Kw(&KEYWORDS.try_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.with),
            Slot,
        ],
        binder: None,
        lazy_slots: &[(1, CODE), (5, CODE)],
        reserved: false,
    },
    // CATCH <body>.
    Form {
        id: FormId::Catch,
        key: &[Kw(&KEYWORDS.catch), Slot],
        binder: None,
        lazy_slots: &[(1, CODE)],
        reserved: false,
    },
    // USING <module> SCOPE <body>.
    Form {
        id: FormId::UsingScope,
        key: &[Kw(&KEYWORDS.using), Slot, Kw(&KEYWORDS.scope), Slot],
        binder: None,
        lazy_slots: &[(3, CODE)],
        reserved: false,
    },
    // CLOSE OVER <captures> <body>.
    Form {
        id: FormId::CloseOver,
        key: &[Kw(&KEYWORDS.close), Kw(&KEYWORDS.over), Slot, Slot],
        binder: None,
        lazy_slots: &[(2, CODE), (3, CODE)],
        reserved: false,
    },
    // CLOSE <body> — the inferred-capture form.
    Form {
        id: FormId::Close,
        key: &[Kw(&KEYWORDS.close), Slot],
        binder: None,
        lazy_slots: &[(1, CODE)],
        reserved: false,
    },
    // <field list> FROM <record>.
    Form {
        id: FormId::Projection,
        key: &[Slot, Kw(&KEYWORDS.from), Slot],
        binder: None,
        lazy_slots: &[(0, CODE)],
        reserved: false,
    },
    // ATTR <record> <field> — the parse of `m.x`.
    Form {
        id: FormId::Attribute,
        key: &[Kw(&KEYWORDS.attr), Slot, Slot],
        binder: None,
        lazy_slots: &[],
        reserved: false,
    },
    // EVAL <expr> — the parse of `$(expr)`.
    Form {
        id: FormId::Eval,
        key: &[Kw(&KEYWORDS.eval), Slot],
        binder: None,
        lazy_slots: &[],
        reserved: false,
    },
];

/// A spec key rendered for a failure message: keywords verbatim, slots as `_`.
#[cfg(test)]
pub fn render_key(key: &[KeyElementSpec]) -> Vec<String> {
    key.iter()
        .map(|element| match element {
            KeyElementSpec::Keyword(name) => name.text().to_string(),
            KeyElementSpec::Slot => "_".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
