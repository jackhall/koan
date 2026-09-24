//! The builtin shape table: every fixed expression shape the machine recognizes, spelled once.
//!
//! A builtin shape is recognized by its **full untyped bucket key**, every keyword pinned in
//! position. That recognition is sound because builtin buckets are unshadowable: a node whose key
//! matches a table entry can only ever resolve to that builtin's overloads, and a key the table
//! marks [`reserved`](BuiltinShape::reserved) is refused to user registration for the same reason.
//!
//! [`BUILTIN_SHAPES`] is the one table, and an entry is a **typed** run: keywords in position, and
//! at each slot a [`Role`] beside one [`SlotType`] per overload of the bucket, with one return per
//! overload. The untyped facts are erasures of that run rather than columns of their own — the
//! bucket key a probe compares against is the elements with their types dropped
//! ([`BuiltinShape::matches`]), and the part kinds a slot keeps raw are the raw-capture leaves
//! among its overloads' types ([`BuiltinShape::lazy_kinds_at`]). What no erasure yields rides the
//! entry beside them: the binder it installs and the reserved bit.
//!
//! A node resolves its entry once, at construction ([`NodeCache`]), and every later reader indexes
//! by the [`BuiltinShapeId`] tag rather than re-walking a key: the close-inference rules
//! ([`CLOSE_RULES`](crate::machine::model::close_inference)) and the miss diagnostics
//! ([`MISS_DIAGNOSTICS`](crate::machine::model::miss_diagnostics::MISS_DIAGNOSTICS)) are
//! `(BuiltinShapeId, …)` pairs and hold no key of their own.
//!
//! A slot type rests here as a [`KType`], whose handle is a `const` content digest, so the table
//! states a type without a registry in hand and both erasures run at build time. Interning an
//! entry's overloads as `ExpressionShape` handles is a separate door, in
//! [`elaborate`](crate::elaborate).
//!
//! [`NodeCache`]: crate::parse::ast::NodeCache

pub mod binder;
pub mod layout;
pub mod lazy;
pub mod role;

use crate::parse::ast::KeyElement;
use crate::parse::builtin_shapes::binder::{
    BinderFacts, BinderSurface, fn_def_binder_bucket, identifier_part_binder_name,
    op_def_binder_bucket, type_decl_binder_name, type_part_binder_name,
};
use crate::parse::builtin_shapes::lazy::LazyKinds;
use crate::parse::builtin_shapes::role::{BodyKind, DefinitionKind, Heads, Role};
use crate::symbols::{KeywordSymbol, StaticName};
use crate::type_lattice::KType;

/// The fixed tokens the builtin shapes are spelled with, each declared once and minted once. Every
/// [`BUILTIN_SHAPES`] entry names its keywords out of this group, and the binder module's reserved-symbol
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
    /// A bound, `<Name> UNDER <bound>`: in a `FOR ALL` group and a `TYPE` declarator.
    pub(crate) under: StaticName<KeywordSymbol>,
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
    /// The opaque ascription operator `:|`.
    pub(crate) opaque: StaticName<KeywordSymbol>,
    /// The transparent ascription operator `:!`.
    pub(crate) transparent: StaticName<KeywordSymbol>,
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
    under: crate::static_name!(KeywordSymbol, "UNDER"),
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
    opaque: crate::static_name!(KeywordSymbol, ":|"),
    transparent: crate::static_name!(KeywordSymbol, ":!"),
};

