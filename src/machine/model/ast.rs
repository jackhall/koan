//! What the machine does with a parsed node: lowering a literal to a value, resolving a part to a
//! cell against a slot type, and the [`Part`] view the field-list walkers read both expression
//! families through.
//!
//! The syntax itself — [`KLiteral`], [`ExpressionPart`], [`KExpression`] and the structural cache
//! they carry — is the parser's, in [`crate::parse::ast`]. Everything here is an inherent impl on
//! one of those types, so the split costs no trait bridge and no wrapper.
//!
//! [`WorkingExpression`] is the scheduler's own per-call form, a distinct type in [`working`].

use crate::machine::model::Held;
use crate::machine::model::{KObject, Parseable, RunRegistries};
use crate::memory::RegionBrand;
use crate::parse::labels::BinderSymbol;

mod shape;
pub mod working;

pub use shape::{FieldSlot, Part, PartSummary, part_summary};
pub use working::{WorkingExpression, WorkingPart, WorkingSummary};

// Temporary: the syntax half's names, still reachable at their old path while the tree's imports
// are swept over to `crate::parse`.
pub use crate::parse::ast::{
    AstPartSummary, DispatchShape, ExpressionSummary, KeyElement, NodeCache, ProgramExpression,
    ProgramNode, UntypedKey, classify_dispatch_shape, operator_probe_for, stored_untyped_key,
};
pub use crate::parse::ast::{ExpressionPart, KExpression, KLiteral, PartClass};

#[cfg(test)]
mod tests;

impl<'a> KLiteral<'a> {
    /// The [`KObject`] this literal denotes, built into `brand`'s region: the scalar arms own their
    /// data outright, and a string literal's bytes are bumped into that region
    /// ([`RegionBrand::allocator`]). Generic in `'b` (rather than `'a`) despite [`KObject`] being
    /// invariant in its lifetime — the value is constructible at whatever lifetime the brand
    /// carries, in particular a construction site's fold brand.
    ///
    /// Region-pure in the sense the construction sites need: every borrow the product carries points
    /// into `brand`'s own region, so a fold door stores it with no audit at all.
    pub fn to_kobject<'b>(&self, brand: RegionBrand<'b>) -> KObject<'b> {
        match self {
            KLiteral::Number(n) => KObject::Number(*n),
            KLiteral::String(s) => KObject::KString(brand.allocator().text(s)),
            KLiteral::Boolean(b) => KObject::Bool(*b),
            KLiteral::Null => KObject::Null,
        }
    }
}

impl<'a> Part<'a> for ExpressionPart<'a> {
    fn class(&self) -> PartClass {
        ExpressionPart::class(self)
    }

    fn field_slot(&self) -> FieldSlot<'a> {
        match self {
            ExpressionPart::Identifier(v) => FieldSlot::Name(*v),
            ExpressionPart::Type(t) => FieldSlot::Type(*t),
            ExpressionPart::SigiledTypeExpr(body) => FieldSlot::AstSigil(body.reference()),
            ExpressionPart::RecordType(body) => FieldSlot::AstRecord(body.reference()),
            _ => FieldSlot::Other,
        }
    }

    /// The AST view renders through the interner alone, so the bundle narrows here. Parse fills
    /// that interner before a run frame exists, which is why the inherent method below keeps the
    /// narrower parameter rather than matching its working-family peer.
    fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        registries: &RunRegistries,
    ) -> std::fmt::Result {
        ExpressionPart::write_summary(self, f, &registries.labels)
    }
}

