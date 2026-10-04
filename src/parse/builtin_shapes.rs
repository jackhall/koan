//! The builtin shape table: every fixed expression shape the interpreter recognizes, spelled once.
//!
//! A builtin shape is recognized by its **full untyped bucket key**, every keyword pinned in
//! position. That recognition is sound because builtin buckets are unshadowable: a node whose key
//! matches a table entry can only ever resolve to that builtin's overloads, and a key the table
//! marks [`reserved`](BuiltinShape::reserved) is refused to user registration for the same reason.
//!
//! [`BUILTIN_SHAPES`] is the one table, and an entry is a **typed** run: keywords in position, and
//! at each slot a [`Role`] beside one [`KType`] per overload of the bucket, with one return per
//! overload. The bucket key a probe compares against is an erasure of that run — the elements with
//! their types dropped ([`BuiltinShape::matches`]). A slot's role says how the shape builder reads
//! its part ([`Role::reading`]): as a written quote, as bare syntax, as a container of quotes, or
//! evaluated. Its type says what fills it — a code kind for a part read as written, a value type
//! for one evaluated — and no slot keeps a part raw. What no erasure yields rides the entry beside
//! them: the binder it installs and the reserved bit.
//!
//! A node resolves its entry once, at construction ([`NodeCache`]), and every later reader indexes
//! by the [`BuiltinShapeId`] tag rather than re-walking a key.
//!
//! A slot type rests here as a [`KType`], whose handle is a `const` content digest, so the table
//! states a type without a registry in hand and its laws run at build time. Interning an
//! entry's overloads as `ExpressionShape` handles is a separate door, in
//! [`elaborate`](crate::elaborate).
//!
//! [`NodeCache`]: crate::parse::ast::NodeCache

pub mod binder;
pub mod layout;
pub mod role;

