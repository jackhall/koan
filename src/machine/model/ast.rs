//! The raw AST: what the parser produces and what a function body, a quote, and a stored signature
//! hold. Every node borrows its storage — parts, keyword text, and the structural cache alike — so a
//! node is `Copy`, `Drop`-free, and copying one to another region is a slice copy rather than a
//! rebuild.
//!
//! The scheduler's own per-call form is [`WorkingExpression`], a distinct type in [`working`]. A
//! resolved sub-result and a staging hole live only there, which is what keeps this type
//! structurally splice-free: an AST node names no producer region, so nothing here has a reach to
//! describe.

use crate::source::{FileId, Span, Spanned};

use crate::machine::core::RegionBrand;
use crate::machine::model::{Held, KObject, Parseable, StoredBinderKey};
use crate::machine::model::{StoredElement, UntypedElement, UntypedKey};
use crate::witnessed::reattachable;

mod shape;
pub mod working;

pub use shape::{
    classify_dispatch_shape, operator_probe_for, stored_untyped_key, DispatchShape, FieldSlot,
    Part, PartClass,
};
pub use working::{WorkingExpression, WorkingPart};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KLiteral<'a> {
    Number(f64),
    String(&'a str),
    Boolean(bool),
    Null,
}

impl<'a> KLiteral<'a> {
    /// The [`KObject`] this literal denotes, built into `brand`'s region: the scalar arms own their
    /// data outright, and a string literal's bytes are bumped into that region
    /// ([`RegionBrand::alloc_text`]). Generic in `'b` (rather than `'a`) despite [`KObject`] being
    /// invariant in its lifetime — the value is constructible at whatever lifetime the brand
    /// carries, in particular a construction site's fold brand.
    ///
    /// Region-pure in the sense the construction sites need: every borrow the product carries points
    /// into `brand`'s own region, so a fold door stores it with no audit at all.
    pub fn to_kobject<'b>(&self, brand: RegionBrand<'b>) -> KObject<'b> {
        match self {
            KLiteral::Number(n) => KObject::Number(*n),
            KLiteral::String(s) => KObject::KString(brand.alloc_text(s)),
            KLiteral::Boolean(b) => KObject::Bool(*b),
            KLiteral::Null => KObject::Null,
        }
    }
}

/// A bare type identifier as written in source (`Number`, `Point`, `Mo.Ty`) — a single name
/// token, never compound syntax.
///
/// A thin borrow of the source name: `Deref`s to `str`, derives eq/hash by string. The
/// identifier stays a flat name even when it *denotes* a compound type (a `NEWTYPE` / `UNION`
/// name resolves to a record / tagged type); compound *syntax* (`:(LIST OF …)`, `:(FN … -> …)`)
/// is a `SigiledTypeExpr`, not a `TypeIdentifier`. The position tag rides on the carrier variant
/// (`ExpressionPart::Type`), not on this struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeIdentifier<'a>(&'a str);

impl<'a> std::ops::Deref for TypeIdentifier<'a> {
    type Target = str;
    fn deref(&self) -> &str {
        self.0
    }
}

impl<'a> TypeIdentifier<'a> {
    /// Name a leaf type from text already resident at `'a` — a parser token bumped into program
    /// storage, or a name bumped through [`RegionBrand::alloc_text`] at a construction site.
    pub fn leaf(name: &'a str) -> TypeIdentifier<'a> {
        TypeIdentifier(name)
    }

    pub fn as_str(&self) -> &'a str {
        self.0
    }

    /// Render in surface syntax so the output round-trips through the parser unchanged.
    pub fn render(&self) -> String {
        self.0.to_string()
    }
}

