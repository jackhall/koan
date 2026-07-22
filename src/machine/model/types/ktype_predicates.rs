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
use super::sig_schema::{sig_subtype, SigSchema};
use super::signature::{ExpressionSignature, SignatureElement};
use super::type_digest::{empty_schema_digest, TypeDigest};
use crate::machine::model::ast::{ExpressionPart, KLiteral};
use crate::machine::model::values::{Carried, Held, KObject};
use crate::machine::DeliveredCarried;

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
            value_arguments.get(name).is_some_and(|value_argument| {
                *slot_argument == KType::ANY || *value_argument == *slot_argument
            })
        })
}

/// The two slot types that constrain nothing beyond "a name": a concrete type out-specifies
/// either of them.
fn is_unconstrained_name(kt: KType) -> bool {
    kt == KType::IDENTIFIER || kt == KType::of_kind(KKind::ProperType)
}

impl KType {
    /// Strict specificity ordering. Concrete types outrank `Any` and the unconstrained-name slot
    /// types (`Identifier`, `ProperType`), so an overload like `ATTR <s:NewType>` beats its
    /// `ATTR <s:Identifier>` sibling when both admit. A nominal-family kind out-specifies
    /// `OfKind(ProperType)` (`OfKind(NewType) ≺ OfKind(ProperType)`), and a sealed member
    /// out-specifies the `OfKind(kind)` of its own family. Parameterized containers are
    /// covariant in their inner slots. Returns `false` for equal types.
    ///
    /// Every handle's digest is content-derived, so the `(subject, candidate)` pair is always a
    /// sound verdict key and the walk is memoized unconditionally.
    pub fn is_more_specific_than(self, other: KType, types: &TypeRegistry) -> bool {
        let (subject, candidate) = (self.digest(), other.digest());
        if let Some(verdict) = types.verdict(subject, candidate, Relation::MoreSpecific) {
            return verdict;
        }
        let verdict = self.more_specific_walk(other, types);
        types.record_verdict(subject, candidate, Relation::MoreSpecific, verdict);
        verdict
    }