use crate::parse::ast::KeyElement;
use crate::parse::builtin_shapes::binder::{
    BinderFacts, BinderSurface, fn_def_binder_bucket, identifier_part_binder_name,
    op_def_binder_bucket, type_decl_binder_name, type_part_binder_name,
};
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
    /// The two tokens of a quantifier group's head, `EXPR FOR ALL #[<names>] …`.
    pub(crate) for_: StaticName<KeywordSymbol>,
    pub(crate) all: StaticName<KeywordSymbol>,
    /// A bound, `<Name> UNDER <bound>`: in a `FOR ALL` group.
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
    /// The head that runs code.
    pub(crate) eval: StaticName<KeywordSymbol>,
    /// The head of the parse of `<record>.<field>`.
    pub(crate) attr: StaticName<KeywordSymbol>,
    /// The opaque ascription operator `:|`.
    pub(crate) opaque: StaticName<KeywordSymbol>,
    /// The transparent ascription operator `:!`.
    pub(crate) transparent: StaticName<KeywordSymbol>,
    /// The connector of a code kind and the names its code needs, `Expression NEEDING #[y]`.
    pub(crate) needing: StaticName<KeywordSymbol>,
    /// `!=`, which the operator-run rewrite turns into `NOT (a == b)`; the parse's depth count
    /// reads it to count the nesting that rewrite builds.
    pub(crate) unequal: StaticName<KeywordSymbol>,
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
    needing: crate::static_name!(KeywordSymbol, "NEEDING"),
    unequal: crate::static_name!(KeywordSymbol, "!="),
};

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
        types: &'static [KType],
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
    pub returns: &'static [KType],
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

    /// True for a bucket whose every slot is an operand dispatch evaluates or a label it reads —
    /// `ATTR`, `FROM` and `USING` over code. A use of one selects among the builtin
    /// overloads at its key, which the bucket being closed keeps the only ones.
    pub fn dispatched(&self) -> bool {
        self.roles()
            .all(|role| matches!(role, Role::Keyword | Role::Argument | Role::Field))
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

/// Whether every overload types a slot as `want`.
const fn every_overload_is(types: &[KType], want: KType) -> bool {
    let mut overload = 0;
    while overload < types.len() {
        if !types[overload].same_as(want) {
            return false;
        }
        overload += 1;
    }
    true
}

/// Whether every overload types a slot as a code kind at or under `kind`.
const fn every_overload_within(types: &[KType], kind: KType) -> bool {
    let mut overload = 0;
    while overload < types.len() {
        if !types[overload].within_code(kind) {
            return false;
        }
        overload += 1;
    }
    true
}

/// The roles whose reading fixes the syntax that fills them, and so the slot's type: every body is
/// a `Block`, an `EXPR` head an `Expression`, a quoted symbol a `Keyword`, a field label a code
/// kind no larger than a `Name`, an arm set a `Dict(TypeCode, Block)` under type guards and a
/// `Dict(Name, Block)` under labels, a union's variants a `Dict(Name, TypeCode)`, a signature's
/// members a `List(Declaration)`, a representation `TypeCode`, a `FOR ALL` group a list of names or
/// a dict of names to bounds, and a binder name a code kind no larger than an expression. A
/// binding's right-hand side is classified where it lands, so it is `Any`.
const fn roles_agree_with_code_types(table: &[BuiltinShape]) -> bool {
    let mut entry = 0;
    while entry < table.len() {
        let shape = &table[entry];
        let mut index = 0;
        while index < shape.elements.len() {
            if let ShapeElement::Slot { role, types } = &shape.elements[index] {
                let agrees = match role {
                    Role::Body(_) => every_overload_is(types, KType::BLOCK),
                    Role::Head => every_overload_is(types, KType::EXPRESSION),
                    Role::Data => every_overload_is(types, KType::KEYWORD),
                    Role::Field => every_overload_within(types, KType::NAME),
                    Role::Branches(Heads::Types) => {
                        every_overload_is(types, KType::DICT_TYPE_CODE_BLOCK)
                    }
                    Role::Branches(Heads::Labels) => {
                        every_overload_is(types, KType::DICT_NAME_BLOCK)
                    }
                    Role::Definition(DefinitionKind::Union) => {
                        every_overload_is(types, KType::DICT_NAME_TYPE_CODE)
                    }
                    Role::Definition(DefinitionKind::Members) => {
                        every_overload_is(types, KType::LIST_OF_DECLARATION)
                    }
                    Role::Definition(DefinitionKind::Plain) => {
                        every_overload_is(types, KType::TYPE_CODE)
                    }
                    Role::Quantifiers => every_overload_is(types, KType::QUANTIFIER_CODE),
                    Role::Name => every_overload_within(types, KType::EXPRESSION),
                    Role::Rhs => every_overload_is(types, KType::ANY),
                    _ => true,
                };
                if !agrees {
                    return false;
                }
            }
            index += 1;
        }
        entry += 1;
    }
    true
}

const _: () = assert!(overload_counts_agree(BUILTIN_SHAPE_SPEC));
const _: () = assert!(roles_agree_with_code_types(BUILTIN_SHAPE_SPEC));

/// One variant per [`BUILTIN_SHAPES`] entry, in table order — the tag every other table keys by, so a rule
/// or a diagnostic names a shape without respelling its key. The table-order property pins each tag
/// to its index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltinShapeId {
    LetValue,
    LetAnnotated,
    TypeDeclaration,
    Module,
    GroupFoldLeft,
    GroupFoldRight,
    GroupPairwiseFoldLeft,
    GroupPairwiseFoldRight,
    Sig,
    QuantifiedSig,
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
    BucketDeclaration,
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
    UsingCode,
}