/// One element of a parsed expression. Every arm borrows at `'a`: a name is a run of bytes in the
/// storage that parsed it, a nested node is a pointer to a sibling node in the same storage, and a
/// literal run is a bumped slice.
#[derive(Debug, Clone, Copy)]
pub enum ExpressionPart<'a> {
    Keyword(&'a str),
    Identifier(&'a str),
    Type(TypeIdentifier<'a>),
    Expression(&'a KExpression<'a>),
    /// Parse-context marker for a `:(...)` group: the wrapped `KExpression` must dispatch
    /// in type-context, returning a type-side carrier. Shape recognition is the
    /// dispatcher's responsibility — the parser does no folding here. See
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
    SigiledTypeExpr(&'a KExpression<'a>),
    /// First-class record type `:{x :Number, y :Str}`. The nested `KExpression` is the
    /// field-list `(x :Number, y :Str)` — the same `<name> :<Type>` pair shape a SIG member
    /// or FN parameter list uses. Unlike `SigiledTypeExpr`, this is matched
    /// structurally (the elaborator folds it straight to `KType::Record`); there is no
    /// internal type-constructor builtin behind it. See
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
    RecordType(&'a KExpression<'a>),
    ListLiteral(&'a [ExpressionPart<'a>]),
    DictLiteral(&'a [(ExpressionPart<'a>, ExpressionPart<'a>)]),
    /// Anonymous record literal (`{x = 1, y = "a"}`) — identifier-keyed `=` pairs. The
    /// brace frame routes here when the first pair separator is `=`; `:` pairs stay a
    /// `DictLiteral`. Field names are syntactic identifiers (never name-resolved).
    RecordLiteral(&'a [(&'a str, ExpressionPart<'a>)]),
    Literal(KLiteral<'a>),
    /// A `#(...)` quote: the parenthesized body captured at parse time as data. The parser folds
    /// the sigil and its group into this part, so quoting is static syntax — there is no runtime
    /// quoting operation and the body never dispatches. Behaves as a literal everywhere: it is a
    /// `Slot` in the untyped key, a single one classifies [`DispatchShape::LiteralPassThrough`],
    /// and it resolves to `KObject::KExpression(<body>)` — the value `$(...)` evaluates. See
    /// [design/expressions-and-parsing.md](../../../design/expressions-and-parsing.md).
    QuotedExpression(&'a KExpression<'a>),
}

impl<'a> Part<'a> for ExpressionPart<'a> {
    fn class(&self) -> PartClass<'a> {
        match self {
            ExpressionPart::Keyword(s) => PartClass::Keyword(s),
            ExpressionPart::Identifier(_) => PartClass::Identifier,
            ExpressionPart::Type(_) => PartClass::Type,
            ExpressionPart::Expression(_) => PartClass::Expression,
            ExpressionPart::SigiledTypeExpr(_) => PartClass::SigiledTypeExpr,
            ExpressionPart::RecordType(_) => PartClass::RecordType,
            ExpressionPart::ListLiteral(_) => PartClass::ListLiteral,
            ExpressionPart::DictLiteral(_) => PartClass::DictLiteral,
            ExpressionPart::RecordLiteral(_) => PartClass::RecordLiteral,
            ExpressionPart::Literal(_) => PartClass::Literal,
            ExpressionPart::QuotedExpression(_) => PartClass::QuotedExpression,
        }
    }

    fn field_slot(&self) -> FieldSlot<'a> {
        match self {
            ExpressionPart::Identifier(s) => FieldSlot::Name(s),
            ExpressionPart::Type(t) => FieldSlot::Type(*t),
            ExpressionPart::SigiledTypeExpr(body) => FieldSlot::AstSigil(body),
            ExpressionPart::RecordType(body) => FieldSlot::AstRecord(body),
            _ => FieldSlot::Other,
        }
    }

    fn summarize(&self) -> String {
        ExpressionPart::summarize(self)
    }
}

impl<'a> ExpressionPart<'a> {
    /// Wrap a run of parts as a nested `Expression` part, bumping both the run and the node into
    /// `brand`'s region.
    pub fn expression(
        brand: RegionBrand<'a>,
        parts: Vec<Spanned<ExpressionPart<'a>>>,
    ) -> ExpressionPart<'a> {
        ExpressionPart::Expression(KExpression::nested(brand, parts))
    }

    /// Per-part subset of [`KExpression::summarize`].
    pub fn summarize(&self) -> String {
        match self {
            ExpressionPart::Keyword(s) => (*s).to_string(),
            ExpressionPart::Identifier(s) => (*s).to_string(),
            ExpressionPart::Type(t) => t.render(),
            ExpressionPart::Expression(e) => e.summarize(),
            ExpressionPart::SigiledTypeExpr(e) => format!(":({})", e.summarize()),
            ExpressionPart::RecordType(e) => format!(":{{{}}}", e.summarize()),
            ExpressionPart::QuotedExpression(e) => format!("#({})", e.summarize()),
            ExpressionPart::ListLiteral(items) => {
                let inner: Vec<String> = items.iter().map(|p| p.summarize()).collect();
                format!("[{}]", inner.join(" "))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.summarize(), v.summarize()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            ExpressionPart::RecordLiteral(pairs) => {
                let inner: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{} = {}", k, v.summarize()))
                    .collect();
                format!("{{{}}}", inner.join(", "))
            }
            ExpressionPart::Literal(lit) => match lit {
                KLiteral::Number(n) => n.to_string(),
                KLiteral::String(s) => (*s).to_string(),
                KLiteral::Boolean(b) => b.to_string(),
                KLiteral::Null => "null".to_string(),
            },
        }
    }

    /// Slot-aware resolve producing an owned [`Held`] cell, run at [`KFunction::bind_args`] time. A
    /// type rides the `Type` arm; a runtime value rides the `Object` arm. A `Type`-name token in a
    /// proper-type slot lowers via [`KType::from_type_identifier`], falling back to the
    /// [`Held::UnresolvedType`] carrier for a bare user name — no type handle ever denotes an
    /// unresolved name, so the surface [`TypeIdentifier`] rides through verbatim and scope-aware
    /// elaboration defers to
    /// [`Scope::resolve_type_identifier`](crate::machine::core::Scope::resolve_type_identifier).
    ///
    /// [`KFunction::bind_args`]: crate::machine::KFunction::bind_args
    /// [`KType::from_type_identifier`]: crate::machine::model::KType::from_type_identifier
    pub fn resolve_for(
        &self,
        slot: &crate::machine::model::KType,
        scope: &'a crate::machine::core::Scope<'a>,
        types: &crate::machine::model::types::TypeRegistry,
    ) -> Held<'a> {
        use crate::machine::model::types::KType;
        if let (ExpressionPart::Type(t), KType::PROPER_TYPE | KType::ANY_TYPE) = (self, *slot) {
            return match KType::from_type_identifier(t, types) {
                Ok(kt) => Held::Type(kt),
                Err(_) => Held::UnresolvedType(*t),
            };
        }
        if let (ExpressionPart::SigiledTypeExpr(inner), KType::SIGILED_TYPE_EXPR) = (self, *slot) {
            return Held::Object(KObject::KExpression(**inner));
        }
        if let (ExpressionPart::RecordType(inner), KType::RECORD_TYPE) = (self, *slot) {
            return Held::Object(KObject::KExpression(**inner));
        }
        Held::Object(self.resolve(scope.brand()))
    }

    /// The [`KObject`] this part denotes, built into `brand`'s region — the string arms bump their
    /// bytes there, so the product is dest-resident and holds no allocation of its own.
    pub fn resolve(&self, brand: RegionBrand<'a>) -> KObject<'a> {
        match self {
            ExpressionPart::Keyword(s) => KObject::KString(brand.alloc_text(s)),
            ExpressionPart::Identifier(s) => KObject::KString(brand.alloc_text(s)),
            ExpressionPart::Type(t) => KObject::KString(brand.alloc_text(t.as_str())),
            ExpressionPart::Literal(KLiteral::Number(n)) => KObject::Number(*n),
            ExpressionPart::Literal(KLiteral::String(s)) => KObject::KString(brand.alloc_text(s)),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => KObject::Bool(*b),
            ExpressionPart::Literal(KLiteral::Null) => KObject::Null,
            ExpressionPart::Expression(e) => KObject::KExpression(**e),
            // A quote denotes its body as data — the same `KObject` an `Expression` part in a
            // `:KExpression` slot denotes, reached from any slot a literal reaches.
            ExpressionPart::QuotedExpression(e) => KObject::KExpression(**e),
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
    /// of [`resolve`](Self::resolve) for static-cell sites that fold. The region-pure variants
    /// (keyword, bare identifier, type name, literal) reach nothing outside `brand`'s own region, so
    /// the value is constructible at the caller's fold brand, where [`resolve`](Self::resolve)'s
    /// invariant `KObject<'a>` cannot go. Borrow-bearing variants are classified to owned
    /// sub-dispatches before any static cell, so they never reach here.
    pub fn resolve_region_pure<'b>(&self, brand: RegionBrand<'b>) -> KObject<'b> {
        match self {
            ExpressionPart::Keyword(s) | ExpressionPart::Identifier(s) => {
                KObject::KString(brand.alloc_text(s))
            }
            ExpressionPart::Type(t) => KObject::KString(brand.alloc_text(t.as_str())),
            ExpressionPart::Literal(lit) => lit.to_kobject(brand),
            // A quote's `KObject::KExpression` is invariant in `'a` with no `'static` rebuild, so it
            // cannot be constructed at the caller's `yoke` brand — the classifier routes a quote to
            // its own sub-dispatch (which seals it through the expression door) before any static cell.
            ExpressionPart::Expression(_)
            | ExpressionPart::SigiledTypeExpr(_)
            | ExpressionPart::RecordType(_)
            | ExpressionPart::QuotedExpression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => unreachable!(
                "resolve_region_pure is only called on a region-pure static-cell part \
                 (keyword / bare identifier / type name / literal); borrow-bearing parts are \
                 classified to owned sub-dispatches before any static cell"
            ),
        }
    }
}

/// The parse-time binder plan for a node that is *itself* a binder: the channel it installs
/// ([`StoredBinderKey`]) and the chain-slot mask marking which of its slots carry nested binders
/// forward. Cached on [`KExpression`] beside [`DispatchShape`]; `None` for a non-binder node.
#[derive(Clone, Copy, Debug)]
pub struct BinderPlan<'a> {
    pub key: StoredBinderKey<'a>,
    pub chain_slot_mask: &'static [bool],
}