    fn more_specific_walk(self, other: KType, types: &TypeRegistry) -> bool {
        if other == KType::ANY && self != KType::ANY {
            return true;
        }
        if is_unconstrained_name(other) && !(is_unconstrained_name(self) || self == KType::ANY) {
            return true;
        }
        let other_node = types.node(other);
        match (types.node(self), other_node) {
            (TypeNode::List { element: a }, TypeNode::List { element: b }) => {
                a.is_more_specific_than(b, types)
            }
            (TypeNode::Dict { key: ka, value: va }, TypeNode::Dict { key: kb, value: vb }) => {
                let key_more = ka.is_more_specific_than(kb, types);
                let value_more = va.is_more_specific_than(vb, types);
                (key_more && (value_more || va == vb)) || (ka == kb && value_more)
            }
            // Record-value subtyping: width-superset + covariant depth (the dual of the
            // contravariant width-drop `param_record_more_specific` for function params).
            (TypeNode::Record { fields: a }, TypeNode::Record { fields: b }) => {
                record_value_more_specific(&a, &b, types)
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
            ) => param_record_more_specific(&pa, ra, &pb, rb, types),
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
                if digest_b == empty && digest_a != empty {
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
                sig_schema_more_specific(&schema_a, self.digest(), &schema_b, other.digest(), types)
            }
            // A nominal-family kind out-specifies `OfKind(ProperType)` — `OfKind(NewType) ≺
            // OfKind(ProperType)`. (Against `Identifier` / `OfKind(ProperType)` the generic rule
            // above already fires; this covers a nominal-vs-nominal-supertype tie.)
            (TypeNode::OfKind(a), TypeNode::OfKind(b)) if a.strictly_below(b) => true,
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
                && aa.keys().all(|name| ab.get(name).is_some()) =>
            {
                // Same constructor, same parameter names: compare each argument against its
                // same-named counterpart.
                let pairs = || aa.iter().map(|(name, x)| (*x, *ab.get(name).unwrap()));
                let any_more = pairs().any(|(x, y)| x.is_more_specific_than(y, types));
                let all_equal_or_more =
                    pairs().all(|(x, y)| x == y || x.is_more_specific_than(y, types));
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
                            .any(|y| x == y || x.is_more_specific_than(*y, types))
                    })
            }
            // Each member of a union is a subtype of it: a non-union `x` is more specific than
            // `Union(ms)` iff it equals or refines one of the members.
            (_, TypeNode::Union { members }) => members
                .iter()
                .any(|m| self == *m || self.is_more_specific_than(*m, types)),
            _ => false,
        }
    }

    /// True iff `carried` satisfies a slot declared as `self` — exact match or covariant
    /// refinement. A `List<Any>` value (the join an empty or heterogeneous literal
    /// memoizes) does not satisfy `:(LIST OF Number)`.
    pub fn satisfied_by(self, carried: KType, types: &TypeRegistry) -> bool {
        self == carried || carried.is_more_specific_than(self, types)
    }

    /// True iff a runtime `KObject` value satisfies this declared type.
    /// Aggregate-cell satisfaction: an `Object` cell defers to [`matches_value`]; a `Type`
    /// cell (a first-class type stored in a list/dict/record) satisfies a type-accepting
    /// slot — `Any`, an `OfKind` kind that subsumes the type's `kind_of`, or an exact type
    /// identity.
    pub fn matches_held(self, cell: &Held<'_>, types: &TypeRegistry) -> bool {
        match cell {
            Held::Object(o) => self.matches_value(o, types),
            Held::Type(t) => match types.node(self) {
                TypeNode::Any => true,
                TypeNode::OfKind(k) => k.admits(t.kind_of(types)),
                _ => self == *t,
            },
            // An aggregate cell holds a value or a resolved type; the bind seam's unlowered
            // name carrier never becomes one, so no slot classifies it.
            Held::UnresolvedType(_) => false,
        }
    }

    pub fn matches_value(self, obj: &KObject<'_>, types: &TypeRegistry) -> bool {
        match types.node(self) {
            TypeNode::Any => true,
            TypeNode::List { element } => match obj {
                KObject::List(items, _) => items.iter().all(|x| element.matches_held(x, types)),
                _ => false,
            },
            TypeNode::Dict { key, value } => match obj {
                KObject::Dict(map, _) => map.iter().all(|(map_key, held)| {
                    (key == KType::ANY || key == map_key.ktype()) && value.matches_held(held, types)
                }),
                _ => false,
            },
            // Every slot field must be present in the value and match (depth). Extra value
            // fields are fine — a wider record value is more specific than a narrower slot.
            TypeNode::Record { fields } => match obj {
                KObject::Record(substrate, _) => fields.iter().all(|(name, field_type)| {
                    substrate
                        .fields()
                        .get(name)
                        .map(|v| field_type.matches_held(v, types))
                        .unwrap_or(false)
                }),
                _ => false,
            },
            TypeNode::KFunction { params, ret } => match obj {
                KObject::KFunction(f) => function_compat(&f.signature, &params, ret, types),
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
                KObject::Module(m) => m.satisfies_sig_schema(&schema, schema_digest, types),
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
            // A stamped `type_args` carrier (from ascription) takes precedence and is
            // checked structurally per parameter name; an erased carrier falls back to
            // checking the inhabited tag's payload against the same-named argument — a tag name
            // and its carrier's parameter name agree by construction.
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => match obj {
                KObject::Tagged { tag, value, .. } => {
                    // The value's own identity is either the applied form (stamped `type_args`)
                    // or the bare member handle (erased).
                    let identity = obj.ktype();
                    match types.node(identity) {
                        TypeNode::ConstructorApply {
                            constructor: value_constructor,
                            arguments: value_arguments,
                        } => constructor_apply_admits(
                            constructor,
                            &arguments,
                            value_constructor,
                            &value_arguments,
                        ),
                        _ => {
                            if identity != constructor {
                                return false;
                            }
                            match arguments.get(tag) {
                                Some(argument) => argument.matches_value(value, types),
                                None => true,
                            }
                        }
                    }
                }
                // An identity-wrapper value (`NEWTYPE (Type AS Wrapper)`): its `type_id` is
                // itself a `ConstructorApply`. Match by the same constructor + per-argument rule
                // the stamped-`type_args` `Tagged` path uses.
                KObject::Wrapped { type_id, .. } => match types.node(*type_id) {
                    TypeNode::ConstructorApply {
                        constructor: value_constructor,
                        arguments: value_arguments,
                    } => constructor_apply_admits(
                        constructor,
                        &arguments,
                        value_constructor,
                        &value_arguments,
                    ),
                    _ => false,
                },
                _ => false,
            },
            // A union slot admits a value any of its members admits.
            TypeNode::Union { members } => members.iter().any(|m| m.matches_value(obj, types)),
            // A sealed nominal slot admits a value whose `ktype()` reports the same member
            // handle — a per-variant newtype `Wrapped` value or a `TypeConstructor` value.
            _ => self == obj.ktype(),
        }
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
        let carrier_ktype = match types.node(t) {
            TypeNode::Signature { .. } => t,
            _ => KType::of_kind(KKind::ProperType),
        };
        match types.node(self) {
            TypeNode::Any => true,
            TypeNode::Signature { .. } => false,
            TypeNode::OfKind(k) => k.admits(t.kind_of(types)),
            // A union slot is satisfied by any type its members are satisfied by.
            TypeNode::Union { members } => members.iter().any(|m| m.matches_type(t, types)),
            _ => self == carrier_ktype,
        }
    }

    /// Per-value admissibility for a resolved [`Carried`] argument — the classifier the spliced
    /// arms of [`accepts_part`] delegate to, and what a spliced cell opens against at its own brand.
    /// The slot is a handle and the value (`'v`) is a region borrow, so every comparison is a
    /// verdict-only structural check, none of which needs the value's own lifetime.
    /// "Dispatch trusts the carried element type": a container's memoized carried `KType` is read
    /// via `satisfied_by`, never by walking its contents.
    pub fn accepts_carried<'v>(self, c: Carried<'v>, types: &TypeRegistry) -> bool {
        match types.node(self) {
            TypeNode::Any => true,
            TypeNode::Number => matches!(c, Carried::Object(KObject::Number(_))),
            TypeNode::Str => matches!(c, Carried::Object(KObject::KString(_))),
            TypeNode::Bool => matches!(c, Carried::Object(KObject::Bool(_))),
            TypeNode::Null => matches!(c, Carried::Object(KObject::Null)),
            // Evaluated container: the value already memoizes its own full container-type
            // handle, so the check is one `satisfied_by` between two handles — no element walk,
            // and nothing to rebuild or re-digest.
            TypeNode::List { .. } => match c {
                Carried::Object(KObject::List(_, carried)) => self.satisfied_by(*carried, types),
                _ => false,
            },
            TypeNode::Dict { .. } => match c {
                Carried::Object(KObject::Dict(_, carried)) => self.satisfied_by(*carried, types),
                _ => false,
            },
            TypeNode::Record { .. } => match c {
                Carried::Object(KObject::Record(_, carried)) => self.satisfied_by(*carried, types),
                _ => false,
            },
            TypeNode::KFunction { params, ret } => match c {
                Carried::Object(KObject::KFunction(f)) => {
                    function_compat(&f.signature, &params, ret, types)
                }
                _ => false,
            },
            // Part-shape-only slots (identifier / expression / type-expr / record-type) admit a
            // parser part shape, never a resolved value.
            TypeNode::Identifier
            | TypeNode::KExpression
            | TypeNode::SigiledTypeExpr
            | TypeNode::RecordType => false,
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
            TypeNode::Union { members } => members.iter().any(|m| m.accepts_carried(c, types)),
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
                    m.satisfies_sig_schema(&schema, schema_digest, types)
                }
                _ => false,
            },
            // A sibling reference is meaningful only inside its pre-seal window and never
            // reaches a real argument slot.
            TypeNode::Sibling(_) => false,
            // A whole-group handle names a group of types, not a value type — it admits no
            // argument.
            TypeNode::Group { .. } => false,
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
                Carried::Object(obj) => match types.node(obj.ktype()) {
                    TypeNode::ConstructorApply {
                        constructor: value_constructor,
                        arguments: value_arguments,
                    } => constructor_apply_admits(
                        slot_constructor,
                        &slot_arguments,
                        value_constructor,
                        &value_arguments,
                    ),
                    _ => false,
                },
            },
        }
    }

    /// Classify a spliced **cell** against this slot without adopting it — opens the delivery
    /// envelope at a fresh brand under its retained host pin and routes the opened value through
    /// [`accepts_carried`](Self::accepts_carried) at that brand. No cast: the slot is a handle,
    /// so it carries no brand of its own for the opened value's brand to relate to — a
    /// verdict-only walk needs no re-anchoring. The picker may reject the candidate, so this
    /// deliberately does not adopt.
    pub(crate) fn accepts_cell(self, cell: &DeliveredCarried, types: &TypeRegistry) -> bool {
        cell.open(|c| self.accepts_carried(c, types))
    }

    /// Per-`ExpressionPart` admissibility for argument slots. Unevaluated container
    /// literals admit shape-only (element types unknown until evaluation); a spliced cell
    /// ([`ExpressionPart::Spliced`]) classifies through [`accepts_cell`](Self::accepts_cell),
    /// which opens it at its own brand. Non-satisfying containers fall through the scope walk
    /// rather than failing the bind.
    pub fn accepts_part(self, part: &ExpressionPart<'_>, types: &TypeRegistry) -> bool {
        // A spliced cell opens at its own brand through `accepts_cell`, which routes the opened
        // value through `accepts_carried` — no cast. Every remaining arm is a shape check on the
        // parser part, so no coercion of `part` is needed.
        if let ExpressionPart::Spliced { cell } = part {
            return self.accepts_cell(cell, types);
        }
        match types.node(self) {
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
            // A `:KExpression` slot captures a parenthesized expression raw, and a `#(...)` quote —
            // whose body is already data — with it. It also captures a bare list literal raw, the
            // shape a `Unary`-mode operator run reduces to (`[Keyword, ListLiteral]`), so the
            // receiving builtin owns the operand run.
            TypeNode::KExpression => matches!(
                part,
                ExpressionPart::Expression(_)
                    | ExpressionPart::QuotedExpression(_)
                    | ExpressionPart::ListLiteral(_)
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
            // A whole-group handle names a group of types, not a value type — it admits no
            // argument; the `RECURSIVE TYPES` group name is a reserved value-language seam.
            TypeNode::Group { .. } => false,
            // Confined to a synthesized FN `ret` slot — never a free-standing argument slot.
            TypeNode::DeferredReturn(_) => false,
        }
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
    types: &TypeRegistry,
) -> bool {
    let forward_hit = types.verdict(digest_a, digest_b, Relation::SigSatisfies);
    let reverse_hit = types.verdict(digest_b, digest_a, Relation::SigSatisfies);
    if let (Some(forward), Some(reverse)) = (forward_hit, reverse_hit) {
        return forward && !reverse;
    }
    let forward = forward_hit.unwrap_or_else(|| {
        let verdict = sig_subtype(a, b, types).is_ok();
        types.record_verdict(digest_a, digest_b, Relation::SigSatisfies, verdict);
        verdict
    });
    let reverse = reverse_hit.unwrap_or_else(|| {
        let verdict = sig_subtype(b, a, types).is_ok();
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
    types: &TypeRegistry,
) -> bool {
    if !pa.keys().all(|k| pb.get(k).is_some()) {
        return false;
    }
    let params_ok = pa.iter().all(|(name, s)| {
        let o = *pb.get(name).unwrap();
        o == *s || o.is_more_specific_than(*s, types)
    });
    let params_more = pa.keys().any(|k| {
        pb.get(k)
            .unwrap()
            .is_more_specific_than(*pa.get(k).unwrap(), types)
    });
    let ret_more = ra.is_more_specific_than(rb, types);
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
fn record_value_more_specific(a: &Record<KType>, b: &Record<KType>, types: &TypeRegistry) -> bool {
    if !b.keys().all(|k| a.get(k).is_some()) {
        return false;
    }
    let depth_ok = b.iter().all(|(name, bt)| {
        let at = *a.get(name).unwrap();
        at == *bt || at.is_more_specific_than(*bt, types)
    });
    let depth_more = b.keys().any(|k| {
        a.get(k)
            .unwrap()
            .is_more_specific_than(*b.get(k).unwrap(), types)
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
    types: &TypeRegistry,
) -> bool {
    use crate::machine::model::types::{DeferredReturnSurface, ReturnType};
    let ret_ok = match &sig.return_type {
        ReturnType::Resolved(kt) => *kt == ret || kt.is_more_specific_than(ret, types),
        ReturnType::Deferred(d) => match types.node(ret) {
            TypeNode::Any => true,
            TypeNode::DeferredReturn(slot) => DeferredReturnSurface::from_deferred(d) == slot,
            _ => false,
        },
    };
    if !ret_ok {
        return false;
    }
    for el in &sig.elements {
        if let SignatureElement::Argument(a) = el {
            match params.get(&a.name) {
                None => return false,
                Some(slot_pt) => {
                    if !(*slot_pt == a.ktype || slot_pt.is_more_specific_than(a.ktype, types)) {
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
