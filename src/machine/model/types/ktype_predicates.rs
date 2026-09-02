//! Per-`ExpressionPart` admissibility, per-value type-tag checks, and specificity
//! ordering for dispatch tie-breaking on `KType`. See
//! [design/typing/ktype/README.md](../../../../design/typing/ktype/README.md).
//!
//! A `KType` is a handle, so every predicate here reads its subject's content out of the run's
//! [`TypeRegistry`] and matches on the [`TypeNode`]. Identity questions never need a node at
//! all: two types are equal iff their handles are, one `u128` compare.

use super::kkind::KKind;
use super::ktraits::Parseable;
use super::ktype::KType;
use super::node::TypeNode;
use super::record::Record;
use super::registry::{Relation, TypeRegistry};
use super::sig_schema::{SigSchema, sig_subtype};
use super::signature::{ExpressionSignature, SignatureElement};
use super::type_digest::{TypeDigest, empty_schema_digest};
use crate::machine::SplicedCell;
use crate::machine::core::read_resting;
use crate::machine::model::RunRegistries;
use crate::machine::model::ast::{ExpressionPart, KLiteral, WorkingPart};
use crate::machine::model::values::{Carried, Held, KObject};

/// Whether a value reporting a `ConstructorApply` `ktype()` satisfies a `ConstructorApply`
/// slot: the two constructors are the same type, the two argument records name the same
/// parameters, and each argument matches its same-named counterpart (an `Any` slot admits
/// anything, else exact identity). Drives both [`KType::matches_value`]'s `Wrapped` arm and
/// [`KType::accepts_carried`]'s dispatch arm.
fn constructor_apply_admits(
    slot_constructor: KType,
    slot_arguments: &Record<KType>,
    value_constructor: KType,
    value_arguments: &Record<KType>,
) -> bool {
    slot_constructor == value_constructor
        && value_arguments.len() == slot_arguments.len()
        && slot_arguments.iter().all(|(name, slot_argument)| {
            value_arguments
                .get(name.symbol())
                .is_some_and(|value_argument| {
                    *slot_argument == KType::ANY || *value_argument == *slot_argument
                })
        })
}

/// One raw part shape a carrier slot type can claim — the alphabet the capture footprints below
/// are sets over. A part shape absent here (a literal, a container literal, a keyword) is never
/// claimed by any slot type, so it plays no part in carrier-union disjointness.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureShape {
    /// A bare value-token part.
    Identifier,
    /// A bare `Type`-token part.
    TypeToken,
    /// A sigiled type expression, `:(…)`.
    TypeExpr,
    /// A record type, `:{…}`.
    RecordType,
    /// A `(…)` group or a `#(…)` quote — the eager sub-expression shape.
    Code,
}

/// A set of [`CaptureShape`]s, modeled on
/// [`LazyKinds`](crate::machine::model::lazy_slots::LazyKinds).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CaptureShapes(u8);

impl CaptureShapes {
    pub const EMPTY: CaptureShapes = CaptureShapes(0);

    const fn of(shape: CaptureShape) -> CaptureShapes {
        CaptureShapes(1 << shape as u8)
    }

    pub const fn with(self, other: CaptureShapes) -> CaptureShapes {
        CaptureShapes(self.0 | other.0)
    }

    pub const fn contains(self, other: CaptureShapes) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: CaptureShapes) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// The raw shape `part` presents to a carrier slot, or `None` for a part shape no slot type
/// captures raw. The inverse of the [`capture_footprint`] table: a slot type claims `part` iff its
/// footprint contains this shape.
pub fn capture_shape_of(part: &ExpressionPart<'_>) -> Option<CaptureShape> {
    Some(match part {
        ExpressionPart::Identifier(_) => CaptureShape::Identifier,
        ExpressionPart::Type(_) => CaptureShape::TypeToken,
        ExpressionPart::SigiledTypeExpr(_) => CaptureShape::TypeExpr,
        ExpressionPart::RecordType(_) => CaptureShape::RecordType,
        ExpressionPart::Expression(_) | ExpressionPart::QuotedExpression(_) => CaptureShape::Code,
        _ => return None,
    })
}

/// The **exact carrier members** — the slot types whose whole content is a raw part shape, so a
/// part reaching one is captured verbatim instead of sub-dispatched. These are the members that
/// make a union a *carrier* union: only they carry raw-capture semantics into one, and none of
/// them is nameable from source, so a union carrier slot can only come from builtin registration.
pub fn is_exact_carrier(kt: KType) -> bool {
    matches!(
        kt,
        KType::IDENTIFIER
            | KType::NAME_TOKEN
            | KType::TYPE_NAME_TOKEN
            | KType::SIGILED_TYPE_EXPR
            | KType::RECORD_TYPE
    )
}

/// The raw part shapes `kt` claims: the shapes for which it imposes its own capture or shape-only
/// admission semantics rather than letting the part sub-dispatch. `ProperType` / `AnyType` claim
/// the `Type` token they lower plus the `:(…)` / `:{…}` shapes they shape-admit
/// (`slot_admits_strict`), which is why a kind member cannot share a union with a carrier member
/// over the same shape. Every other slot type — a value type, a container, a nominal — claims
/// nothing.
pub fn capture_footprint(kt: KType) -> CaptureShapes {
    use CaptureShape::{Code, Identifier, RecordType, TypeExpr, TypeToken};
    match kt {
        KType::IDENTIFIER => CaptureShapes::of(Identifier),
        KType::NAME_TOKEN => CaptureShapes::of(Identifier).with(CaptureShapes::of(TypeToken)),
        KType::TYPE_NAME_TOKEN => CaptureShapes::of(TypeToken),
        KType::SIGILED_TYPE_EXPR => CaptureShapes::of(TypeExpr),
        KType::RECORD_TYPE => CaptureShapes::of(RecordType),
        KType::KEXPRESSION => CaptureShapes::of(Code),
        KType::PROPER_TYPE | KType::ANY_TYPE => CaptureShapes::of(TypeToken)
            .with(CaptureShapes::of(TypeExpr))
            .with(CaptureShapes::of(RecordType)),
        _ => CaptureShapes::EMPTY,
    }
}