/// A slot's declared type, as it rests in a `static`.
///
/// A leaf is its own `const` handle. A compound's handle is a digest over its members, which no
/// `const` computes, so the two compounds a builtin slot uses rest as the recipe the `elaborate`
/// door interns.
#[derive(Clone, Copy, Debug)]
pub enum SlotType {
    /// A type whose handle is a `const`.
    Leaf(KType),
    /// The canonical union of these leaves.
    Union(&'static [KType]),
    /// The empty record type.
    EmptyRecord,
}

impl SlotType {
    /// The part kinds this slot type captures raw, distributed over a union's members: a
    /// union-typed slot admits every carrier spelling it lists, so it keeps each member's kind raw.
    const fn raw_kinds(self) -> LazyKinds {
        match self {
            SlotType::Leaf(leaf) => raw_kind_of(leaf),
            SlotType::Union(members) => raw_kinds_over(members),
            SlotType::EmptyRecord => LazyKinds::EMPTY,
        }
    }
}

/// The kinds a run of union members keeps raw: every member's own kind, together. A union-typed
/// slot admits each carrier spelling it lists, so each contributes.
const fn raw_kinds_over(members: &[KType]) -> LazyKinds {
    let mut kinds = LazyKinds::EMPTY;
    let mut member = 0;
    while member < members.len() {
        kinds = kinds.with(raw_kind_of(members[member]));
        member += 1;
    }
    kinds
}

/// The kind one raw-capture leaf stands for, empty for every other type: `KExpression` captures an
/// `(…)` group or a `#(…)` quote, `SigiledTypeExpr` a `:(…)`, `RecordType` a `:{…}`.
const fn raw_kind_of(leaf: KType) -> LazyKinds {
    if leaf.same_as(KType::KEXPRESSION) {
        LazyKinds::CODE
    } else if leaf.same_as(KType::SIGILED_TYPE_EXPR) {
        LazyKinds::TYPE_EXPR
    } else if leaf.same_as(KType::RECORD_TYPE) {
        LazyKinds::RECORD_TYPE
    } else {
        LazyKinds::EMPTY
    }
}

/// One position of a builtin bucket: a fixed keyword token, or a slot under a role typed once per
/// overload. [`BUILTIN_SHAPES`] is `static`, so a keyword rests as one of the [`KEYWORDS`] names and
/// matching compares its memoized symbol against the symbol a stored key carries — a table probe is
/// a walk over short runs that hashes nothing past each name's first touch.
pub enum ShapeElement {
    Keyword(&'static StaticName<KeywordSymbol>),
    /// `types[n]` is overload `n`'s type at this position, so a bucket's overloads sit side by side
    /// under one keyword run and cannot disagree about the key they erase to.
    Slot {
        role: Role,
        types: &'static [SlotType],
    },
}

impl ShapeElement {
    /// True iff `element` fills this position: a keyword against the key's own symbol, a slot
    /// against any non-keyword position.
    fn matches(&self, element: KeyElement) -> bool {
        match (self, element) {
            (ShapeElement::Keyword(name), KeyElement::Keyword(symbol)) => name.symbol() == symbol,
            (ShapeElement::Slot { .. }, KeyElement::Slot) => true,
            _ => false,
        }
    }
}

/// One builtin bucket, spelled as the typed shape its overloads share. Every key is spelled here
/// and nowhere else.
pub struct BuiltinShape {
    pub id: BuiltinShapeId,
    /// The whole run — ALL keywords in position, never just the lead keyword — each slot typed once
    /// per overload.
    pub elements: &'static [ShapeElement],
    /// `returns[n]` is overload `n`'s return; its length is the bucket's overload count.
    pub returns: &'static [SlotType],
    /// What the shape installs when submitted as a statement. `None` for a shape that binds nothing.
    pub binder: Option<BinderFacts>,
    /// True when nothing registers under this key: the shape is a diagnosable mistake, and the
    /// overload write door refuses a user registration there.
    pub reserved: bool,
}

impl BuiltinShape {
    /// True iff `key` is this entry's erasure, element for element. The one comparison in the tree:
    /// every table probe feeds it a node's stored key, and the parser's pre-freeze admission feeds
    /// it the key elements its parts run spells.
    pub fn matches(&self, key: impl ExactSizeIterator<Item = KeyElement>) -> bool {
        self.elements.len() == key.len()
            && self
                .elements
                .iter()
                .zip(key)
                .all(|(element, actual)| element.matches(actual))
    }

    /// The kinds that stay raw at `index`, over every overload of the bucket. Empty when the slot
    /// evaluates, when `index` names a keyword, and when it is past the run.
    pub const fn lazy_kinds_at(&self, index: usize) -> LazyKinds {
        if index >= self.elements.len() {
            return LazyKinds::EMPTY;
        }
        let ShapeElement::Slot { types, .. } = &self.elements[index] else {
            return LazyKinds::EMPTY;
        };
        let mut kinds = LazyKinds::EMPTY;
        let mut overload = 0;
        while overload < types.len() {
            kinds = kinds.with(types[overload].raw_kinds());
            overload += 1;
        }
        kinds
    }

    /// Each part's role, element for element with the run and [`Role::Keyword`] at a keyword.
    pub fn roles(&self) -> impl ExactSizeIterator<Item = Role> + '_ {
        self.elements.iter().map(|element| match element {
            ShapeElement::Keyword(_) => Role::Keyword,
            ShapeElement::Slot { role, .. } => *role,
        })
    }

    /// False for a bucket the body-shape builder does not resolve — its slots carry
    /// [`Role::Unsupported`], one and all.
    pub fn supported(&self) -> bool {
        !self.elements.iter().any(|element| {
            matches!(
                element,
                ShapeElement::Slot {
                    role: Role::Unsupported,
                    ..
                }
            )
        })
    }