/// A parsed Koan expression: an ordered run of [`ExpressionPart`]s borrowed from the storage that
/// parsed them.
///
/// `span` and `file` are `None` for hand-built ASTs.
///
/// `untyped_key`, `shape`, and `operator_probe` are a structural cache filled by the construction
/// doors once the parts run is complete, so the dispatch driver reads the cache rather than
/// re-deriving on every call of the enclosing function. `binder_plan` and `binder_installs` are the
/// binder-position cache; a binder is always a parsed statement, so only this node carries them —
/// the scheduler's [`WorkingExpression`] installs nothing.
///
/// Every field is a shared borrow at `'a` or a `Copy` handle, so the node is covariant in `'a`: a
/// program-storage node flows into shorter-lived code by ordinary subtyping, with no reattach and no
/// witness.
#[derive(Clone, Copy)]
pub struct KExpression<'a> {
    pub parts: &'a [Spanned<ExpressionPart<'a>>],
    pub span: Option<Span>,
    pub file: Option<FileId>,
    untyped_key: &'a [StoredElement<'a>],
    shape: DispatchShape,
    operator_probe: Option<&'a str>,
    binder_plan: Option<&'a BinderPlan<'a>>,
    binder_installs: &'a [StoredBinderKey<'a>],
}

// Lifetimes do not affect layout, so this retype is a no-op transmute. The witness's `'b: 'w` bound
// is what makes a reattach a shortening; nothing here weakens it.
reattachable! { KExpression<'static> => KExpression<'r> }