/// Why `kt` is an ill-formed builtin slot type, or `None` when it is well-formed. Only a **union
/// carrier slot** — a `Union` with at least one [`is_exact_carrier`] member — is
/// constrained; a pure value union like `:(KExpression | Number)` is an ordinary eager slot a user
/// can spell, and is left alone.
///
/// Two rules, both forced by `Union` identity being order-blind: admission and capture must pick
/// the same member for a given part however the union was written.
///
/// 1. No `KExpression` member. A `(…)` group is *the* eager sub-expression shape, so a
///    CODE-capturing union member would make the seal-time raw-kind derivation
///    ([`LAZY_SLOT_SPECS`](crate::machine::model::lazy_slots::LAZY_SLOT_SPECS)) and the group's
///    staging ambiguous.
/// 2. Pairwise capture-footprint disjointness across *all* members, so at most one member ever
///    claims a part shape.
///
/// Only builtin registration can construct a union carrier slot, so a violation is a developer
/// error the seed asserts on rather than a user-facing diagnostic.
pub fn carrier_union_error(kt: KType, registries: &RunRegistries) -> Option<String> {
    let members: Vec<KType> = registries.types.with_node(kt, |node| match node {
        TypeNode::Union { members } => members.clone(),
        _ => Vec::new(),
    });
    if !members.iter().copied().any(is_exact_carrier) {
        return None;
    }
    let name = |m: KType| m.name(registries);
    if members.contains(&KType::KEXPRESSION) {
        return Some(format!(
            "union carrier slot {} may not have a `{}` member: a `(…)` group is the eager \
             sub-expression shape",
            name(kt),
            name(KType::KEXPRESSION),
        ));
    }
    for (i, a) in members.iter().enumerate() {
        for b in &members[i + 1..] {
            if capture_footprint(*a).intersects(capture_footprint(*b)) {
                return Some(format!(
                    "union carrier slot {} has overlapping members `{}` and `{}`: both claim the \
                     same raw part shape, so capture would depend on member order",
                    name(kt),
                    name(*a),
                    name(*b),
                ));
            }
        }
    }
    None
}

/// The slot types that constrain nothing beyond "a name": a concrete type out-specifies any of
/// them.
fn is_unconstrained_name(kt: KType) -> bool {
    kt == KType::IDENTIFIER
        || kt == KType::of_kind(KKind::ProperType)
        || kt == KType::NAME_TOKEN
        || kt == KType::TYPE_NAME_TOKEN
}

impl KType {
    /// True iff `self` is `needle`, or a `Union` with `needle` among its members. The
    /// distribution primitive for the structural exact-constant checks dispatch runs on a slot
    /// type: a rule written against a carrier constant reads through here to cover the union
    /// spelling of the same slot.
    pub fn union_has_member(self, needle: KType, types: &TypeRegistry) -> bool {
        self == needle
            || types.with_node(self, |node| match node {
                TypeNode::Union { members } => members.contains(&needle),
                _ => false,
            })
    }

    /// The [`is_exact_carrier`] member of this union that claims `part`'s raw shape,
    /// or `None` — for a non-union, for a union with no such member, and for a part shape no
    /// carrier claims. `Some` is what routes a part to raw capture instead of eager
    /// sub-dispatch, so an `of_kind(…)` member never answers here: it is an ordinary eager
    /// member. [`carrier_union_error`] forbids two members claiming one shape, so the
    /// answer is unique regardless of member order.
    pub fn raw_capture_member(
        self,
        part: &ExpressionPart<'_>,
        types: &TypeRegistry,
    ) -> Option<KType> {
        self.member_claiming(part, types, true)
    }

    /// The bind-time reduction of a union slot to the member whose capture semantics apply to
    /// `part`: [`raw_capture_member`](Self::raw_capture_member) widened to admit a
    /// `ProperType` / `AnyType` member for a bare `Type` token, which such a member lowers to
    /// [`Held::Type`] / [`Held::UnresolvedType`] rather than capturing raw.
    pub fn capture_member_for(
        self,
        part: &ExpressionPart<'_>,
        types: &TypeRegistry,
    ) -> Option<KType> {
        self.member_claiming(part, types, false)
    }

    fn member_claiming(
        self,
        part: &ExpressionPart<'_>,
        types: &TypeRegistry,
        exact_only: bool,
    ) -> Option<KType> {
        let shape = capture_shape_of(part)?;
        let eligible = |m: KType| {
            is_exact_carrier(m)
                || (!exact_only
                    && shape == CaptureShape::TypeToken
                    && matches!(m, KType::PROPER_TYPE | KType::ANY_TYPE))
        };
        let wanted = CaptureShapes::of(shape);
        types.with_node(self, |node| match node {
            TypeNode::Union { members } => members
                .iter()
                .copied()
                .find(|m| eligible(*m) && capture_footprint(*m).contains(wanted)),
            _ => None,
        })
    }

    /// Whether a slot of this type *owns* a bare name part rather than letting it resolve — the
    /// auto-wrap exclusion ([`KFunction::classify_for_pick`](crate::machine::KFunction)). Blanket
    /// by slot type, not by part shape: an `:Identifier` slot owns even a bare `Type` token it
    /// cannot admit, and a union owns one as soon as any member does.
    pub fn owns_bare_name(self, types: &TypeRegistry) -> bool {
        fn is_owner(kt: KType) -> bool {
            matches!(
                kt,
                KType::IDENTIFIER | KType::PROPER_TYPE | KType::NAME_TOKEN | KType::TYPE_NAME_TOKEN
            )
        }
        is_owner(self)
            || types.with_node(self, |node| match node {
                TypeNode::Union { members } => members.iter().copied().any(is_owner),
                _ => false,
            })
    }

