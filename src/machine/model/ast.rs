//! AST node types shared across the parse module.

use crate::machine::model::types::KKind;
use std::collections::HashMap;

use crate::machine::core::source::{FileId, Span, Spanned};
use std::rc::Rc;

use crate::machine::model::{
    ArgValue, Carried, KKey, KObject, Parseable, Record, Serializable, UntypedElement, UntypedKey,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum KLiteral {
    Number(f64),
    String(String),
    Boolean(bool),
    Null,
}

/// Surface representation of a bare type-name leaf (`Number`, `Point`, `T`, `Mo.Ty`).
///
/// A thin newtype over the source name: `Deref`s to `str`, derives eq/hash by string.
/// Compound shapes (`:(LIST OF X)`, `:(FN … -> …)`) are dispatch expressions, not
/// `TypeName` structure, so this carries no information the name string wouldn't. The
/// position tag rides on the carrier variant (`ExpressionPart::Type`,
/// `KObject::TypeNameRef`), not on this struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeName(String);

impl std::ops::Deref for TypeName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl TypeName {
    pub fn leaf(name: String) -> TypeName {
        TypeName(name)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Render in surface syntax so the output round-trips through the parser unchanged.
    pub fn render(&self) -> String {
        self.0.clone()
    }
}

/// One element of a parsed expression. `Future` is introduced by the scheduler when it
/// splices a completed dep's resolved value into its dependent's parts list.
pub enum ExpressionPart<'a> {
    Keyword(String),
    Identifier(String),
    Type(TypeName),
    Expression(Box<KExpression<'a>>),
    /// Parse-context marker for a `:(...)` group: the wrapped `KExpression` must dispatch
    /// in type-context, returning a type-side carrier. Shape recognition is the
    /// dispatcher's responsibility — the parser does no folding here. See
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
    SigiledTypeExpr(Box<KExpression<'a>>),
    /// First-class record type `:{x :Number, y :Str}`. The boxed `KExpression` is the
    /// field-list `(x :Number, y :Str)` — the same `<name> :<Type>` pair shape STRUCT /
    /// UNION / FN parameter lists use. Unlike `SigiledTypeExpr`, this is matched
    /// structurally (the elaborator folds it straight to `KType::Record`); there is no
    /// internal type-constructor builtin behind it. See
    /// [design/typing/type-language-via-dispatch.md](../../../design/typing/type-language-via-dispatch.md).
    RecordType(Box<KExpression<'a>>),
    ListLiteral(Vec<ExpressionPart<'a>>),
    DictLiteral(Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>),
    /// Anonymous record literal (`{x = 1, y = "a"}`) — identifier-keyed `=` pairs. The
    /// brace frame routes here when the first pair separator is `=`; `:` pairs stay a
    /// `DictLiteral`. Field names are syntactic identifiers (never name-resolved).
    RecordLiteral(Vec<(String, ExpressionPart<'a>)>),
    Literal(KLiteral),
    /// A resolved sub-result spliced back into a parent expression — the dispatch-argument
    /// carrier, in the two-arm [`Carried`] currency.
    Future(Carried<'a>),
}

impl<'a> std::fmt::Debug for ExpressionPart<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpressionPart::Keyword(s) => f.debug_tuple("Keyword").field(s).finish(),
            ExpressionPart::Identifier(s) => f.debug_tuple("Identifier").field(s).finish(),
            ExpressionPart::Type(t) => f.debug_tuple("Type").field(t).finish(),
            ExpressionPart::Expression(e) => f.debug_tuple("Expression").field(e).finish(),
            ExpressionPart::SigiledTypeExpr(e) => {
                f.debug_tuple("SigiledTypeExpr").field(e).finish()
            }
            ExpressionPart::RecordType(e) => f.debug_tuple("RecordType").field(e).finish(),
            ExpressionPart::ListLiteral(items) => {
                f.debug_tuple("ListLiteral").field(items).finish()
            }
            ExpressionPart::DictLiteral(pairs) => {
                f.debug_tuple("DictLiteral").field(pairs).finish()
            }
            ExpressionPart::RecordLiteral(pairs) => {
                f.debug_tuple("RecordLiteral").field(pairs).finish()
            }
            ExpressionPart::Literal(l) => f.debug_tuple("Literal").field(l).finish(),
            ExpressionPart::Future(obj) => write!(f, "Future({})", obj.summarize()),
        }
    }
}

impl<'a> ExpressionPart<'a> {
    pub fn expression(parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
        ExpressionPart::Expression(Box::new(KExpression::new(
            parts.into_iter().map(Spanned::bare).collect(),
        )))
    }

    /// Per-part subset of `KExpression::summarize`.
    pub fn summarize(&self) -> String {
        match self {
            ExpressionPart::Keyword(s) => s.clone(),
            ExpressionPart::Identifier(s) => s.clone(),
            ExpressionPart::Type(t) => t.render(),
            ExpressionPart::Expression(e) => e.summarize(),
            ExpressionPart::SigiledTypeExpr(e) => format!(":({})", e.summarize()),
            ExpressionPart::RecordType(e) => format!(":{{{}}}", e.summarize()),
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
                KLiteral::String(s) => s.clone(),
                KLiteral::Boolean(b) => b.to_string(),
                KLiteral::Null => "null".to_string(),
            },
            ExpressionPart::Future(obj) => obj.summarize(),
        }
    }

    /// Slot-aware resolve producing the bundle's owned [`ArgValue`]. A type rides the `Type`
    /// arm raw; a runtime value rides the `Object` arm. Runs at `KFunction::bind` time, which
    /// has no `Scope` in hand.
    ///
    /// - A `Future(Carried::Type(_))` sub-result threads its type straight into the `Type` arm.
    /// - A parser `Type`-name token into a proper-type slot lowers to a concrete `KType` via
    ///   [`KType::from_type_expr`], or to the [`KType::Unresolved`] transient for a bare user
    ///   name (a name not in the builtin table) — scope-aware elaboration defers to
    ///   [`Scope::resolve_type_expr`](crate::machine::core::Scope::resolve_type_expr).
    /// - Lazy `:(...)` / `:{…}` slots capture the inner expression raw in the `Object` arm.
    pub fn resolve_for(&self, slot: &crate::machine::model::KType<'a>) -> ArgValue<'a> {
        use crate::machine::model::types::KType;
        if let ExpressionPart::Future(Carried::Type(kt)) = self {
            return ArgValue::Type((*kt).clone());
        }
        if let (ExpressionPart::Type(t), KType::OfKind(KKind::Proper)) = (self, slot) {
            let kt =
                KType::<'a>::from_type_expr(t).unwrap_or_else(|_| KType::Unresolved(t.clone()));
            return ArgValue::Type(kt);
        }
        if let (ExpressionPart::SigiledTypeExpr(inner), KType::SigiledTypeExpr) = (self, slot) {
            return ArgValue::Object(Rc::new(KObject::KExpression((**inner).clone())));
        }
        if let (ExpressionPart::RecordType(inner), KType::RecordType) = (self, slot) {
            return ArgValue::Object(Rc::new(KObject::KExpression((**inner).clone())));
        }
        ArgValue::Object(Rc::new(self.resolve()))
    }

    pub fn resolve(&self) -> KObject<'a> {
        match self {
            ExpressionPart::Keyword(s) => KObject::KString(s.clone()),
            ExpressionPart::Identifier(s) => KObject::KString(s.clone()),
            ExpressionPart::Type(t) => KObject::KString(t.render()),
            ExpressionPart::Literal(KLiteral::Number(n)) => KObject::Number(*n),
            ExpressionPart::Literal(KLiteral::String(s)) => KObject::KString(s.clone()),
            ExpressionPart::Literal(KLiteral::Boolean(b)) => KObject::Bool(*b),
            ExpressionPart::Literal(KLiteral::Null) => KObject::Null,
            ExpressionPart::Expression(e) => KObject::KExpression((**e).clone()),
            // Every SigiledTypeExpr must reach a value either through the dispatcher's
            // fast lane or via sub-Dispatch — both unwrap it preserving the type-context
            // marker. Reaching `resolve()` means a builtin grabbed the raw part and lost
            // that marker.
            ExpressionPart::SigiledTypeExpr(_) => {
                unreachable!("SigiledTypeExpr only valid in type-context dispatch")
            }
            // Like SigiledTypeExpr: a record type reaches a value through the dispatcher's
            // `RecordType` fast lane or a raw `:RecordType`-slot capture, never `resolve()`.
            ExpressionPart::RecordType(_) => {
                unreachable!("RecordType only valid in type-context dispatch")
            }
            ExpressionPart::ListLiteral(items) => {
                KObject::list(items.iter().map(|p| p.resolve()).collect())
            }
            // Non-scalar keys reaching here are a scheduler bug — it must surface them as
            // a structured `ShapeError` before resolve.
            ExpressionPart::DictLiteral(pairs) => {
                let mut map: HashMap<Box<dyn Serializable<'a> + 'a>, KObject<'a>> = HashMap::new();
                for (k, v) in pairs {
                    let key_obj = k.resolve();
                    let kkey = KKey::try_from_kobject(&key_obj).unwrap_or_else(|e| {
                        panic!("DictLiteral::resolve = non-scalar key reached resolve(): {e}")
                    });
                    map.insert(Box::new(kkey), v.resolve());
                }
                KObject::dict(map)
            }
            ExpressionPart::RecordLiteral(pairs) => {
                let fields: Record<KObject<'a>> = pairs
                    .iter()
                    .map(|(name, v)| (name.clone(), v.resolve()))
                    .collect();
                KObject::record(fields)
            }
            // Deep-clone, don't stringify: a Future-borne List or KExpression must
            // materialize back to its structured form. A value-position Future is the
            // `Object` arm; a type arm never reaches `resolve` (it flows the type channel).
            ExpressionPart::Future(c) => c.object().deep_clone(),
        }
    }
}

impl<'a> Clone for ExpressionPart<'a> {
    fn clone(&self) -> Self {
        match self {
            ExpressionPart::Keyword(s) => ExpressionPart::Keyword(s.clone()),
            ExpressionPart::Identifier(s) => ExpressionPart::Identifier(s.clone()),
            ExpressionPart::Type(t) => ExpressionPart::Type(t.clone()),
            ExpressionPart::Expression(e) => ExpressionPart::Expression(e.clone()),
            ExpressionPart::SigiledTypeExpr(e) => ExpressionPart::SigiledTypeExpr(e.clone()),
            ExpressionPart::RecordType(e) => ExpressionPart::RecordType(e.clone()),
            ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(items.clone()),
            ExpressionPart::DictLiteral(pairs) => ExpressionPart::DictLiteral(pairs.clone()),
            ExpressionPart::RecordLiteral(pairs) => ExpressionPart::RecordLiteral(pairs.clone()),
            ExpressionPart::Literal(l) => ExpressionPart::Literal(l.clone()),
            ExpressionPart::Future(o) => ExpressionPart::Future(*o),
        }
    }
}

impl<'a> Clone for KExpression<'a> {
    fn clone(&self) -> Self {
        KExpression {
            parts: self.parts.clone(),
            span: self.span,
            file: self.file,
            untyped_key: self.untyped_key.clone(),
            shape: self.shape,
            operator_probe: self.operator_probe.clone(),
        }
    }
}

/// Pure-structural classification of a `KExpression` into the no-keyword fast-lane
/// shapes, the chainable operator shape, and the catch-all keyword-bearing shape.
///
/// A function of expression structure only (no scope, no types), so it is computed
/// once when the parts vector is complete and cached on [`KExpression::shape`]. The
/// dispatch driver reads the cache rather than re-deriving per call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchShape {
    BareIdentifier,
    BareTypeLeaf,
    /// Bare-`Type`-head call: head is a leaf `Type` and `parts[1..]` is non-empty.
    /// Resolves the name synchronously and branches into a type construction or a
    /// functor application via the shared apply-a-callable tail.
    TypeCall,
    /// Function-value call: head is a lowercase `Identifier`, followed by ≥1
    /// non-keyword parts.
    FunctionValueCall,
    /// Single-part `:(...)` sigiled type-expression wrapper.
    SigiledTypeExpr,
    /// Single-part `:{…}` record-type sigil. The handler folds the field list straight
    /// to `KType::Record` (deferring through a Combine when a field type sub-dispatches
    /// or forward-references), with no internal type-constructor builtin behind it.
    RecordType,
    /// Single-part literal-shaped expression — `Literal`, `Future`, nested
    /// `Expression`, `ListLiteral`, `DictLiteral`, or `RecordLiteral`. Surfaces the
    /// inner value without a bucket lookup.
    LiteralPassThrough,
    /// Chainable operator run: a slot-led key whose keywords alternate with slots,
    /// with two or more keyword positions (`Slot (Keyword Slot)+`, first keyword at
    /// index 1). A refinement of `Keyworded`: nothing else produces that shape, so it
    /// carves a track that the fold pre-pass folds into nested binary sub-dispatches.
    OperatorChain,
    /// Head-deferred call: head is a nested `Expression` followed by ≥1 non-keyword
    /// parts. The head is evaluated first; its resulting value (a function, functor,
    /// or constructible type) is then applied to `parts[1..]` via the shared
    /// apply-a-callable tail.
    HeadDeferred,
    /// Type-position head-deferred call: head is a `:(...)` sigiled type expression
    /// followed by ≥1 non-keyword parts. Like `HeadDeferred`, but the resumed value
    /// is admitted only when it is type-shaped (a constructible type or a functor);
    /// a plain function or other value surfaces a type-shaped `TypeMismatch`.
    TypeHeadDeferred,
    /// A keyword appears anywhere in `expr.parts` (and the chain shape did not match).
    Keyworded,
    /// Head is a non-callable surface — a literal, list, dict, or record — in a
    /// multi-part expression. Heads are always eager and must resolve to something
    /// callable; this shape surfaces a loud `DispatchFailed` from the dispatch entry.
    NonCallableHead,
}

/// Sweeps every part for `Keyword` first so a mixed shape like `(f IF x)` goes to
/// `Keyworded`; only with the no-keyword precondition established does it branch on
/// head shape. A keyword-bearing expression is refined to `OperatorChain` when it
/// matches the `Slot (Keyword Slot)+` shape with ≥2 keyword positions.
pub fn classify_dispatch_shape(expr: &KExpression<'_>) -> DispatchShape {
    if expr
        .parts
        .iter()
        .any(|p| matches!(&p.value, ExpressionPart::Keyword(_)))
    {
        if is_operator_chain_shape(&expr.parts) {
            return DispatchShape::OperatorChain;
        }
        return DispatchShape::Keyworded;
    }
    if let [only] = expr.parts.as_slice() {
        return match &only.value {
            ExpressionPart::Identifier(_) => DispatchShape::BareIdentifier,
            ExpressionPart::Type(_) => DispatchShape::BareTypeLeaf,
            ExpressionPart::SigiledTypeExpr(_) => DispatchShape::SigiledTypeExpr,
            ExpressionPart::RecordType(_) => DispatchShape::RecordType,
            ExpressionPart::Literal(_)
            | ExpressionPart::Future(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => DispatchShape::LiteralPassThrough,
            ExpressionPart::Keyword(_) => {
                unreachable!("no-keyword precondition: the sweep above caught every Keyword part")
            }
        };
    }
    // `len >= 2` here: the keyword sweep passed and the single-part block did not
    // match, so an empty `parts` falls through as the explicit `NonCallableHead`.
    let Some(head_part) = expr.parts.first() else {
        return DispatchShape::NonCallableHead;
    };
    match &head_part.value {
        ExpressionPart::Type(_) => DispatchShape::TypeCall,
        ExpressionPart::Identifier(_) => DispatchShape::FunctionValueCall,
        ExpressionPart::Expression(_) => DispatchShape::HeadDeferred,
        ExpressionPart::SigiledTypeExpr(_) => DispatchShape::TypeHeadDeferred,
        // A literal / list / dict / record-literal / record-type / future head in a
        // multi-part expression: heads are always eager and must resolve to something
        // callable, so a non-callable head surfaces a loud `DispatchFailed`. A record
        // *type* is a value, not a callable, so a `:{…}` head joins them here.
        ExpressionPart::Literal(_)
        | ExpressionPart::Future(_)
        | ExpressionPart::ListLiteral(_)
        | ExpressionPart::DictLiteral(_)
        | ExpressionPart::RecordLiteral(_)
        | ExpressionPart::RecordType(_) => DispatchShape::NonCallableHead,
        ExpressionPart::Keyword(_) => {
            unreachable!("no-keyword precondition: the sweep above caught every Keyword part")
        }
    }
}

/// True iff `parts` is the `Slot (Keyword Slot)+` chainable-operator shape: odd
/// length ≥ 5 (slot, keyword, slot, …), every odd index a `Keyword`, every even
/// index a non-keyword slot, with ≥2 keyword positions. The first keyword sits at
/// index 1, so no builtin (`STRUCT …`, keyword-led) collides with it.
fn is_operator_chain_shape(parts: &[Spanned<ExpressionPart<'_>>]) -> bool {
    // Need slot, keyword, slot, keyword, slot — at least 5 parts (2 keywords).
    if parts.len() < 5 || parts.len().is_multiple_of(2) {
        return false;
    }
    parts.iter().enumerate().all(|(i, part)| {
        let is_keyword = matches!(&part.value, ExpressionPart::Keyword(_));
        // Odd indices must be keywords; even indices must be non-keyword slots.
        (i % 2 == 1) == is_keyword
    })
}

/// The unique operator keywords of an `OperatorChain`, sorted and joined into the
/// probe key the per-scope operator registry is looked up by. Returns `None` for any
/// other shape.
fn operator_probe_for(
    parts: &[Spanned<ExpressionPart<'_>>],
    shape: DispatchShape,
) -> Option<String> {
    if shape != DispatchShape::OperatorChain {
        return None;
    }
    let mut ops: Vec<&str> = parts
        .iter()
        .filter_map(|part| match &part.value {
            ExpressionPart::Keyword(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    ops.sort_unstable();
    ops.dedup();
    Some(ops.join(" "))
}

/// A parsed Koan expression: an ordered sequence of `ExpressionPart`s.
///
/// `span` and `file` are `None` for hand-built ASTs.
///
/// `untyped_key`, `shape`, and `operator_probe` are a structural cache filled by
/// [`KExpression::fill_cache`] whenever the parts vector is complete (construction,
/// parse-frame finalization, redundant-wrapper peeling). They are invariant under the
/// dispatch-time splice that replaces an eager `Slot` part with a `Future` (also a
/// `Slot`), so the dispatch driver reads them rather than re-deriving per call. The
/// same AST node re-dispatches on every call of its enclosing function, so the eager
/// cache amortizes across all invocations.
pub struct KExpression<'a> {
    pub parts: Vec<Spanned<ExpressionPart<'a>>>,
    pub span: Option<Span>,
    pub file: Option<FileId>,
    untyped_key: UntypedKey,
    shape: DispatchShape,
    operator_probe: Option<String>,
}

impl<'a> KExpression<'a> {
    /// Spanless constructor; `span`/`file` populated by later phases. Fills the
    /// structural cache from `parts`.
    pub fn new(parts: Vec<Spanned<ExpressionPart<'a>>>) -> Self {
        Self::build(parts, None, None)
    }

    /// Construction chokepoint: takes the full parts vector plus its `span`/`file` and
    /// fills the structural cache. Every literal `KExpression { .. }` site routes here
    /// so no node ships with a stale or unfilled cache.
    pub fn build(
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        span: Option<Span>,
        file: Option<FileId>,
    ) -> Self {
        let mut expr = KExpression {
            parts,
            span,
            file,
            untyped_key: Vec::new(),
            shape: DispatchShape::Keyworded,
            operator_probe: None,
        };
        expr.fill_cache();
        expr
    }

    /// Recompute the structural cache from the current `parts`. Called by every
    /// constructor and by the parse path once a frame's parts vector is finalized.
    pub fn fill_cache(&mut self) {
        self.untyped_key = self
            .parts
            .iter()
            .map(|part| match &part.value {
                ExpressionPart::Keyword(s) => UntypedElement::Keyword(s.clone()),
                _ => UntypedElement::Slot,
            })
            .collect();
        self.shape = classify_dispatch_shape(self);
        self.operator_probe = operator_probe_for(&self.parts, self.shape);
    }

    /// Cached dispatch shape (see [`classify_dispatch_shape`]).
    pub fn shape(&self) -> DispatchShape {
        self.shape
    }

    /// Cached operator-registry probe key: `Some` only for an `OperatorChain`, holding
    /// the sorted-joined unique operator keywords.
    pub fn operator_probe(&self) -> Option<&str> {
        self.operator_probe.as_deref()
    }

    /// Bucket key: `Keyword` parts contribute `Keyword(s)`; every other variant contributes
    /// `Slot`. Must agree with `ExpressionSignature::untyped_key` for any signature that
    /// should match. Reads the structural cache filled at construction.
    pub fn untyped_key(&self) -> UntypedKey {
        self.untyped_key.clone()
    }

    /// Dispatch-time placeholder extractor for typed-binder builtins (`STRUCT <Name> = …`):
    /// if `parts[1]` is a single `Type(t)`, returns its bare name; `None` on shape
    /// mismatch. The builtin body surfaces the structured error.
    pub fn binder_name_from_type_part(&self) -> Option<String> {
        match &self.parts.get(1)?.value {
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        }
    }

    /// If every part is `Expression(_)`, return refs to the inner expressions; otherwise
    /// `None`. The returned `Vec` encodes the all-`Expression` shape — callers iterate
    /// `&KExpression` directly without re-matching the variant.
    pub fn borrow_inner_expressions(&self) -> Option<Vec<&KExpression<'a>>> {
        let mut out = Vec::with_capacity(self.parts.len());
        for p in &self.parts {
            match &p.value {
                ExpressionPart::Expression(b) => out.push(b.as_ref()),
                _ => return None,
            }
        }
        Some(out)
    }

    /// Consuming right-fold counterpart of [`Self::borrow_inner_expressions`]: returns
    /// `(preceding, last)` with both unwrapped from `ExpressionPart::Expression`. On any
    /// shape mismatch returns `self` back so the caller can pass through.
    pub fn try_take_inner_expressions_split(
        self,
    ) -> Result<(Vec<KExpression<'a>>, KExpression<'a>), Self> {
        let mut iter = self.parts.into_iter();
        let Some(first) = iter.next() else {
            return Err(KExpression::new(Vec::new()));
        };
        let mut last: KExpression<'a> = match first.value {
            ExpressionPart::Expression(b) => *b,
            other => {
                let mut parts = vec![Spanned {
                    value: other,
                    span: first.span,
                }];
                parts.extend(iter);
                return Err(KExpression::new(parts));
            }
        };
        let mut preceding: Vec<KExpression<'a>> = Vec::new();
        for p in iter.by_ref() {
            match p.value {
                ExpressionPart::Expression(b) => {
                    preceding.push(std::mem::replace(&mut last, *b));
                }
                other => {
                    let mut parts: Vec<Spanned<ExpressionPart<'a>>> = preceding
                        .into_iter()
                        .map(|e| Spanned::bare(ExpressionPart::Expression(Box::new(e))))
                        .collect();
                    parts.push(Spanned::bare(ExpressionPart::Expression(Box::new(last))));
                    parts.push(Spanned {
                        value: other,
                        span: p.span,
                    });
                    parts.extend(iter);
                    return Err(KExpression::new(parts));
                }
            }
        }
        Ok((preceding, last))
    }
}

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression")
            .field("parts", &self.parts)
            .finish()
    }
}

impl<'a> Parseable<'a> for KExpression<'a> {
    fn equal(&self, other: &dyn Parseable<'a>) -> bool {
        self.summarize() == other.summarize()
    }
    fn ktype(&self) -> crate::machine::model::KType<'a> {
        crate::machine::model::KType::KExpression
    }
    fn summarize(&self) -> String {
        self.parts
            .iter()
            .map(|p| p.value.summarize())
            .collect::<Vec<_>>()
            .join(" ")
    }
}