impl<'a> KExpression<'a> {
    /// Spanless construction door; `span`/`file` populated by later phases.
    pub fn new(brand: RegionBrand<'a>, parts: Vec<Spanned<ExpressionPart<'a>>>) -> Self {
        Self::build(brand, parts, None, None)
    }

    /// Construction chokepoint: bumps the parts run into `brand`'s region and fills the structural
    /// cache from it. Every node routes here, so none ships with a stale or unfilled cache and no
    /// part run is mutated after it is frozen.
    pub fn build(
        brand: RegionBrand<'a>,
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        let parts = brand.alloc_slice(&parts);
        let untyped_key = stored_untyped_key(brand, parts);
        let shape = classify_dispatch_shape(parts);
        let mut expression = KExpression {
            parts,
            span,
            file,
            untyped_key,
            shape,
            operator_probe: operator_probe_for(brand, parts, shape),
            binder_plan: None,
            binder_installs: &[],
        };
        expression.binder_plan = crate::machine::model::binder::binder_plan_for(brand, &expression)
            .map(|(key, chain_slot_mask)| {
                brand.alloc_value(BinderPlan {
                    key,
                    chain_slot_mask,
                })
            });
        expression.binder_installs = expression.compute_binder_installs(brand);
        expression
    }

    /// Build a node and bump it, for a part arm that nests one ([`ExpressionPart::Expression`] and
    /// its sigil siblings hold `&'a KExpression<'a>`).
    pub fn nested(
        brand: RegionBrand<'a>,
        parts: Vec<Spanned<ExpressionPart<'a>>>,
    ) -> &'a KExpression<'a> {
        brand.alloc_value(Self::new(brand, parts))
    }