    /// Strict specificity ordering. Concrete types outrank `Any` and the unconstrained-name slot
    /// types (`Identifier`, `ProperType`, `NameToken`, `TypeNameToken`), so an overload like
    /// `ATTR <s:NewType>` beats its `ATTR <s:Identifier>` sibling when both admit. `Str` is the
    /// one exception to that rule: a bare-token slot out-specifies it, because the token slot and
    /// the `Str` slot read the same bare token at different depths (see `more_specific_walk`).
    /// Among the name slots themselves, `Identifier` and `TypeNameToken` each out-specify
    /// `NameToken`, admitting one of its two part shapes. A nominal-family kind out-specifies
    /// `OfKind(ProperType)` (`OfKind(NewType) ≺ OfKind(ProperType)`), and a sealed member
    /// out-specifies the `OfKind(kind)` of its own family. Parameterized containers are
    /// covariant in their inner slots. Returns `false` for equal types.
    ///
    /// Every handle's digest is content-derived, so the `(subject, candidate)` pair is always a
    /// sound verdict key and the walk is memoized unconditionally.
    pub fn is_more_specific_than(self, other: KType, registries: &RunRegistries) -> bool {
        let types = &registries.types;
        let (subject, candidate) = (self.digest(), other.digest());
        if let Some(verdict) = types.verdict(subject, candidate, Relation::MoreSpecific) {
            return verdict;
        }
        let verdict = self.more_specific_walk(other, registries);
        types.record_verdict(subject, candidate, Relation::MoreSpecific, verdict);
        verdict
    }

    fn more_specific_walk(self, other: KType, registries: &RunRegistries) -> bool {
        let types = &registries.types;
        if other == KType::ANY && self != KType::ANY {
            return true;
        }
        // An `Identifier` slot claims the token itself; a `Str` slot claims the token's resolved
        // value. When one bucket offers both readings of the same bare token, the token reading
        // wins: a name binds bare wherever an `Identifier` slot admits it, and a string binding
        // that shadows the name cannot steal it. The second guard exempts `Str` from the general
        // "concrete outranks the unconstrained-name slots" rule below.
        if self == KType::IDENTIFIER && other == KType::STR {
            return true;
        }
        if self == KType::STR && other == KType::IDENTIFIER {
            return false;
        }
        // A name-token slot claims a bare field/binder token the way `Identifier` does, so the
        // same token-over-resolved-string rule applies (ATTR keeps `field :Str` siblings in the
        // bucket).
        if self == KType::NAME_TOKEN && other == KType::STR {
            return true;
        }
        if self == KType::STR && other == KType::NAME_TOKEN {
            return false;
        }
        // Among the name slots themselves, strictly-narrower part admission is strictly more
        // specific: `Identifier` and `TypeNameToken` each admit one of `NameToken`'s two part
        // shapes.
        if (self == KType::IDENTIFIER || self == KType::TYPE_NAME_TOKEN)
            && other == KType::NAME_TOKEN
        {
            return true;
        }
        if is_unconstrained_name(other) && !(is_unconstrained_name(self) || self == KType::ANY) {
            return true;
        }
        // Two reads nested: the recursion below runs under both borrows, which is sound because no
        // predicate interns.
        types.with_node(self, |subject| {
            types.with_node(other, |candidate| {
                match (subject, candidate) {
                    (TypeNode::List { element: a }, TypeNode::List { element: b }) => {
                        a.is_more_specific_than(*b, registries)
                    }
                    (
                        TypeNode::Dict { key: ka, value: va },
                        TypeNode::Dict { key: kb, value: vb },
                    ) => {
                        let key_more = ka.is_more_specific_than(*kb, registries);
                        let value_more = va.is_more_specific_than(*vb, registries);
                        (key_more && (value_more || va == vb)) || (ka == kb && value_more)
                    }
                    // Record-value subtyping: width-superset + covariant depth (the dual of the
                    // contravariant width-drop `param_record_more_specific` for function params).
                    (TypeNode::Record { fields: a }, TypeNode::Record { fields: b }) => {
                        record_value_more_specific(a, b, registries)
                    }
                    // Function subtyping: contravariant params (width-subset), covariant return —
                    // see `param_record_more_specific`.
                    (
                        TypeNode::KFunction {
                            params: pa,
                            ret: ra,
                        },
                        TypeNode::KFunction {
                            params: pb,
                            ret: rb,
                        },
                    ) => param_record_more_specific(pa, *ra, pb, *rb, registries),
                    // Value role: a concrete signature type is more specific than the
                    // `:Signature` wildcard.
                    (TypeNode::Signature { .. }, TypeNode::OfKind(KKind::Signature)) => true,
                    (
                        TypeNode::Signature {
                            schema: schema_a,
                            schema_digest: digest_a,
                        },
                        TypeNode::Signature {
                            schema: schema_b,
                            schema_digest: digest_b,
                        },
                    ) => {
                        let empty = empty_schema_digest();
                        // Any non-empty signature refines the empty interface (the lattice top). Keyed on
                        // empty *content*, not the mint that produced it, so a zero-member `SIG E = ()` is
                        // the same top as `:Module`.
                        if *digest_b == empty && *digest_a != empty {
                            return true;
                        }
                        if digest_a == digest_b {
                            // One content is one handle — equal is not strictly more specific. (A `WITH`
                            // specialization folds its pins into the schema, so a refinement always
                            // lands a distinct content and takes the structural compare below —
                            // `S WITH {A = Number} ≺ S` because the folded manifest strictly
                            // `sig_subtype`s the abstract original.)
                            return false;
                        }
                        // Two different interfaces — SIG-declared, `WITH`-specialized, or self-sig, any
                        // combination — compare by strict structural subtyping: `a ≺ b` iff `a`'s schema
                        // strictly `sig_subtype`s `b`'s.
                        sig_schema_more_specific(
                            schema_a,
                            self.digest(),
                            schema_b,
                            other.digest(),
                            registries,
                        )
                    }
                    // A nominal-family kind out-specifies `OfKind(ProperType)` — `OfKind(NewType) ≺
                    // OfKind(ProperType)`. (Against `Identifier` / `OfKind(ProperType)` the generic rule
                    // above already fires; this covers a nominal-vs-nominal-supertype tie.)
                    (TypeNode::OfKind(a), TypeNode::OfKind(b)) if a.strictly_below(*b) => true,
                    // A sealed nominal member is more specific than the `OfKind` wildcard of the same
                    // surface family — read the member's `kind` off its node.
                    (TypeNode::SetMember { kind, .. }, TypeNode::OfKind(b)) if kind == b => true,
                    (
                        TypeNode::ConstructorApply {
                            constructor: ca,
                            arguments: aa,
                        },
                        TypeNode::ConstructorApply {
                            constructor: cb,
                            arguments: ab,
                        },
                    ) if ca == cb
                        && aa.len() == ab.len()
                        && aa.keys().all(|name| ab.get(name.symbol()).is_some()) =>
                    {
                        // Same constructor, same parameter names: compare each argument against its
                        // same-named counterpart.
                        let pairs = || {
                            aa.iter()
                                .map(|(name, x)| (*x, *ab.get(name.symbol()).unwrap()))
                        };
                        let any_more = pairs().any(|(x, y)| x.is_more_specific_than(y, registries));
                        let all_equal_or_more =
                            pairs().all(|(x, y)| x == y || x.is_more_specific_than(y, registries));
                        any_more && all_equal_or_more
                    }
                    // Union subset: `a` refines `b` iff they are not the same set and every member of
                    // `a` is equal to or more specific than some member of `b`. Two identical unions are
                    // one handle, so the strictness gate is a set compare of distinct handles.
                    (TypeNode::Union { members: a }, TypeNode::Union { members: b }) => {
                        let same_set = a.len() == b.len() && a.iter().all(|m| b.contains(m));
                        !same_set
                            && a.iter().all(|x| {
                                b.iter()
                                    .any(|y| x == y || x.is_more_specific_than(*y, registries))
                            })
                    }
                    // Each member of a union is a subtype of it: a non-union `x` is more specific than
                    // `Union(ms)` iff it equals or refines one of the members.
                    (_, TypeNode::Union { members }) => members
                        .iter()
                        .any(|m| self == *m || self.is_more_specific_than(*m, registries)),
                    _ => false,
                }
            })
        })
    }