    /// How many typed overloads this bucket has.
    pub fn overloads(&self) -> usize {
        self.returns.len()
    }
}

/// Every slot of an entry types as many overloads as the entry returns, and every bucket has at
/// least one. The count is what makes a column a column: a slot one type short would leave an
/// overload untyped there, and nothing downstream could say which.
const fn overload_counts_agree(table: &[BuiltinShape]) -> bool {
    let mut entry = 0;
    while entry < table.len() {
        let shape = &table[entry];
        if shape.returns.is_empty() {
            return false;
        }
        let mut index = 0;
        while index < shape.elements.len() {
            if let ShapeElement::Slot { types, .. } = &shape.elements[index]
                && types.len() != shape.returns.len()
            {
                return false;
            }
            index += 1;
        }
        entry += 1;
    }
    true
}

/// The roles whose meaning is a claim about the slot's type. A part the machine reads as code — a
/// body, a branch run, a `FOR ALL` group, a quoted symbol — must reach its reader unevaluated, so
/// every overload types it `KExpression`; a binding's right-hand side is classified where it lands,
/// so it keeps no part raw.
const fn roles_agree_with_raw_kinds(table: &[BuiltinShape]) -> bool {
    let mut entry = 0;
    while entry < table.len() {
        let shape = &table[entry];
        let mut index = 0;
        while index < shape.elements.len() {
            if let ShapeElement::Slot { role, types } = &shape.elements[index] {
                match role {
                    Role::Body(_) | Role::Branches(_) | Role::Quantifiers | Role::Data => {
                        let mut overload = 0;
                        while overload < types.len() {
                            let SlotType::Leaf(leaf) = types[overload] else {
                                return false;
                            };
                            if !leaf.same_as(KType::KEXPRESSION) {
                                return false;
                            }
                            overload += 1;
                        }
                    }
                    Role::Rhs if !shape.lazy_kinds_at(index).is_empty() => return false,
                    _ => {}
                }
            }
            index += 1;
        }
        entry += 1;
    }
    true
}

const _: () = assert!(overload_counts_agree(BUILTIN_SHAPE_SPEC));
const _: () = assert!(roles_agree_with_raw_kinds(BUILTIN_SHAPE_SPEC));

/// One variant per [`BUILTIN_SHAPES`] entry, in table order — the tag every other table keys by, so a rule
/// or a diagnostic names a shape without respelling its key. The table-order property pins each tag
/// to its index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltinShapeId {
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
    QuantifiedLambda,
    QuantifiedLambdaType,
    CombinedQuantifiedLambda,
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
    AscribeOpaque,
    AscribeTransparent,
    CloseOver,
    Close,
    Projection,
    Attribute,
    Eval,
}

/// The [`BUILTIN_SHAPES`] entry `key` matches, or `None` for every user-defined bucket. The one
/// table probe: a node resolves its entry here at construction and caches it, and every later
/// reader goes through the cache.
pub fn builtin_shape_for(
    key: impl ExactSizeIterator<Item = KeyElement> + Clone,
) -> Option<&'static BuiltinShape> {
    BUILTIN_SHAPES
        .iter()
        .find(|shape| shape.matches(key.clone()))
}

use ShapeElement::Keyword as Kw;

/// A slot under `role`, typed once per overload of its bucket.
const fn slot(role: Role, types: &'static [SlotType]) -> ShapeElement {
    ShapeElement::Slot { role, types }
}

use Role::{
    Argument, Body, Branches, Data, Definition, Label, Name, Quantifiers, Rhs, Signature,
    TypeExpression as Te, Unsupported,
};