    /// Aggregate the binder installs of this node's subtree, per the position rule: this node's own
    /// key (when a binder) plus, transitively, the installs of its chain-slot children and of a
    /// redundant single-`Expression` paren wrapper. Children are always built before parents (parse
    /// and every construction door build bottom-up), so each child's cache is already filled and
    /// this is a plain read. Aggregation never crosses keyword/identifier/type/literal parts,
    /// quotes, sigils, list/dict/record literals, lazy (`:KExpression`) slots, or block-shaped
    /// children.
    fn compute_binder_installs(&self, brand: RegionBrand<'a>) -> &'a [StoredBinderKey<'a>] {
        let mut installs: Vec<StoredBinderKey<'a>> = Vec::new();
        if let Some(plan) = self.binder_plan {
            installs.push(plan.key);
            for (index, part) in self.parts.iter().enumerate() {
                if !plan.chain_slot_mask.get(index).copied().unwrap_or(false) {
                    continue;
                }
                if let ExpressionPart::Expression(child) = &part.value {
                    if !child.is_statement_block() {
                        installs.extend_from_slice(child.binder_installs);
                    }
                }
            }
        }
        // Redundant single-`Expression` paren wrapper (`((…))`) passes its child's aggregate
        // straight through — a pointer copy, the child's slice already being resident. A binder is
        // always keyword-led, so this never co-occurs with the binder-plan branch above.
        if let [only] = self.parts {
            if let ExpressionPart::Expression(child) = &only.value {
                return child.binder_installs;
            }
        }
        brand.alloc_slice(&installs)
    }

    /// This node's own binder plan — `Some` iff this node is itself a binder.
    pub fn binder_plan(&self) -> Option<&'a BinderPlan<'a>> {
        self.binder_plan
    }

    /// Everything this node's subtree installs into the enclosing scope (see the field docs).
    pub fn binder_installs(&self) -> &'a [StoredBinderKey<'a>] {
        self.binder_installs
    }