    /// True iff `carried` satisfies a slot declared as `self` — exact match or covariant
    /// refinement. A `List<Any>` value (the join an empty or heterogeneous literal
    /// memoizes) does not satisfy `:(LIST OF Number)`.
    pub fn satisfied_by(self, carried: KType, registries: &RunRegistries) -> bool {
        self == carried || carried.is_more_specific_than(self, registries)
    }

    /// True iff a runtime `KObject` value satisfies this declared type.
    /// Aggregate-cell satisfaction: an `Object` cell defers to [`matches_value`]; a `Type`
    /// cell (a first-class type stored in a list/dict/record) satisfies a type-accepting
    /// slot — `Any`, an `OfKind` kind that subsumes the type's `kind_of`, or an exact type
    /// identity.
    pub fn matches_held(self, cell: &Held<'_>, registries: &RunRegistries) -> bool {
        let types = &registries.types;
        match cell {
            Held::Object(o) => self.matches_value(o, registries),
            Held::Type(t) => types.with_node(self, |node| match node {
                TypeNode::Any => true,
                TypeNode::OfKind(k) => k.admits(t.kind_of(types)),
                _ => self == *t,
            }),
            // An aggregate cell holds a value or a resolved type; neither of the bind seam's
            // name carriers ever becomes one, so no slot classifies them.
            Held::UnresolvedType(_) | Held::Name(_) => false,
        }
    }

    pub fn matches_value(self, obj: &KObject<'_>, registries: &RunRegistries) -> bool {
        let types = &registries.types;
        types.with_node(self, |node| match node {
            TypeNode::Any => true,
            TypeNode::List { element } => match obj {
                KObject::List(substrate, _) => substrate
                    .elements()
                    .iter()
                    .all(|x| element.matches_held(x, registries)),
                _ => false,
            },
            TypeNode::Dict { key, value } => match obj {
                KObject::Dict(substrate, _) => substrate.entries().all(|(map_key, held)| {
                    (*key == KType::ANY || *key == map_key.ktype())
                        && value.matches_held(held, registries)
                }),
                _ => false,
            },
            // Every slot field must be present in the value and match (depth). Extra value
            // fields are fine — a wider record value is more specific than a narrower slot.
            TypeNode::Record { fields } => match obj {
                KObject::Record(substrate, _) => fields.iter().all(|(name, field_type)| {
                    substrate
                        .field(name.symbol())
                        .map(|v| field_type.matches_held(v, registries))
                        .unwrap_or(false)
                }),
                _ => false,
            },
            TypeNode::KFunction { params, ret } => match obj {
                KObject::KFunction(f) => function_compat(&f.signature, params, *ret, registries),
                _ => false,
            },
            // Constraint role: a signature slot is satisfied by a module value on the Object
            // channel, via [`Module::satisfies_sig_schema`]. `WITH` pins are folded into the
            // schema as manifest members, so pinned-slot agreement is the manifest-equality leg
            // of the same structural check.
            TypeNode::Signature {
                schema,
                schema_digest,
            } => match obj {
                KObject::Module(m) => m.satisfies_sig_schema(schema, *schema_digest, registries),
                _ => false,
            },
            // A type-accepting slot is **type-channel-only**: no runtime `KObject` is a type
            // value, so a value is never matched by a kind. `ProperType` / `AnyType` keep a
            // defensive identity check for the rare case of a type carried as a value
            // (`OfKind(ProperType) == ktype()`); every other kind admits no runtime instance.
            TypeNode::OfKind(k) => match k {
                KKind::ProperType | KKind::AnyType => self == obj.ktype(),
                _ => false,
            },
            // A stamped carrier (from ascription) takes precedence and is checked structurally
            // per parameter name; an erased carrier — one whose identity is still the bare member
            // handle — falls back to checking its payload against the argument its own member name
            // keys, which the applied member and the value's member agree on by construction.
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => match obj {
                KObject::Wrapped { type_id, inner } => {
                    types.with_node(*type_id, |value_node| match value_node {
                        TypeNode::ConstructorApply {
                            constructor: value_constructor,
                            arguments: value_arguments,
                        } => constructor_apply_admits(
                            *constructor,
                            arguments,
                            *value_constructor,
                            value_arguments,
                        ),
                        _ => {
                            if *type_id != *constructor {
                                return false;
                            }
                            let member_name = types.with_node(*constructor, |node| match node {
                                TypeNode::SetMember { name, .. } => Some(*name),
                                _ => None,
                            });
                            match member_name.and_then(|name| arguments.get(name.symbol())) {
                                Some(argument) => {
                                    argument.matches_value(inner.payload(), registries)
                                }
                                None => true,
                            }
                        }
                    })
                }
                _ => false,
            },
            // A union slot admits a value any of its members admits.
            TypeNode::Union { members } => members.iter().any(|m| m.matches_value(obj, registries)),
            // A sealed nominal slot admits a value whose `ktype()` reports the same member
            // handle — a per-variant newtype `Wrapped` value or a `TypeConstructor` value.
            _ => self == obj.ktype(),
        })
    }