// The slot types the table spells, each a `const` handle or a recipe over them. Their names are the
// lattice's own, so an entry reads as the types its overloads declare.
const ANY: SlotType = SlotType::Leaf(KType::ANY);
const NEVER: SlotType = SlotType::Leaf(KType::NEVER);
const CODE: SlotType = SlotType::Leaf(KType::KEXPRESSION);
const IDENTIFIER: SlotType = SlotType::Leaf(KType::IDENTIFIER);
const NAME_TOKEN: SlotType = SlotType::Leaf(KType::NAME_TOKEN);
const TYPE_NAME: SlotType = SlotType::Leaf(KType::TYPE_NAME_TOKEN);
const SIGILED_TYPE: SlotType = SlotType::Leaf(KType::SIGILED_TYPE_EXPR);
const RECORD_TYPE: SlotType = SlotType::Leaf(KType::RECORD_TYPE);
const STR: SlotType = SlotType::Leaf(KType::STR);
const PROPER_TYPE: SlotType = SlotType::Leaf(KType::PROPER_TYPE);
const SIGNATURE_KIND: SlotType = SlotType::Leaf(KType::SIGNATURE_KIND);
const ANY_TYPE: SlotType = SlotType::Leaf(KType::ANY_TYPE);
/// The empty signature: the type a module value is declared at, `:Module`'s own handle.
const MODULE: SlotType = SlotType::Leaf(KType::EMPTY_SIGNATURE);
/// What a declarator's type slot takes: a bare Type name, a `:(…)` type expression or a `:{…}`
/// record type. The `:(…)` and `:{…}` spellings are what make such a slot keep its part raw; a bare
/// `(…)` there evaluates.
const TYPE_CARRIER: SlotType = SlotType::Union(&[
    KType::TYPE_NAME_TOKEN,
    KType::SIGILED_TYPE_EXPR,
    KType::RECORD_TYPE,
]);
const EMPTY_RECORD: SlotType = SlotType::EmptyRecord;