    /// True when this expression is a statement block: two or more parts, all of them
    /// `Expression`. The single definition the body splitters ([`split_body_statements`] /
    /// [`body_statement_refs`]) and the binder-install aggregation share, so the multi-statement
    /// cutoff is stated once.
    ///
    /// [`split_body_statements`]: crate::machine::split_body_statements
    /// [`body_statement_refs`]: crate::machine::body_statement_refs
    pub fn is_statement_block(&self) -> bool {
        self.parts.len() >= 2
            && self
                .parts
                .iter()
                .all(|part| matches!(part.value, ExpressionPart::Expression(_)))
    }

    /// Cached dispatch shape (see [`classify_dispatch_shape`]).
    pub fn shape(&self) -> DispatchShape {
        self.shape
    }

    /// Cached operator-registry probe key: `Some` only for an `OperatorChain`, holding
    /// the sorted-joined unique operator keywords.
    pub fn operator_probe(&self) -> Option<&'a str> {
        self.operator_probe
    }

    /// The stored bucket key, as a borrow of the run bumped at construction.
    pub fn stored_key(&self) -> &'a [StoredElement<'a>] {
        self.untyped_key
    }

    /// Bucket key: `Keyword` parts contribute `Keyword(s)`; every other variant contributes
    /// `Slot`. Must agree with `ExpressionSignature::untyped_key` for any signature that
    /// should match. Materializes the owned key the bucket tables are keyed by from the stored run.
    pub fn untyped_key(&self) -> UntypedKey {
        self.untyped_key
            .iter()
            .map(|element| match element {
                StoredElement::Keyword(s) => UntypedElement::Keyword((*s).to_string()),
                StoredElement::Slot => UntypedElement::Slot,
            })
            .collect()
    }

    /// Binder-name extractor for typed-binder builtins (`SIG <Name> = …`, `UNION <Name> = …`):
    /// if `parts[1]` is a single `Type(t)`, returns its bare name; `None` on shape
    /// mismatch. The builtin body surfaces the structured error.
    pub fn binder_name_from_type_part(&self) -> Option<&'a str> {
        match &self.parts.get(1)?.value {
            ExpressionPart::Type(t) => Some(t.as_str()),
            _ => None,
        }
    }

    /// If every part is `Expression(_)`, return the inner expressions; otherwise `None`. The
    /// returned `Vec` encodes the all-`Expression` shape — callers iterate the nodes directly
    /// without re-matching the variant.
    pub fn borrow_inner_expressions(&self) -> Option<Vec<KExpression<'a>>> {
        let mut out = Vec::with_capacity(self.parts.len());
        for part in self.parts {
            match &part.value {
                ExpressionPart::Expression(inner) => out.push(**inner),
                _ => return None,
            }
        }
        Some(out)
    }

    /// Right-fold counterpart of [`Self::borrow_inner_expressions`]: `(preceding, last)` with both
    /// unwrapped from [`ExpressionPart::Expression`]. On any shape mismatch returns `self` back so
    /// the caller can pass through.
    pub fn try_split_inner_expressions(
        self,
    ) -> Result<(Vec<KExpression<'a>>, KExpression<'a>), Self> {
        let Some(mut inner) = self.borrow_inner_expressions() else {
            return Err(self);
        };
        match inner.pop() {
            Some(last) => Ok((inner, last)),
            None => Err(self),
        }
    }

    /// Surface rendering of the whole expression — parts only, so no registry is needed.
    pub fn summarize(&self) -> String {
        self.parts
            .iter()
            .map(|p| p.value.summarize())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression")
            .field("parts", &self.parts)
            .finish()
    }
}

impl<'a> Parseable for KExpression<'a> {
    fn ktype(&self) -> crate::machine::model::KType {
        crate::machine::model::KType::KEXPRESSION
    }
}