    /// True iff a first-class type `t` (flowing in the type channel) satisfies this declared
    /// slot — the type-channel analog of [`matches_value`]. An `OfKind` slot is satisfied when its
    /// kind subsumes `t.kind_of()` (so `OfKind(ProperType)` admits any proper type, including a
    /// nominal, while the signature wall keeps `ProperType` from admitting a signature); `Any` by
    /// anything; a signature *value* slot by structural identity. A signature slot admits no
    /// first-class type here — it is a constraint on a module, and a module surfaces on the
    /// Object channel, matched by [`matches_value`]. Other concrete slots compare against the
    /// `OfKind(ProperType)` dispatch identity a non-signature type carrier reports, so they admit
    /// no bare type value.
    pub fn matches_type(self, t: KType, types: &TypeRegistry) -> bool {
        // The shallow dispatch identity a concrete slot compares against: a signature carries its
        // identity directly; every other type fills the `OfKind(ProperType)` marker.
        let carrier_ktype = types.with_node(t, |node| match node {
            TypeNode::Signature { .. } => t,
            _ => KType::of_kind(KKind::ProperType),
        });
        types.with_node(self, |node| match node {
            TypeNode::Any => true,
            TypeNode::Signature { .. } => false,
            TypeNode::OfKind(k) => k.admits(t.kind_of(types)),
            // A union slot is satisfied by any type its members are satisfied by.
            TypeNode::Union { members } => members.iter().any(|m| m.matches_type(t, types)),
            _ => self == carrier_ktype,
        })
    }