/// The single source of truth for the builtin shapes. One entry per distinct bucket key, in
/// [`BuiltinShapeId`] order; the keys are pinned against the live builtin registration table by the
/// table⟺registration property, so an entry whose builtin was renamed, re-shaped, or dropped fails
/// the suite, and so does a builtin that grows a raw-capture slot without an entry here.
///
/// The table is spelled as a `const` and read through the `static` below because a `const` is what
/// the two build-time laws above can be evaluated over — a `const` cannot read a `static`. Readers
/// take the `static`, so every `&'static BuiltinShape` a node caches names one address.
const BUILTIN_SHAPE_SPEC: &[BuiltinShape] = &[
    // ---------- the name binders ----------
    //
    // LET <name> = <value>: value-name overload then type-alias overload.
    BuiltinShape {
        id: BuiltinShapeId::LetValue,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Name, &[NAME_TOKEN]),
            Kw(&KEYWORDS.equals),
            slot(Rhs, &[ANY]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name, type_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // TYPE <name> — SIG-body-only abstract-type declarator (bare and higher-kinded share the key).
    BuiltinShape {
        id: BuiltinShapeId::TypeDeclaration,
        elements: &[Kw(&KEYWORDS.type_), slot(Name, &[TYPE_NAME, CODE])],
        returns: &[ANY, ANY],
        binder: Some(BinderFacts {
            names: &[type_decl_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // MODULE <name> = <body> (a module is a value, so the name slot is an `Identifier`; a
    // Type-token name registers nothing and takes the miss table's respelling diagnostic).
    BuiltinShape {
        id: BuiltinShapeId::Module,
        elements: &[
            Kw(&KEYWORDS.module),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Module), &[CODE]),
        ],
        returns: &[MODULE],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // GROUP <name> FOLD LEFT = <body>.
    BuiltinShape {
        id: BuiltinShapeId::GroupFoldLeft,
        elements: &[
            Kw(&KEYWORDS.group),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Module), &[CODE]),
        ],
        returns: &[MODULE],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // GROUP <name> FOLD RIGHT = <body>.
    BuiltinShape {
        id: BuiltinShapeId::GroupFoldRight,
        elements: &[
            Kw(&KEYWORDS.group),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Module), &[CODE]),
        ],
        returns: &[MODULE],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // GROUP <name> PAIRWISE FOLD <combiner> LEFT = <body>.
    BuiltinShape {
        id: BuiltinShapeId::GroupPairwiseFoldLeft,
        elements: &[
            Kw(&KEYWORDS.group),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            slot(Argument, &[CODE]),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Module), &[CODE]),
        ],
        returns: &[MODULE],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // GROUP <name> PAIRWISE FOLD <combiner> RIGHT = <body>.
    BuiltinShape {
        id: BuiltinShapeId::GroupPairwiseFoldRight,
        elements: &[
            Kw(&KEYWORDS.group),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            slot(Argument, &[CODE]),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Module), &[CODE]),
        ],
        returns: &[MODULE],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // SIG <name> = <body>.
    BuiltinShape {
        id: BuiltinShapeId::Sig,
        elements: &[
            Kw(&KEYWORDS.sig),
            slot(Name, &[TYPE_NAME]),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Plain), &[CODE]),
        ],
        returns: &[SIGNATURE_KIND],
        binder: Some(BinderFacts {
            names: &[type_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // UNION <name> = <schema>.
    BuiltinShape {
        id: BuiltinShapeId::Union,
        elements: &[
            Kw(&KEYWORDS.union),
            slot(Name, &[TYPE_NAME]),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Union), &[CODE]),
        ],
        returns: &[ANY_TYPE],
        binder: Some(BinderFacts {
            names: &[type_part_binder_name],
            bucket: None,
            surface: BinderSurface::UnionDef,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // NEWTYPE <name> = <repr> (scalar / sigil / record reprs share the key). The repr slot takes a
    // type but is deliberately unmasked: a bare `(…)` there already works by evaluation, so
    // flipping it would change a working spelling's route for nothing. A `:(…)` or `:{…}` there is
    // captured raw while a bare `(…)` evaluates — the mixed index the kind set exists for.
    BuiltinShape {
        id: BuiltinShapeId::NewTypeDefinition,
        elements: &[
            Kw(&KEYWORDS.newtype),
            slot(Name, &[TYPE_NAME, TYPE_NAME, TYPE_NAME]),
            Kw(&KEYWORDS.equals),
            slot(
                Definition(DefinitionKind::Plain),
                &[PROPER_TYPE, SIGILED_TYPE, RECORD_TYPE],
            ),
        ],
        returns: &[ANY_TYPE, ANY_TYPE, ANY_TYPE],
        binder: Some(BinderFacts {
            names: &[type_part_binder_name],
            bucket: None,
            surface: BinderSurface::NewTypeDef,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // NEWTYPE <decl> — constructor family (keyword set {NEWTYPE}, disjoint from the `= _` shapes).
    BuiltinShape {
        id: BuiltinShapeId::NewTypeDeclaration,
        elements: &[Kw(&KEYWORDS.newtype), slot(Name, &[CODE])],
        returns: &[ANY_TYPE],
        binder: Some(BinderFacts {
            names: &[type_decl_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // VAL <name> <ty> — a declarator with no install channel. It records into the decl scope's
    // slot collector, not a binding map any name lookup can see, so it installs nothing; it appears
    // here so the one-place specification of the declarators is complete.
    BuiltinShape {
        id: BuiltinShapeId::Val,
        elements: &[
            Kw(&KEYWORDS.val),
            slot(Label, &[IDENTIFIER]),
            slot(Te, &[PROPER_TYPE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // ---------- the function and expression declarators ----------
    //
    // FN <record schema> -> <return type> = <body> — the lambda. It binds nothing: it has no name
    // and no head to key a bucket on. Its binder facts are here for the type slot alone, so a bare
    // `(…)` return spelling rewrites to a sigiled type expression as it does on every other shape.
    // The signature slot resolves: a `:{…}` record is a type the lane can evaluate where it stands,
    // unlike a head, whose tokens name nothing until the definition binds them.
    BuiltinShape {
        id: BuiltinShapeId::Lambda,
        elements: &[
            Kw(&KEYWORDS.fn_),
            slot(Signature, &[PROPER_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: None,
            type_slots: &[3],
        }),
        reserved: false,
    },
    // FN <record schema> -> <return type> — the lambda type expression, no body.
    BuiltinShape {
        id: BuiltinShapeId::LambdaType,
        elements: &[
            Kw(&KEYWORDS.fn_),
            slot(Signature, &[PROPER_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
        ],
        returns: &[ANY_TYPE],
        binder: None,
        reserved: false,
    },
    // LET <name> = FN <signature> -> <return type> = <body> — reserved: a combined statement
    // installs a dispatch bucket, and no `FN` signature has a head to key one on, so nothing
    // registers here. The lazy stamp is what keeps its miss a miss, holding the body raw so the
    // statement reports the shape it got rather than an error from evaluating a body whose
    // parameters no binder ever bound.
    BuiltinShape {
        id: BuiltinShapeId::CombinedLambda,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Unsupported, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            slot(Unsupported, &[CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Unsupported, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Unsupported, &[CODE]),
        ],
        returns: &[NEVER],
        binder: None,
        reserved: true,
    },
    // FN FOR ALL <names> <record schema> -> <return type> = <body> — the quantified lambda.
    //
    // The schema captures raw where the unquantified `Lambda`'s resolves: its field types name the
    // group's quantifiers, so it must reach its reader unevaluated — the reason the names group
    // and the return stage on every quantified entry.
    BuiltinShape {
        id: BuiltinShapeId::QuantifiedLambda,
        elements: &[
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Quantifiers, &[CODE]),
            slot(Signature, &[RECORD_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: None,
            type_slots: &[6],
        }),
        reserved: false,
    },
    // FN FOR ALL <names> <record schema> -> <return type> — the quantified lambda type expression,
    // no body.
    BuiltinShape {
        id: BuiltinShapeId::QuantifiedLambdaType,
        elements: &[
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Quantifiers, &[CODE]),
            slot(Signature, &[RECORD_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
        ],
        returns: &[ANY_TYPE],
        binder: None,
        reserved: false,
    },
    // LET <name> = FN FOR ALL <names> <signature> -> <return type> = <body> — reserved for the
    // same reason `CombinedLambda` is: a combined statement installs a dispatch bucket, and no
    // `FN` signature has a head to key one on. The lazy stamp holds the body raw so the miss is
    // diagnosable.
    BuiltinShape {
        id: BuiltinShapeId::CombinedQuantifiedLambda,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Unsupported, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Unsupported, &[CODE]),
            slot(Unsupported, &[RECORD_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Unsupported, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Unsupported, &[CODE]),
        ],
        returns: &[NEVER],
        binder: None,
        reserved: true,
    },
    // EXPR <head> -> <return type> = <body> (every EXPR definition overload shares this key).
    BuiltinShape {
        id: BuiltinShapeId::ExpressionDefinition,
        elements: &[
            Kw(&KEYWORDS.expr),
            slot(Signature, &[CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(fn_def_binder_bucket),
            surface: BinderSurface::Other,
            name_slot: None,
            type_slots: &[3],
        }),
        reserved: false,
    },
    // EXPR <head> -> <return type> — the bodyless head, whose carrier is the head's expression
    // shape. Only the head captures raw: it is the declarator's own operand and must reach it
    // unevaluated, while the return slot is an ordinary kind expectation, as on every bodyless head.
    BuiltinShape {
        id: BuiltinShapeId::ExpressionHead,
        elements: &[
            Kw(&KEYWORDS.expr),
            slot(Signature, &[CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // EXPR FOR ALL <names> <head> -> <return type> = <body>.
    BuiltinShape {
        id: BuiltinShapeId::QuantifiedExpressionDefinition,
        elements: &[
            Kw(&KEYWORDS.expr),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Quantifiers, &[CODE]),
            slot(Signature, &[CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(fn_def_binder_bucket),
            surface: BinderSurface::Other,
            name_slot: None,
            type_slots: &[6],
        }),
        reserved: false,
    },
    // EXPR FOR ALL <names> <head> -> <return type> — the quantified bodyless head. The names group
    // captures raw beside the head: its tokens name nothing yet, so the lane must not try to
    // resolve them. The return stages too, because it may name a quantifier and so resolves against
    // the group's own scope rather than the surrounding one.
    BuiltinShape {
        id: BuiltinShapeId::QuantifiedExpressionHead,
        elements: &[
            Kw(&KEYWORDS.expr),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Quantifiers, &[CODE]),
            slot(Signature, &[CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // LET <name> = FN EXPR <head> -> <return type> = <body> — a combined statement, one binder
    // filling both channels: the LET value name and the bucket key the declaration's body
    // registers under.
    BuiltinShape {
        id: BuiltinShapeId::CombinedExpression,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.expr),
            slot(Signature, &[CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(fn_def_binder_bucket),
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[7],
        }),
        reserved: false,
    },
    // LET <name> = FN EXPR FOR ALL <names> <head> -> <return type> = <body>.
    BuiltinShape {
        id: BuiltinShapeId::CombinedQuantifiedExpression,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.expr),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Quantifiers, &[CODE]),
            slot(Signature, &[CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(fn_def_binder_bucket),
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[10],
        }),
        reserved: false,
    },
    // ---------- the operator declarators ----------
    //
    // OP <symbol> OVER <operand> = <body>.
    BuiltinShape {
        id: BuiltinShapeId::OperatorDefinition,
        elements: &[
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Operator), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[3],
        }),
        reserved: false,
    },
    // OP <symbol> OVER <operand> -> <return type> = <body>.
    BuiltinShape {
        id: BuiltinShapeId::OperatorDefinitionReturning,
        elements: &[
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Operator), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[3, 5],
        }),
        reserved: false,
    },
    // UNARY OP <symbol> OVER <operand> = <body> — reserved: the result segment is mandatory, so
    // this shape's only reading is the mistake the miss table names.
    BuiltinShape {
        id: BuiltinShapeId::UnaryOperatorDefinition,
        elements: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            slot(Unsupported, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Unsupported, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Unsupported, &[CODE]),
        ],
        returns: &[NEVER],
        binder: None,
        reserved: true,
    },
    // UNARY OP <symbol> OVER <operand> -> <return type> = <body>.
    BuiltinShape {
        id: BuiltinShapeId::UnaryOperatorDefinitionReturning,
        elements: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::UnaryOperator), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[4, 6],
        }),
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
    BuiltinShape {
        id: BuiltinShapeId::OperatorHead,
        elements: &[
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[3],
        }),
        reserved: false,
    },
    // OP <symbol> OVER <operand> -> <result>.
    BuiltinShape {
        id: BuiltinShapeId::OperatorHeadReturning,
        elements: &[
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[3, 5],
        }),
        reserved: false,
    },
    // UNARY OP <symbol> OVER <operand> — the head shape, missing its result. Reserved for the same
    // reason the definition is: the run has no other reading, and a user registration claiming the
    // key would turn the pointed message into a typed miss under its own bucket.
    BuiltinShape {
        id: BuiltinShapeId::UnaryOperatorHead,
        elements: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            slot(Unsupported, &[ANY]),
            Kw(&KEYWORDS.over),
            slot(Unsupported, &[ANY]),
        ],
        returns: &[NEVER],
        binder: None,
        reserved: true,
    },
    // UNARY OP <symbol> OVER <operand> -> <result>.
    BuiltinShape {
        id: BuiltinShapeId::UnaryOperatorHeadReturning,
        elements: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[],
            bucket: None,
            surface: BinderSurface::OperatorDef,
            name_slot: None,
            type_slots: &[4, 6],
        }),
        reserved: false,
    },
    // LET <name> = OP <symbol> OVER <operand> = <body>.
    BuiltinShape {
        id: BuiltinShapeId::CombinedOperator,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Operator), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: Some(1),
            type_slots: &[6],
        }),
        reserved: false,
    },
    // LET <name> = OP <symbol> OVER <operand> -> <return type> = <body>.
    BuiltinShape {
        id: BuiltinShapeId::CombinedOperatorReturning,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Operator), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: Some(1),
            type_slots: &[6, 8],
        }),
        reserved: false,
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> = <body> — reserved, the combined twin of the
    // missing-result mistake.
    BuiltinShape {
        id: BuiltinShapeId::CombinedUnaryOperator,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Unsupported, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            slot(Unsupported, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Unsupported, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Unsupported, &[CODE]),
        ],
        returns: &[NEVER],
        binder: None,
        reserved: true,
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> -> <return type> = <body> — the two-bucket
    // maximum: a value name and both keys a `UNARY OP` body registers under.
    BuiltinShape {
        id: BuiltinShapeId::CombinedUnaryOperatorReturning,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            slot(Data, &[CODE]),
            Kw(&KEYWORDS.over),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[TYPE_CARRIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::UnaryOperator), &[CODE]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: Some(op_def_binder_bucket),
            surface: BinderSurface::OperatorDef,
            name_slot: Some(1),
            type_slots: &[7, 9],
        }),
        reserved: false,
    },
    // ---------- the SIG-body group heads: the definition spellings minus the name slot ----------
    //
    // GROUP FOLD LEFT = <heads>.
    BuiltinShape {
        id: BuiltinShapeId::GroupHeadFoldLeft,
        elements: &[
            Kw(&KEYWORDS.group),
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Plain), &[CODE]),
        ],
        returns: &[MODULE],
        binder: None,
        reserved: false,
    },
    // GROUP FOLD RIGHT = <heads>.
    BuiltinShape {
        id: BuiltinShapeId::GroupHeadFoldRight,
        elements: &[
            Kw(&KEYWORDS.group),
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Plain), &[CODE]),
        ],
        returns: &[MODULE],
        binder: None,
        reserved: false,
    },
    // GROUP PAIRWISE FOLD <combiner> LEFT = <heads>.
    BuiltinShape {
        id: BuiltinShapeId::GroupHeadPairwiseFoldLeft,
        elements: &[
            Kw(&KEYWORDS.group),
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            slot(Argument, &[CODE]),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Plain), &[CODE]),
        ],
        returns: &[MODULE],
        binder: None,
        reserved: false,
    },
    // GROUP PAIRWISE FOLD <combiner> RIGHT = <heads>.
    BuiltinShape {
        id: BuiltinShapeId::GroupHeadPairwiseFoldRight,
        elements: &[
            Kw(&KEYWORDS.group),
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            slot(Argument, &[CODE]),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Plain), &[CODE]),
        ],
        returns: &[MODULE],
        binder: None,
        reserved: false,
    },
    // ---------- the control shapes ----------
    //
    // MATCH <scrutinee> -> <result type> WITH <branches>.
    BuiltinShape {
        id: BuiltinShapeId::Match,
        elements: &[
            Kw(&KEYWORDS.match_),
            slot(Argument, &[ANY]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[PROPER_TYPE]),
            Kw(&KEYWORDS.with),
            slot(Branches(Heads::Types), &[CODE]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // MATCH <scrutinee> OVER <union> -> <result type> WITH <branches>.
    BuiltinShape {
        id: BuiltinShapeId::MatchOver,
        elements: &[
            Kw(&KEYWORDS.match_),
            slot(Argument, &[ANY]),
            Kw(&KEYWORDS.over),
            slot(Te, &[PROPER_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[PROPER_TYPE]),
            Kw(&KEYWORDS.with),
            slot(Branches(Heads::Labels), &[CODE]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // TRY <body> -> <result type> WITH <branches>.
    BuiltinShape {
        id: BuiltinShapeId::Try,
        elements: &[
            Kw(&KEYWORDS.try_),
            slot(Argument, &[CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[PROPER_TYPE]),
            Kw(&KEYWORDS.with),
            slot(Branches(Heads::Labels), &[CODE]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // CATCH <body>.
    BuiltinShape {
        id: BuiltinShapeId::Catch,
        elements: &[Kw(&KEYWORDS.catch), slot(Argument, &[CODE])],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // USING <module> SCOPE <body> — the body is a block whose parameters are the names the
    // operand surfaces, read where the shape is built.
    BuiltinShape {
        id: BuiltinShapeId::UsingScope,
        elements: &[
            Kw(&KEYWORDS.using),
            slot(Argument, &[MODULE]),
            Kw(&KEYWORDS.scope),
            slot(Body(BodyKind::Surfaced), &[CODE]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // <module> :| <Sig> — the opaque ascription: a view whose abstract members are minted afresh.
    BuiltinShape {
        id: BuiltinShapeId::AscribeOpaque,
        elements: &[
            slot(Argument, &[MODULE]),
            Kw(&KEYWORDS.opaque),
            slot(Te, &[SIGNATURE_KIND]),
        ],
        returns: &[MODULE],
        binder: None,
        reserved: false,
    },
    // <module> :! <Sig> — the transparent ascription: a view at the source's own bindings.
    BuiltinShape {
        id: BuiltinShapeId::AscribeTransparent,
        elements: &[
            slot(Argument, &[MODULE]),
            Kw(&KEYWORDS.transparent),
            slot(Te, &[SIGNATURE_KIND]),
        ],
        returns: &[MODULE],
        binder: None,
        reserved: false,
    },
    // CLOSE OVER <captures> <body>.
    BuiltinShape {
        id: BuiltinShapeId::CloseOver,
        elements: &[
            Kw(&KEYWORDS.close),
            Kw(&KEYWORDS.over),
            slot(Unsupported, &[CODE]),
            slot(Unsupported, &[CODE]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // CLOSE <body> — the inferred-capture form.
    BuiltinShape {
        id: BuiltinShapeId::Close,
        elements: &[Kw(&KEYWORDS.close), slot(Unsupported, &[CODE])],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // <field list> FROM <record>.
    BuiltinShape {
        id: BuiltinShapeId::Projection,
        elements: &[
            slot(Label, &[CODE]),
            Kw(&KEYWORDS.from),
            slot(Argument, &[EMPTY_RECORD]),
        ],
        returns: &[EMPTY_RECORD],
        binder: None,
        reserved: false,
    },
    // ATTR <record> <field> — the parse of `m.x`.
    BuiltinShape {
        id: BuiltinShapeId::Attribute,
        elements: &[
            Kw(&KEYWORDS.attr),
            slot(Argument, &[IDENTIFIER, MODULE, ANY, ANY, ANY_TYPE, MODULE]),
            slot(
                Label,
                &[NAME_TOKEN, NAME_TOKEN, NAME_TOKEN, STR, NAME_TOKEN, STR],
            ),
        ],
        returns: &[ANY, ANY, ANY, ANY, ANY, ANY],
        binder: None,
        reserved: false,
    },
    // EVAL <expr> — the parse of `$(expr)`.
    BuiltinShape {
        id: BuiltinShapeId::Eval,
        elements: &[Kw(&KEYWORDS.eval), slot(Argument, &[ANY])],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
];

pub static BUILTIN_SHAPES: &[BuiltinShape] = BUILTIN_SHAPE_SPEC;

/// An entry's run rendered for a failure message: keywords verbatim, slots as `_`.
#[cfg(test)]
pub fn render_key(elements: &[ShapeElement]) -> Vec<String> {
    elements
        .iter()
        .map(|element| match element {
            ShapeElement::Keyword(name) => name.text().to_string(),
            ShapeElement::Slot { .. } => "_".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