impl<'a> ExpressionPart<'a> {
    /// Slot-aware resolve producing an owned [`Held`] cell, run at [`KFunction::bind_args`] time. A
    /// type rides the `Type` arm; a runtime value rides the `Object` arm. A `Type`-name token in a
    /// proper-type slot lowers through the builtin table ([`KType::from_symbol`]), falling back to
    /// the [`Held::UnresolvedType`] carrier for every other name — no type handle ever denotes an
    /// unresolved name, so the token's symbol rides through verbatim and scope-aware
    /// elaboration defers to
    /// [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier).
    ///
    /// The name-capture slots are a separate seam: `:Identifier` and the two binder-position
    /// slots ride [`Held::Name`], carrying the class the parser assigned the part. They consult
    /// [`KType::from_symbol`] for nothing, so a binder name is never lowered and never rendered to
    /// a string to be bound.
    ///
    /// The two raw type-expression captures ride distinct carriers so a body can tell them apart:
    /// a `:(…)` sigil rides [`KObject::KExpression`], a `:{…}` record rides [`Held::RecordType`].
    ///
    /// A union carrier slot reduces to the one member that claims this part's shape
    /// ([`KType::capture_member_for`]) before the arms below run, so a union spelling of a carrier
    /// slot captures exactly as the bare member would. A part no member claims keeps the union
    /// handle and falls through to [`resolve`](Self::resolve), as an unclaimed part does at a bare
    /// slot.
    ///
    /// [`KFunction::bind_args`]: crate::machine::KFunction::bind_args
    pub fn resolve_for(
        &self,
        slot: &crate::machine::model::KType,
        scope: &'a crate::machine::core::Scope<'a>,
        types: &crate::machine::model::TypeRegistry,
    ) -> Held<'a> {
        use crate::machine::model::types::KType;
        let slot = slot.capture_member_for(self, types).unwrap_or(*slot);
        if let (ExpressionPart::Type(t), KType::PROPER_TYPE | KType::ANY_TYPE) = (self, slot) {
            return match KType::from_symbol(*t) {
                Some(kt) => Held::Type(kt),
                None => Held::UnresolvedType(*t),
            };
        }
        if let (ExpressionPart::SigiledTypeExpr(inner), KType::SIGILED_TYPE_EXPR) = (self, slot) {
            return Held::Object(KObject::KExpression(inner.expression()));
        }
        if let (ExpressionPart::RecordType(inner), KType::RECORD_TYPE) = (self, slot) {
            return Held::RecordType(*inner);
        }
        if let (ExpressionPart::Identifier(name), KType::IDENTIFIER | KType::NAME_TOKEN) =
            (self, slot)
        {
            return Held::Name(BinderSymbol::Value(*name));
        }
        if let (ExpressionPart::Type(t), KType::NAME_TOKEN | KType::TYPE_NAME_TOKEN) = (self, slot)
        {
            return Held::Name(BinderSymbol::Type(*t));
        }
        Held::Object(self.resolve(scope.brand()))
    }

    /// The [`KObject`] this part denotes, built into `brand`'s region — the string arms bump their
    /// bytes there, so the product is dest-resident and holds no allocation of its own.
    pub fn resolve(&self, brand: RegionBrand<'a>) -> KObject<'a> {
        match self {
            // A keyword part is fixed syntax, never data: a literal rejects one at parse
            // (`lower::Lower::push_part`), and dispatch consumes a fixed token positionally against
            // its bucket key rather than resolving it.
            ExpressionPart::Keyword(_) => {
                unreachable!("a keyword part is fixed syntax and never resolves to a value")
            }
            // A name part carries a symbol, not text, and never becomes a string on the way to a
            // slot. The name-capture slots are part-kind-exact — they admit a part shape and no
            // resolved cell (`accepts_part` / `accepts_carried`) — so a name reaches the bind seam
            // only through the `Held::Name` arms of `resolve_for`; a `Type` part in a type
            // *reference* slot likewise reaches only `PROPER_TYPE` / `ANY_TYPE`, taken at the top
            // of the same function. Every other slot is served by an eagerly-resolved carrier, not
            // a raw name.
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => unreachable!(
                "a name part is captured as its symbol by resolve_for, never resolved to a string"
            ),
            ExpressionPart::Literal(KLiteral::Number(n)) => KObject::Number(*n),
            ExpressionPart::Literal(KLiteral::String(s)) => {
                KObject::KString(brand.allocator().text(s))
            }
            ExpressionPart::Literal(KLiteral::Boolean(b)) => KObject::Bool(*b),
            ExpressionPart::Literal(KLiteral::Null) => KObject::Null,
            ExpressionPart::Expression(e) => KObject::KExpression(e.expression()),
            // A quote denotes its body as data — the same `KObject` an `Expression` part in a
            // `:KExpression` slot denotes, reached from any slot a literal reaches.
            ExpressionPart::QuotedExpression(e) => KObject::KExpression(e.expression()),
            // Reaches a value only through the dispatcher's type-context fast lane or sub-Dispatch,
            // both of which unwrap it; hitting `resolve()` means a builtin lost the marker.
            ExpressionPart::SigiledTypeExpr(_) => {
                unreachable!("SigiledTypeExpr only valid in type-context dispatch")
            }
            // Like SigiledTypeExpr: a record type reaches a value through the dispatcher's
            // `RecordType` fast lane or a raw `:RecordType`-slot capture, never `resolve()`.
            ExpressionPart::RecordType(_) => {
                unreachable!("RecordType only valid in type-context dispatch")
            }
            // A container literal's substrate is born only through the fold door, which `resolve()`
            // has no brand to reach — and it never needs one: eager staging
            // (`eager_shape`/`stage_eager_part`, `dispatch.rs`) routes every literal part through
            // its scheduled path (`schedule_list_literal` / `_dict_` / `_record_`) before any
            // resolve site reaches it, replacing it with a spliced cell first. Non-scalar dict keys
            // are surfaced as a structured `ShapeError` on that scheduled path, never here.
            ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => {
                unreachable!(
                    "a container-literal part is always staged (schedule_*_literal) before any \
                     resolve() site reaches it"
                )
            }
        }
    }

    /// The [`KObject`] a **region-pure** part denotes, at *any* lifetime — the lifetime-generic peer
    /// of [`resolve`](Self::resolve) for static-cell sites that fold. The region-pure variant
    /// (a literal) reaches nothing outside `brand`'s own region, so the value is constructible
    /// at the caller's fold brand, where [`resolve`](Self::resolve)'s invariant `KObject<'a>` cannot
    /// go. Borrow-bearing variants are classified to owned sub-dispatches before any static cell, so
    /// they never reach here.
    pub fn resolve_region_pure<'b>(&self, brand: RegionBrand<'b>) -> KObject<'b> {
        match self {
            ExpressionPart::Literal(lit) => lit.to_kobject(brand),
            // Fixed syntax, never data — a literal rejects a keyword at parse and dispatch
            // consumes one positionally.
            ExpressionPart::Keyword(_) => {
                unreachable!("a keyword part is fixed syntax and never resolves to a value")
            }
            // A name part in an aggregate is eagerly resolved against the scope chain
            // (`classify_aggregate_part`), so it becomes a resolved cell before any static cell
            // folds — a raw name never reaches a fold brand.
            ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => unreachable!(
                "a bare name in an aggregate is resolved to a cell before any static-cell fold"
            ),
            // A quote's `KObject::KExpression` is invariant in `'a` with no `'static` rebuild, so it
            // cannot be constructed at the caller's fold brand — the classifier routes a quote to
            // its own sub-dispatch (which seals it through the expression door) before any static cell.
            ExpressionPart::Expression(_)
            | ExpressionPart::SigiledTypeExpr(_)
            | ExpressionPart::RecordType(_)
            | ExpressionPart::QuotedExpression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => unreachable!(
                "resolve_region_pure is only called on a region-pure static-cell part \
                 (keyword / literal); borrow-bearing parts are classified to sub-dispatches \
                 before any static cell"
            ),
        }
    }
}

impl<'a> Parseable for KExpression<'a> {
    fn ktype(&self) -> crate::machine::model::KType {
        crate::machine::model::KType::KEXPRESSION
    }
}