    /// Per-value admissibility for a resolved [`Carried`] argument — the classifier the spliced
    /// arms of [`accepts_part`] delegate to, and what a spliced cell opens against at its own brand.
    /// The slot is a handle and the value (`'v`) is a region borrow, so every comparison is a
    /// verdict-only structural check, none of which needs the value's own lifetime.
    /// "Dispatch trusts the carried element type": a container's memoized carried `KType` is read
    /// via `satisfied_by`, never by walking its contents.
    pub fn accepts_carried<'v>(self, c: Carried<'v>, registries: &RunRegistries) -> bool {
        let types = &registries.types;
        types.with_node(self, |node| match node {
            TypeNode::Any => true,
            TypeNode::Number => matches!(c, Carried::Object(KObject::Number(_))),
            TypeNode::Str => matches!(c, Carried::Object(KObject::KString(_))),
            TypeNode::Bool => matches!(c, Carried::Object(KObject::Bool(_))),
            TypeNode::Null => matches!(c, Carried::Object(KObject::Null)),
            // Evaluated container: the value already memoizes its own full container-type
            // handle, so the check is one `satisfied_by` between two handles — no element walk,
            // and nothing to rebuild or re-digest.
            TypeNode::List { .. } => match c {
                Carried::Object(KObject::List(_, carried)) => {
                    self.satisfied_by(*carried, registries)
                }
                _ => false,
            },
            TypeNode::Dict { .. } => match c {
                Carried::Object(KObject::Dict(_, carried)) => {
                    self.satisfied_by(*carried, registries)
                }
                _ => false,
            },
            TypeNode::Record { .. } => match c {
                Carried::Object(KObject::Record(_, carried)) => {
                    self.satisfied_by(*carried, registries)
                }
                _ => false,
            },
            TypeNode::KFunction { params, ret } => match c {
                Carried::Object(KObject::KFunction(f)) => {
                    function_compat(&f.signature, params, *ret, registries)
                }
                _ => false,
            },
            // A `:KExpression` parameter of a user signature is an ordinary eager value slot: a
            // `#(…)` literal arrives as a part shape, and every other expression producing code
            // arrives here as the value it evaluated to. A builtin's lazy slot never reaches this
            // arm — the stamp keeps its part raw, so it classifies through `accepts_part`.
            TypeNode::KExpression => matches!(c, Carried::Object(KObject::KExpression(_))),
            // The remaining part-shape-only slots are builtin raw-capture slots and nothing else,
            // so they admit a parser part shape and never a resolved value.
            TypeNode::Identifier
            | TypeNode::SigiledTypeExpr
            | TypeNode::RecordType
            | TypeNode::NameToken
            | TypeNode::TypeNameToken => false,
            // Type-accepting slot, type-channel-only, by shallow kind via `kind_of` subsumption:
            // a first-class type value is admitted iff the slot kind subsumes the value's
            // `kind_of`, so `Any` takes every type value (signatures included), `ProperType`
            // takes any non-signature type, `Signature` takes only its own carrier, and a
            // nominal-kind slot only its own family. An object value reports a non-type `kind_of`
            // and is refused.
            TypeNode::OfKind(k) => match c {
                Carried::Type(ty) => k.admits(ty.kind_of(types)),
                _ => false,
            },
            // Handle equality is the per-declaration identity check for a sealed nominal type. A
            // per-variant newtype value carries its member handle, so a union-typed slot admits
            // each variant via the member delegation below.
            TypeNode::SetMember { .. } => c.ktype(types) == self,
            // A union slot admits an argument any of its members admits. `Carried` is `Copy`,
            // so each member reads the same carried value.
            TypeNode::Union { members } => members.iter().any(|m| m.accepts_carried(c, registries)),
            TypeNode::AbstractType { .. } => c.ktype(types) == self,
            // Constraint role: a `:S` slot admits a *module* whose self-sig satisfies the
            // signature — no ascription required. A `WITH` pin is a manifest member of the
            // folded schema, checked by the same structural relation. A module is a value, so
            // both the overload-picker probe and the built argument cell carry it on the Object
            // channel. A signature *value* is admitted by the `OfKind(Signature)` wildcard
            // above, never here.
            TypeNode::Signature {
                schema,
                schema_digest,
            } => match c {
                Carried::Object(KObject::Module(m)) => {
                    m.satisfies_sig_schema(schema, *schema_digest, registries)
                }
                _ => false,
            },
            // A sibling reference is meaningful only inside its pre-seal window and never
            // reaches a real argument slot.
            TypeNode::Sibling(_) => false,
            // Confined to a synthesized FN `ret` slot — never a free-standing argument slot.
            TypeNode::DeferredReturn(_) => false,
            // Two carriers satisfy a `ConstructorApply` slot: a first-class meta-type value with
            // an equal inner type, and an identity-wrapper `Wrapped` object whose `ktype()` is
            // itself a `ConstructorApply` (a `NEWTYPE (Type AS Wrapper)`-constructed value) —
            // admitted by the same constructor + per-argument rule the `matches_value` arm uses.
            TypeNode::ConstructorApply {
                constructor: slot_constructor,
                arguments: slot_arguments,
            } => match c {
                Carried::UnresolvedType(_) => false,
                Carried::Type(kt) => kt == self,
                Carried::Object(obj) => {
                    types.with_node(obj.ktype(), |value_node| match value_node {
                        TypeNode::ConstructorApply {
                            constructor: value_constructor,
                            arguments: value_arguments,
                        } => constructor_apply_admits(
                            *slot_constructor,
                            slot_arguments,
                            *value_constructor,
                            value_arguments,
                        ),
                        _ => false,
                    })
                }
            },
        })
    }

    /// Classify a spliced **cell** against this slot without adopting it — opens the delivery
    /// envelope at a fresh brand under its retained host pin and routes the opened value through
    /// [`accepts_carried`](Self::accepts_carried) at that brand. No cast: the slot is a handle,
    /// so it carries no brand of its own for the opened value's brand to relate to — a
    /// verdict-only walk needs no re-anchoring. The picker may reject the candidate, so this
    /// deliberately does not adopt.
    pub(crate) fn accepts_cell(self, cell: &SplicedCell, registries: &RunRegistries) -> bool {
        read_resting(cell, |c| self.accepts_carried(c, registries))
    }

    /// Per-[`WorkingPart`] admissibility for argument slots — the dispatch-path peer of
    /// [`accepts_part`](Self::accepts_part), which classifies raw AST shapes. Only the scheduler's
    /// own arms are answered here; a part pointing at the AST delegates.
    pub fn accepts_working_part(self, part: &WorkingPart<'_>, registries: &RunRegistries) -> bool {
        let types = &registries.types;
        match part {
            WorkingPart::Ast(part) => self.accepts_part(part, types),
            // A resolved sub-result opens at its own brand through `accepts_cell`, which routes the
            // opened value through `accepts_carried` — no cast.
            WorkingPart::Spliced { cell } => self.accepts_cell(cell, registries),
            // A slot the scheduler has yet to fill — a node it synthesized and will dispatch, or a
            // staging hole awaiting its sibling's carrier. Neither denotes a value yet, and both
            // become a `Spliced` cell before anything binds, so only an `Any` slot admits one.
            WorkingPart::Expression(_) | WorkingPart::RecordType(_) | WorkingPart::StagedSlot => {
                types.with_node(self, |node| matches!(node, TypeNode::Any))
            }
        }
    }

    /// The type dispatch matches `part` on — the inverse of
    /// [`accepts_part`](Self::accepts_part), read as a function instead of a predicate. A
    /// diagnostic naming a slot renders this, because it is the whole of what dispatch saw there.
    ///
    /// `None` for a keyword, which is fixed syntax filling no slot. A raw type token answers
    /// `ProperType` — the kind a parser type name denotes, not the type it names. A container
    /// literal types its own contents: elements recurse through here and
    /// [`TypeRegistry::join_iter`] joins them, the same rule
    /// [`KObject::list_of_held`](crate::machine::model::KObject::list_of_held) applies once those
    /// elements have evaluated, so `[1 2 3]` answers `:(LIST OF Number)` whether or not it has run
    /// yet.
    ///
    /// Kept beside `accepts_part` because the two must agree: the type reported here must be one
    /// `accepts_part` admits, which `slot_ktype_round_trips_through_accepts_part` pins for every
    /// part shape.
    pub fn slot_ktype(part: &ExpressionPart<'_>, types: &TypeRegistry) -> Option<KType> {
        Some(match part {
            ExpressionPart::Keyword(_) => return None,
            ExpressionPart::Literal(KLiteral::Number(_)) => KType::NUMBER,
            ExpressionPart::Literal(KLiteral::String(_)) => KType::STR,
            ExpressionPart::Literal(KLiteral::Boolean(_)) => KType::BOOL,
            ExpressionPart::Literal(KLiteral::Null) => KType::NULL,
            ExpressionPart::ListLiteral(items) => {
                let element = types.join_iter(
                    items
                        .iter()
                        .map(|i| KType::slot_ktype(i, types).unwrap_or(KType::ANY)),
                );
                types.list(element)
            }
            ExpressionPart::DictLiteral(pairs) => {
                let key = types.join_iter(
                    pairs
                        .iter()
                        .map(|(k, _)| KType::slot_ktype(k, types).unwrap_or(KType::ANY)),
                );
                let value = types.join_iter(
                    pairs
                        .iter()
                        .map(|(_, v)| KType::slot_ktype(v, types).unwrap_or(KType::ANY)),
                );
                types.dict(key, value)
            }
            ExpressionPart::RecordLiteral(fields) => {
                types.record(Record::from_pairs(fields.iter().map(|(name, value)| {
                    (*name, KType::slot_ktype(value, types).unwrap_or(KType::ANY))
                })))
            }
            ExpressionPart::Identifier(_) => KType::IDENTIFIER,
            ExpressionPart::Expression(_) | ExpressionPart::QuotedExpression(_) => {
                KType::KEXPRESSION
            }
            ExpressionPart::SigiledTypeExpr(_) => KType::SIGILED_TYPE_EXPR,
            ExpressionPart::RecordType(_) => KType::RECORD_TYPE,
            ExpressionPart::Type(_) => KType::PROPER_TYPE,
        })
    }

    /// Per-[`ExpressionPart`] admissibility for argument slots: a shape check on raw parser syntax.
    /// Unevaluated container literals admit shape-only (element types unknown until evaluation).
    /// Non-satisfying containers fall through the scope walk rather than failing the bind. A part
    /// the scheduler produced classifies through
    /// [`accepts_working_part`](Self::accepts_working_part) instead.
    pub fn accepts_part(self, part: &ExpressionPart<'_>, types: &TypeRegistry) -> bool {
        types.with_node(self, |node| match node {
            TypeNode::Any => true,
            TypeNode::Number => matches!(part, ExpressionPart::Literal(KLiteral::Number(_))),
            TypeNode::Str => matches!(part, ExpressionPart::Literal(KLiteral::String(_))),
            TypeNode::Bool => matches!(part, ExpressionPart::Literal(KLiteral::Boolean(_))),
            TypeNode::Null => matches!(part, ExpressionPart::Literal(KLiteral::Null)),
            // An unevaluated container literal admits shape-only (element types unknown until
            // evaluation, so two container-typed overloads tie and defer-then-reevaluate).
            TypeNode::List { .. } => matches!(part, ExpressionPart::ListLiteral(_)),
            TypeNode::Dict { .. } => matches!(part, ExpressionPart::DictLiteral(_)),
            TypeNode::Record { .. } => matches!(part, ExpressionPart::RecordLiteral(_)),
            // A function slot admits no parser part shape — only a resolved value, handled
            // above by `accepts_carried`.
            TypeNode::KFunction { .. } => false,
            TypeNode::Identifier => matches!(part, ExpressionPart::Identifier(_)),
            // The binder-position slots: a bare name token of the class(es) they admit, captured
            // raw. Neither is an `OfKind`, so no name reaching one is ever lowered or resolved.
            TypeNode::NameToken => matches!(
                part,
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_)
            ),
            TypeNode::TypeNameToken => matches!(part, ExpressionPart::Type(_)),
            // A `:KExpression` slot captures a parenthesized expression raw, and a `#(...)` quote —
            // whose body is already data — with it.
            TypeNode::KExpression => matches!(
                part,
                ExpressionPart::Expression(_) | ExpressionPart::QuotedExpression(_)
            ),
            TypeNode::SigiledTypeExpr => matches!(part, ExpressionPart::SigiledTypeExpr(_)),
            TypeNode::RecordType => matches!(part, ExpressionPart::RecordType(_)),
            // A raw parser type token is a proper type name, admitted only for `ProperType` /
            // `AnyType`; a first-class type *value* reaches `accepts_carried` above.
            TypeNode::OfKind(k) => match part {
                ExpressionPart::Type(_) => matches!(k, KKind::ProperType | KKind::AnyType),
                _ => false,
            },
            // The nominal / signature / constructor slots classify only resolved values
            // (via `accepts_carried`); no parser part shape satisfies them. A union delegates to
            // its members, and a member admits a part only for a shape it classifies — a literal
            // for `Number` / `Str` / `Bool` / `Null`.
            TypeNode::Union { members } => members.iter().any(|m| m.accepts_part(part, types)),
            TypeNode::SetMember { .. }
            | TypeNode::AbstractType { .. }
            | TypeNode::Signature { .. }
            | TypeNode::ConstructorApply { .. } => false,
            // A sibling reference is meaningful only inside its pre-seal window and never
            // reaches a real argument slot.
            TypeNode::Sibling(_) => false,
            // Confined to a synthesized FN `ret` slot — never a free-standing argument slot.
            TypeNode::DeferredReturn(_) => false,
        })
    }
}