impl BuiltinShapeId {
    /// True for a shape that declares a signature member without installing it where it is
    /// written: a `VAL` and every bodyless head, and the reserved `TYPE` declarator, which reaches
    /// the shape builder's refusal as a member. A statement of one of these shapes is a
    /// `Declaration`, and so, beside the binders, is every member a `SIG` body holds.
    pub const fn declares_member(self) -> bool {
        matches!(
            self,
            BuiltinShapeId::Val
                | BuiltinShapeId::TypeDeclaration
                | BuiltinShapeId::ExpressionHead
                | BuiltinShapeId::QuantifiedExpressionHead
                | BuiltinShapeId::OperatorHead
                | BuiltinShapeId::OperatorHeadReturning
                | BuiltinShapeId::UnaryOperatorHead
                | BuiltinShapeId::UnaryOperatorHeadReturning
                | BuiltinShapeId::GroupHeadFoldLeft
                | BuiltinShapeId::GroupHeadFoldRight
                | BuiltinShapeId::GroupHeadPairwiseFoldLeft
                | BuiltinShapeId::GroupHeadPairwiseFoldRight
        )
    }
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
const fn slot(role: Role, types: &'static [KType]) -> ShapeElement {
    ShapeElement::Slot { role, types }
}

use Role::{
    Argument, Body, Branches, Data, Definition, Field, Head, InPlace, Name, Quantifiers, Rhs,
    Signature, TypeExpression as Te, Unsupported,
};

// The slot types the table spells, each a `const` handle named as the lattice names it, so an
// entry reads as the types its overloads declare.
const ANY: KType = KType::ANY;
const NEVER: KType = KType::NEVER;
const IDENTIFIER: KType = KType::IDENTIFIER;
const TYPE_NAME: KType = KType::TYPE_NAME_TOKEN;
const NAME: KType = KType::NAME;
const KEYWORD: KType = KType::KEYWORD;
const EXPRESSION: KType = KType::EXPRESSION;
const BLOCK: KType = KType::BLOCK;
const ANY_CODE: KType = KType::ANY_CODE;
/// The code a type is written as: a type name, a `:(…)` or a `:{…}`.
const TYPE_CODE: KType = KType::TYPE_CODE;
const LIST_OF_NAME: KType = KType::LIST_OF_NAME;
const LIST_OF_DECLARATION: KType = KType::LIST_OF_DECLARATION;
const DICT_NAME_BLOCK: KType = KType::DICT_NAME_BLOCK;
const DICT_TYPE_CODE_BLOCK: KType = KType::DICT_TYPE_CODE_BLOCK;
const DICT_NAME_TYPE_CODE: KType = KType::DICT_NAME_TYPE_CODE;
/// A `FOR ALL` group: a list of names, or a dict of names to the code of their bounds.
const QUANTIFIER_CODE: KType = KType::QUANTIFIER_CODE;
const PROPER_TYPE: KType = KType::PROPER_TYPE;
const SIGNATURE_KIND: KType = KType::SIGNATURE_KIND;
const ANY_TYPE: KType = KType::ANY_TYPE;
/// The empty signature: the type a module value is declared at, `:Module`'s own handle.
const MODULE: KType = KType::EMPTY_SIGNATURE;
const EMPTY_RECORD: KType = KType::EMPTY_RECORD;

/// The single source of truth for the builtin shapes. One entry per distinct bucket key, in
/// [`BuiltinShapeId`] order.
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
            slot(Name, &[NAME]),
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
    // LET <name> <type> = <value> — a value binder bound at a stated type: the value is held to
    // the type as `:!` holds its operand.
    BuiltinShape {
        id: BuiltinShapeId::LetAnnotated,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Name, &[IDENTIFIER]),
            slot(Te, &[PROPER_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Rhs, &[ANY]),
        ],
        returns: &[ANY],
        binder: Some(BinderFacts {
            names: &[identifier_part_binder_name],
            bucket: None,
            surface: BinderSurface::Other,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // TYPE <name> — reserved: a signature hides a type through a head parameter
    // (`SIG <name> FOR ALL <names> = …`), so the shape builder refuses this where it is written.
    BuiltinShape {
        id: BuiltinShapeId::TypeDeclaration,
        elements: &[
            Kw(&KEYWORDS.type_),
            slot(Unsupported, &[TYPE_NAME, EXPRESSION]),
        ],
        returns: &[NEVER, NEVER],
        binder: None,
        reserved: true,
    },
    // MODULE <name> = <body> (a module is a value, so the name slot is an `Identifier`; a
    // Type-token name registers nothing and takes the miss table's respelling diagnostic).
    BuiltinShape {
        id: BuiltinShapeId::Module,
        elements: &[
            Kw(&KEYWORDS.module),
            slot(Name, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Module), &[BLOCK]),
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
            slot(Body(BodyKind::Module), &[BLOCK]),
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
            slot(Body(BodyKind::Module), &[BLOCK]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Module), &[BLOCK]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Module), &[BLOCK]),
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
            slot(Definition(DefinitionKind::Members), &[LIST_OF_DECLARATION]),
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
    // SIG <name> FOR ALL <names> = <body> — a signature over head parameters its members read.
    BuiltinShape {
        id: BuiltinShapeId::QuantifiedSig,
        elements: &[
            Kw(&KEYWORDS.sig),
            slot(Name, &[TYPE_NAME]),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Quantifiers, &[QUANTIFIER_CODE]),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Members), &[LIST_OF_DECLARATION]),
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
    // UNION <name> = <schema> and UNION (<P>… AS <Name>) = <schema> (the parameterized union shares
    // the key).
    BuiltinShape {
        id: BuiltinShapeId::Union,
        elements: &[
            Kw(&KEYWORDS.union),
            slot(Name, &[TYPE_NAME, EXPRESSION]),
            Kw(&KEYWORDS.equals),
            slot(
                Definition(DefinitionKind::Union),
                &[DICT_NAME_TYPE_CODE, DICT_NAME_TYPE_CODE],
            ),
        ],
        returns: &[ANY_TYPE, ANY_TYPE],
        binder: Some(BinderFacts {
            names: &[type_decl_binder_name],
            bucket: None,
            surface: BinderSurface::UnionDef,
            name_slot: Some(1),
            type_slots: &[],
        }),
        reserved: false,
    },
    // NEWTYPE <name> = <repr>: the representation is type code read where it is written — a type
    // name, a `:(…)` or a `:{…}`.
    BuiltinShape {
        id: BuiltinShapeId::NewTypeDefinition,
        elements: &[
            Kw(&KEYWORDS.newtype),
            slot(Name, &[TYPE_NAME]),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Plain), &[TYPE_CODE]),
        ],
        returns: &[ANY_TYPE],
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
        elements: &[Kw(&KEYWORDS.newtype), slot(Name, &[EXPRESSION])],
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
            slot(Name, &[IDENTIFIER]),
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
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[BLOCK]),
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
    // registers here.
    BuiltinShape {
        id: BuiltinShapeId::CombinedLambda,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Unsupported, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            slot(Unsupported, &[EXPRESSION]),
            Kw(&KEYWORDS.arrow),
            slot(Unsupported, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Unsupported, &[EXPRESSION]),
        ],
        returns: &[NEVER],
        binder: None,
        reserved: true,
    },
    // FN FOR ALL <names> <record schema> -> <return type> = <body> — the quantified lambda. Its
    // schema's field types may name the group's quantifiers, so it is elaborated in the group's
    // scope.
    BuiltinShape {
        id: BuiltinShapeId::QuantifiedLambda,
        elements: &[
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Quantifiers, &[QUANTIFIER_CODE]),
            slot(Signature, &[PROPER_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[BLOCK]),
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
            slot(Quantifiers, &[QUANTIFIER_CODE]),
            slot(Signature, &[PROPER_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
        ],
        returns: &[ANY_TYPE],
        binder: None,
        reserved: false,
    },
    // LET <name> = FN FOR ALL <names> <signature> -> <return type> = <body> — reserved for the
    // same reason `CombinedLambda` is: a combined statement installs a dispatch bucket, and no
    // `FN` signature has a head to key one on.
    BuiltinShape {
        id: BuiltinShapeId::CombinedQuantifiedLambda,
        elements: &[
            Kw(&KEYWORDS.let_),
            slot(Unsupported, &[IDENTIFIER]),
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Unsupported, &[EXPRESSION]),
            slot(Unsupported, &[PROPER_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Unsupported, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Unsupported, &[EXPRESSION]),
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
            slot(Head, &[EXPRESSION]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[BLOCK]),
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
    // shape. The head is a quote, its tokens naming nothing until a definition binds them; the
    // return slot is an ordinary kind expectation, as on every bodyless head.
    BuiltinShape {
        id: BuiltinShapeId::ExpressionHead,
        elements: &[
            Kw(&KEYWORDS.expr),
            slot(Head, &[EXPRESSION]),
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
            slot(Quantifiers, &[QUANTIFIER_CODE]),
            slot(Head, &[EXPRESSION]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[BLOCK]),
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
    // EXPR FOR ALL <names> <head> -> <return type> — the quantified bodyless head. The return may
    // name a quantifier, so it resolves against the group's own scope rather than the surrounding
    // one.
    BuiltinShape {
        id: BuiltinShapeId::QuantifiedExpressionHead,
        elements: &[
            Kw(&KEYWORDS.expr),
            Kw(&KEYWORDS.for_),
            Kw(&KEYWORDS.all),
            slot(Quantifiers, &[QUANTIFIER_CODE]),
            slot(Head, &[EXPRESSION]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // EXPR <head> — a bucket declaration: keywords and one integer or `_` per slot, `EXPR #(MOVE
    // 2 TO 1)`. It ranks its bucket's slots and nothing else: no types, no return, no binder, and
    // in a module body no member.
    BuiltinShape {
        id: BuiltinShapeId::BucketDeclaration,
        elements: &[Kw(&KEYWORDS.expr), slot(Head, &[EXPRESSION])],
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
            slot(Head, &[EXPRESSION]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[BLOCK]),
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
            slot(Quantifiers, &[QUANTIFIER_CODE]),
            slot(Head, &[EXPRESSION]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Lambda), &[BLOCK]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Operator), &[BLOCK]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Operator), &[BLOCK]),
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
            slot(Unsupported, &[EXPRESSION]),
            Kw(&KEYWORDS.over),
            slot(Unsupported, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Unsupported, &[EXPRESSION]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::UnaryOperator), &[BLOCK]),
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
    // on. The symbol is a quote; the operand and result are ordinary kind expectations, so a `:(…)`
    // there sub-dispatches to a type eagerly, exactly as the bodyless `FN` head's return slot does.
    //
    // OP <symbol> OVER <operand>.
    BuiltinShape {
        id: BuiltinShapeId::OperatorHead,
        elements: &[
            Kw(&KEYWORDS.op),
            slot(Data, &[KEYWORD]),
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
            slot(Data, &[KEYWORD]),
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
            slot(Unsupported, &[KEYWORD]),
            Kw(&KEYWORDS.over),
            slot(Unsupported, &[ANY_TYPE]),
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
            slot(Data, &[KEYWORD]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Operator), &[BLOCK]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::Operator), &[BLOCK]),
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
            slot(Unsupported, &[EXPRESSION]),
            Kw(&KEYWORDS.over),
            slot(Unsupported, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Unsupported, &[EXPRESSION]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.over),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
            Kw(&KEYWORDS.equals),
            slot(Body(BodyKind::UnaryOperator), &[BLOCK]),
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
            slot(Definition(DefinitionKind::Members), &[LIST_OF_DECLARATION]),
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
            slot(Definition(DefinitionKind::Members), &[LIST_OF_DECLARATION]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Members), &[LIST_OF_DECLARATION]),
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
            slot(Data, &[KEYWORD]),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            slot(Definition(DefinitionKind::Members), &[LIST_OF_DECLARATION]),
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
            slot(Branches(Heads::Types), &[DICT_TYPE_CODE_BLOCK]),
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
            slot(Branches(Heads::Labels), &[DICT_NAME_BLOCK]),
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
            slot(InPlace, &[ANY]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[PROPER_TYPE]),
            Kw(&KEYWORDS.with),
            slot(Branches(Heads::Labels), &[DICT_NAME_BLOCK]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // CATCH <body>.
    BuiltinShape {
        id: BuiltinShapeId::Catch,
        elements: &[Kw(&KEYWORDS.catch), slot(InPlace, &[ANY])],
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
            slot(Body(BodyKind::Surfaced), &[BLOCK]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // <module> :| <Sig> — the opaque ascription: a view whose unpinned parameters are minted afresh.
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
    // <value> :! <Type> — ascription: the value checked against the type and viewed at it; a
    // module's view is modules'.
    BuiltinShape {
        id: BuiltinShapeId::AscribeTransparent,
        elements: &[
            slot(Argument, &[ANY]),
            Kw(&KEYWORDS.transparent),
            slot(Te, &[ANY_TYPE]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // CLOSE OVER <captures> <body>.
    BuiltinShape {
        id: BuiltinShapeId::CloseOver,
        elements: &[
            Kw(&KEYWORDS.close),
            Kw(&KEYWORDS.over),
            slot(Unsupported, &[EXPRESSION]),
            slot(Unsupported, &[EXPRESSION]),
        ],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // CLOSE <body> — the inferred-capture form.
    BuiltinShape {
        id: BuiltinShapeId::Close,
        elements: &[Kw(&KEYWORDS.close), slot(Unsupported, &[EXPRESSION])],
        returns: &[ANY],
        binder: None,
        reserved: false,
    },
    // <field list> FROM <record>.
    BuiltinShape {
        id: BuiltinShapeId::Projection,
        elements: &[
            slot(Argument, &[LIST_OF_NAME]),
            Kw(&KEYWORDS.from),
            slot(Argument, &[EMPTY_RECORD]),
        ],
        returns: &[EMPTY_RECORD],
        binder: None,
        reserved: false,
    },
    // ATTR <record> <field> — the parse of `m.x`. A type's member is labelled by a type name, so
    // `Point.y` is no overload's.
    BuiltinShape {
        id: BuiltinShapeId::Attribute,
        elements: &[
            Kw(&KEYWORDS.attr),
            slot(Argument, &[MODULE, ANY, ANY_TYPE]),
            slot(Field, &[NAME, NAME, TYPE_NAME]),
        ],
        returns: &[ANY, ANY, ANY],
        binder: None,
        reserved: false,
    },
    // EVAL <code> -> <return type> — runs code, its value held to the declared return as a
    // frame's is. Its binder facts are here for the type slot alone, as `FN`'s are, so a bare
    // `(…)` return spelling rewrites to a sigiled type expression.
    BuiltinShape {
        id: BuiltinShapeId::Eval,
        elements: &[
            Kw(&KEYWORDS.eval),
            slot(Argument, &[ANY_CODE]),
            Kw(&KEYWORDS.arrow),
            slot(Te, &[ANY_TYPE]),
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
    // <code> USING <source> — fills the code's holes from a record's fields or a module's members.
    BuiltinShape {
        id: BuiltinShapeId::UsingCode,
        elements: &[
            slot(Argument, &[ANY_CODE, ANY_CODE]),
            Kw(&KEYWORDS.using),
            slot(Argument, &[EMPTY_RECORD, MODULE]),
        ],
        returns: &[ANY_CODE, ANY_CODE],
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