/// Strict cross-interface specificity for two signature types with DIFFERENT schema digests
/// (SIG-declared or self-sig, any combination). `a` is strictly more specific than `b` iff `a`'s
/// pin-folded schema is a `sig_subtype` of `b`'s pin-folded schema in the forward direction only —
/// the reverse must fail, or the two are mutually-satisfying and neither strictly refines. Both
/// directions record a verdict under `SigSatisfies`, keyed by the two signature handles' digests
/// (which fold their pins, so the key is exact).
fn sig_schema_more_specific(
    a: &SigSchema,
    digest_a: TypeDigest,
    b: &SigSchema,
    digest_b: TypeDigest,
    registries: &RunRegistries,
) -> bool {
    let types = &registries.types;
    let forward_hit = types.verdict(digest_a, digest_b, Relation::SigSatisfies);
    let reverse_hit = types.verdict(digest_b, digest_a, Relation::SigSatisfies);
    if let (Some(forward), Some(reverse)) = (forward_hit, reverse_hit) {
        return forward && !reverse;
    }
    let forward = forward_hit.unwrap_or_else(|| {
        let verdict = sig_subtype(a, b, registries).is_ok();
        types.record_verdict(digest_a, digest_b, Relation::SigSatisfies, verdict);
        verdict
    });
    let reverse = reverse_hit.unwrap_or_else(|| {
        let verdict = sig_subtype(b, a, registries).is_ok();
        types.record_verdict(digest_b, digest_a, Relation::SigSatisfies, verdict);
        verdict
    });
    forward && !reverse
}

/// Name-keyed specificity for the `KFunction` arm of
/// [`KType::is_more_specific_than`]. Function subtyping is
/// contravariant in parameters (with width-subset) and covariant in the return,
/// matching the value-into-slot gate in [`function_compat`] so most-specific-wins
/// stays consistent. `self` (the `a` side) is strictly more specific than `other`
/// (the `b` side) iff:
/// - width-subset: `pa.keys() ⊆ pb.keys()` (the more-specific function declares no
///   more parameters — guard returns `false` otherwise);
/// - per shared name, contravariant: `pb[name] == pa[name] || pb[name] ≺ pa[name]`
///   (the more-specific function's params are equal-or-more-general);
/// - covariant return: `ra == rb || ra ≺ rb`;
/// - at least one strict edge (narrower width, a strictly-more-general param, or a
///   strictly-more-specific return).
fn param_record_more_specific(
    pa: &Record<KType>,
    ra: KType,
    pb: &Record<KType>,
    rb: KType,
    registries: &RunRegistries,
) -> bool {
    if !pa.keys().all(|k| pb.get(k.symbol()).is_some()) {
        return false;
    }
    let params_ok = pa.iter().all(|(name, s)| {
        let o = *pb.get(name.symbol()).unwrap();
        o == *s || o.is_more_specific_than(*s, registries)
    });
    let params_more = pa.keys().any(|k| {
        pb.get(k.symbol())
            .unwrap()
            .is_more_specific_than(*pa.get(k.symbol()).unwrap(), registries)
    });
    let ret_more = ra.is_more_specific_than(rb, registries);
    let ret_ok = ra == rb || ret_more;
    let width_strict = pa.len() < pb.len();
    params_ok && ret_ok && (width_strict || params_more || ret_more)
}

/// Width/depth specificity for *record values* — the **dual** of
/// [`param_record_more_specific`]. A record value's fields are covariant (the value is
/// immutable — see [memory-model](../../../../design/memory-model.md)), and a *wider*
/// record is more specific: a `{x, y}` value fills an `{x}` slot. So `a` is strictly more
/// specific than `b` iff:
/// - width-superset: `b.keys() ⊆ a.keys()` (`a` declares every field `b` does, maybe
///   more — guard returns `false` otherwise);
/// - per shared name, covariant: `a[name] == b[name] || a[name] ≺ b[name]`;
/// - at least one strict edge (wider width, or a strictly-more-specific shared field).
///
/// Contrast `param_record_more_specific`, which is *contravariant* with width-*drop* for
/// call-by-name function parameters. Records and function params share the `Record`
/// substrate but order opposite ways — do **not** unify the two helpers.
fn record_value_more_specific(
    a: &Record<KType>,
    b: &Record<KType>,
    registries: &RunRegistries,
) -> bool {
    if !b.keys().all(|k| a.get(k.symbol()).is_some()) {
        return false;
    }
    let depth_ok = b.iter().all(|(name, bt)| {
        let at = *a.get(name.symbol()).unwrap();
        at == *bt || at.is_more_specific_than(*bt, registries)
    });
    let depth_more = b.keys().any(|k| {
        a.get(k.symbol())
            .unwrap()
            .is_more_specific_than(*b.get(k.symbol()).unwrap(), registries)
    });
    let width_strict = a.len() > b.len();
    depth_ok && (width_strict || depth_more)
}

/// Sound, order-blind, name-keyed function subtyping: does the value function `sig`
/// fill the slot whose params record is `params` and return type is `ret`? Reasoned
/// against call-by-name invocation (params arrive name-keyed), so the variance is:
/// - Return covariant for a `Resolved` value return: `sig_ret == ret || sig_ret ≺ ret`
///   — a value returning a subtype of the slot's promised return fills the slot.
/// - Return *syntactic* for a `Deferred` value return: the deferred surface form is
///   compared against the slot's `ret`. An `Any` slot admits any deferred return; a
///   `DeferredReturn` slot (synthesized from another deferred-return FN) admits
///   iff its surface shadow equals the candidate's; every other slot rejects, because a
///   deferred return is opaque until per-call elaboration and so refines nothing more
///   precise than its own shadow. See
///   [ktype/parameterization-and-variance.md § Variance](../../../../design/typing/ktype/parameterization-and-variance.md#variance).
/// - Params contravariant with width-drop: every `Argument` the value declares must
///   appear in `params` (a value-required param the slot doesn't promise is a width
///   violation → `false`); for a shared name, the slot's param must be equal-or-more-
///   specific than the value's (`slot_pt == a.ktype || slot_pt ≺ a.ktype`). Extra
///   slot params the value doesn't declare are fine — under call-by-name they arrive
///   unbound (width drop), so there is no exhaustiveness check.
pub(super) fn function_compat<'v>(
    sig: &ExpressionSignature<'v>,
    params: &Record<KType>,
    ret: KType,
    registries: &RunRegistries,
) -> bool {
    let types = &registries.types;
    use crate::machine::model::types::{DeferredReturnSurface, ReturnType};
    let ret_ok = match &sig.return_type() {
        ReturnType::Resolved(kt) => *kt == ret || kt.is_more_specific_than(ret, registries),
        ReturnType::Deferred(d) => match types.node(ret) {
            TypeNode::Any => true,
            TypeNode::DeferredReturn(slot) => {
                DeferredReturnSurface::from_deferred(d, &registries.labels) == slot
            }
            _ => false,
        },
    };
    if !ret_ok {
        return false;
    }
    for el in sig.elements() {
        if let SignatureElement::Argument(a) = el {
            match params.get(a.name.symbol()) {
                None => return false,
                Some(slot_pt) => {
                    if !(*slot_pt == a.ktype || slot_pt.is_more_specific_than(a.ktype, registries))
                    {
                        return false;
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests;
