//! The syntax AST: what the parser produces and what a function body, a quote, and a stored
//! signature hold. Every node borrows its storage — the parts and the structural cache alike — so a
//! node is `Copy`, `Drop`-free, and copying one to another region is a slice copy rather than a
//! rebuild.
//!
//! A node's structural cache ([`NodeCache`]) is filled at construction from the parts run and the
//! builtin shape table, so a reader answers "which dispatch shape, which bucket key, which
//! builtin shape, what does this install" without walking the run again.
//!
//! The scheduler's own per-dispatch form is `WorkingExpression`, a distinct type in
//! [`values::working`](crate::values::working). A resolved sub-result and a staging hole live only
//! there, which is what keeps this type structurally splice-free: an AST node names no producer
//! region, so nothing here has a reach to describe. Lowering a literal part to a value is
//! [`Value::lower_part`](crate::values::Value::lower_part).

use crate::source::{SourceRef, Spanned};

use crate::memory::{ProgramBrand, Writer, collect, resident};
use crate::parse::builtin_shapes::BuiltinShape;
use crate::parse::builtin_shapes::binder::{StoredBinderKey, binder_plan_for};
use crate::parse::builtin_shapes::layout::SlotLayout;
use crate::parse::depth::node_depth;
use crate::symbols::{
    BinderSymbol, KeySymbol, KeywordSymbol, SymbolInterner, TypeSymbol, ValueSymbol, WILDCARD,
};
use crate::type_lattice::KType;

pub mod blocks;
pub mod program;
pub mod shape;

pub use blocks::{Blocks, PartsSummary};
pub use program::ProgramExpression;
pub use shape::{
    DispatchShape, ExpressionKey, KeyElement, NodeCache, PartClass, classify_dispatch_shape,
    stored_untyped_key,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KLiteral<'a> {
    Number(f64),
    String(&'a str),
    Boolean(bool),
    Null,
}

/// One element of a parsed expression. A keyword and a name — value-side or type-side — are each a
/// symbol the parse minted when it classified the token, so they carry no borrow at all. The arms
/// that do borrow do so at `'a`: a nested node is a pointer to a sibling node in the same storage,
/// and a literal run is a written slice.
#[derive(Debug, Clone, Copy)]
pub enum ExpressionPart<'a> {
    Keyword(KeywordSymbol),
    Identifier(ValueSymbol),
    Type(TypeSymbol),
    Expression(&'a KExpression<'a>),
    /// Parse-context marker for a `:(...)` group: the wrapped `KExpression` must dispatch
    /// in type-context, returning a type-side carrier. Shape recognition is the
    /// dispatcher's responsibility — the parser does no folding here.
    SigiledTypeExpr(&'a KExpression<'a>),
    /// First-class record type `:{x :Number, y :Str}`. The nested `KExpression` is the
    /// field-list `(x :Number, y :Str)` — the same `<name> :<Type>` pair shape a SIG member
    /// or FN parameter list uses. Unlike `SigiledTypeExpr`, this is matched
    /// structurally (the elaborator folds it straight to a record `KType`); there is no
    /// internal type-constructor builtin behind it.
    RecordType(&'a KExpression<'a>),
    ListLiteral(&'a [ListItem<'a>]),
    /// `%"…"`: a string template — the text between its braces and the brace expressions, in
    /// order. See [README.md](README.md) § Templates.
    Template(&'a KTemplate<'a>),
    DictLiteral(&'a [(ExpressionPart<'a>, ExpressionPart<'a>)]),
    /// Anonymous record literal (`{x = 1, y = "a"}`) — identifier-keyed `=` pairs. The
    /// brace frame routes here when the first pair separator is `=`; `:` pairs stay a
    /// `DictLiteral`. A field name is the Identifier or Type token's own parse-minted
    /// symbol, so a key carries its class and no consumer re-derives one from text.
    RecordLiteral(&'a [(BinderSymbol, ExpressionPart<'a>)]),
    Literal(KLiteral<'a>),
    /// A `#(...)` quote: the parenthesized body captured at parse time as data. The parser folds
    /// the sigil and its group into this part, so quoting is static syntax — there is no runtime
    /// quoting operation and the body never dispatches. Behaves as a literal everywhere: it is a
    /// `Slot` in the untyped key and a single one classifies [`DispatchShape::LiteralPassThrough`].
    /// In an evaluated position it is a quote value, whose code `EVAL` runs. See
    /// [README.md](README.md) § The AST: borrowed, `Copy`, and splice-free.
    QuotedExpression(&'a KExpression<'a>),
    /// A marked name, `$x` or `\x`: inside a quote value, a name resolved where the quote is written
    /// or where its code is built rather than left a hole. Classifies as the name it marks.
    MarkedName(Mark, BinderSymbol),
    /// A marked group, `$(…)` or `\(…)`, wrapping exactly one keyworded use. It evaluates nothing:
    /// the mark says where the use's keywords resolve. Classifies as a nested expression.
    MarkedUse(Mark, &'a KExpression<'a>),
    /// `$..xs`: spreads the code elements of `xs` into the quote's syntax where the quote is born.
    /// Only a quote value's syntax holds one; a code value's root never does. A node holding one
    /// matches no builtin shape, since its arity is unknown until the quote is born.
    Splice(ValueSymbol),
    /// A keyworded use or operator run whose uses at `keys` a `USING` filled, through `scope`:
    /// always the one part of its node. Classifies as a nested expression, as `MarkedUse` does,
    /// and prints as the node it holds.
    FilledUse(Scope, &'a [KeySymbol], &'a KExpression<'a>),
}

/// Which binding of its code value a `$` or filled mark reads: the number of the quote or fill it
/// came from (0 in written syntax), and whether it is pinned — laid by a slice or splice inside a
/// quote value nested in the code, so it belongs to the code's root past every quote around it.
/// See [scope README](../scope/README.md#splicing).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Scope {
    pub number: u32,
    pub pinned: bool,
}

impl Scope {
    /// The scope every mark of written syntax carries.
    pub const WRITTEN: Scope = Scope {
        number: 0,
        pinned: false,
    };

    /// The same scope, unpinned.
    pub fn unpinned(self) -> Scope {
        Scope {
            pinned: false,
            ..self
        }
    }
}

/// Where a marked part of a quote resolves: `$` where the quote is written, `\` where its code is
/// built, and a hole a `USING` filled through the fill's scope. See
/// [scope README](../scope/README.md#quotes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    Written(Scope),
    Built,
    Filled(Scope),
}

impl Mark {
    /// The `$` mark of written syntax.
    pub const DOLLAR: Mark = Mark::Written(Scope::WRITTEN);

    /// The sigil this mark is written with; a filled mark has none and prints bare.
    pub fn sigil(self) -> Option<char> {
        match self {
            Mark::Written(_) => Some('$'),
            Mark::Built => Some('\\'),
            Mark::Filled(_) => None,
        }
    }

    /// The scope a `$` or filled mark reads its binding at; a `\` mark reads none.
    pub fn scope(self) -> Option<Scope> {
        match self {
            Mark::Written(scope) | Mark::Filled(scope) => Some(scope),
            Mark::Built => None,
        }
    }

    /// Whether the mark resolves through its code value's bindings rather than where the code is
    /// built: a `$` mark or a filled one.
    pub fn is_bound(self) -> bool {
        self.scope().is_some()
    }
}

/// Which slots of a node are read as written, a quote there syntax of the code around it: those a
/// builtin shape reads as written, or every slot of a node holding a splice.
#[derive(Clone, Copy)]
pub struct WrittenSlots {
    shape: Option<&'static BuiltinShape>,
    splicing: bool,
}

impl WrittenSlots {
    /// The slots of a node of builtin shape `shape`, holding a splice where `splicing`.
    pub fn of(shape: Option<&'static BuiltinShape>, splicing: bool) -> Self {
        WrittenSlots { shape, splicing }
    }

    pub fn at(self, index: usize) -> bool {
        self.splicing || self.shape.is_some_and(|shape| shape.reads_written(index))
    }
}

/// A parts run on its way into a node's region, as a construction door takes it: either a borrowed
/// run to copy in, or an exact-length iterator to fill the region's bytes straight from. One alias
/// for both expression families — [`KExpression`] and
/// [`WorkingExpression`](crate::values::WorkingExpression) split their doors the same way.
///
/// Both forms exist because both shapes of call site do. A fixed-length run — an operator chain's
/// `[left, op, right]`, a wrapped single operand — is a stack array the door copies; a run whose
/// slots are computed one at a time is an iterator, and staging it through an owned `Vec` first
/// would pay a heap allocation and a second copy for bytes the region was going to hold anyway.
pub(crate) type RunIter<I> = <I as IntoIterator>::IntoIter;

/// An item of a list literal: a part, or `..part`, a list whose elements are spread into the
/// literal one level deep. A spread is a list item and nothing else, so no other part holds one; a
/// consumer that means something by an item matches both arms.
#[derive(Debug, Clone, Copy)]
pub enum ListItem<'a> {
    Part(ExpressionPart<'a>),
    Spread(ExpressionPart<'a>),
}

impl<'a> ListItem<'a> {
    /// The part the item is, or spreads: what a consumer reading both alike reads.
    pub fn part(&self) -> &ExpressionPart<'a> {
        match self {
            ListItem::Part(part) | ListItem::Spread(part) => part,
        }
    }

    /// The same item over `part`: a part, or a spread of it.
    pub fn with(&self, part: ExpressionPart<'a>) -> ListItem<'a> {
        match self {
            ListItem::Part(_) => ListItem::Part(part),
            ListItem::Spread(_) => ListItem::Spread(part),
        }
    }

    /// The part the item is, where it spreads nothing.
    pub fn as_part(&self) -> Option<&ExpressionPart<'a>> {
        match self {
            ListItem::Part(part) => Some(part),
            ListItem::Spread(_) => None,
        }
    }
}

/// A template's text and braces: `pieces` holds one more entry than `slots`, piece `i` written
/// before slot `i`, each with `{{` and `}}` read as one brace.
#[derive(Debug, Clone, Copy)]
pub struct KTemplate<'a> {
    pub pieces: &'a [&'a str],
    pub slots: &'a [TemplateSlot<'a>],
}

impl KTemplate<'_> {
    /// Whether a brace keeps its value apart from the text, so the template is a record.
    pub fn kept(&self) -> bool {
        self.slots.iter().any(TemplateSlot::is_kept)
    }
}

/// A template's brace: `{…}` renders its value into the text, `%{…}` keeps it apart. Its part is
/// the one a parenthesized group with the same contents lowers to.
#[derive(Debug, Clone, Copy)]
pub enum TemplateSlot<'a> {
    Rendered(ExpressionPart<'a>),
    Kept(ExpressionPart<'a>),
}

impl<'a> TemplateSlot<'a> {
    /// The part the brace holds.
    pub fn part(&self) -> &ExpressionPart<'a> {
        match self {
            TemplateSlot::Rendered(part) | TemplateSlot::Kept(part) => part,
        }
    }

    /// The same brace over `part`.
    pub fn with<'b>(&self, part: ExpressionPart<'b>) -> TemplateSlot<'b> {
        match self {
            TemplateSlot::Rendered(_) => TemplateSlot::Rendered(part),
            TemplateSlot::Kept(_) => TemplateSlot::Kept(part),
        }
    }

    pub fn is_kept(&self) -> bool {
        matches!(self, TemplateSlot::Kept(_))
    }
}

/// The names of the fields of the record a template with a kept brace evaluates to.
pub const TEMPLATE_FIELDS: [&str; 2] = ["strings", "values"];

/// The fields of the record a template with a kept brace evaluates to: `strings`, then `values`.
pub fn template_fields() -> [BinderSymbol; 2] {
    TEMPLATE_FIELDS
        .map(|name| BinderSymbol::Value(ValueSymbol::classify(name).expect("a value name")))
}

impl<'a> ExpressionPart<'a> {
    /// The structural family this part belongs to — what dispatch-shape classification reads.
    pub fn class(&self) -> PartClass {
        match self {
            ExpressionPart::Keyword(symbol) => PartClass::Keyword(*symbol),
            ExpressionPart::Identifier(_) => PartClass::Identifier,
            ExpressionPart::Type(_) => PartClass::Type,
            ExpressionPart::Expression(_) => PartClass::Expression,
            ExpressionPart::SigiledTypeExpr(_) => PartClass::SigiledTypeExpr,
            ExpressionPart::RecordType(_) => PartClass::RecordType,
            ExpressionPart::ListLiteral(_) => PartClass::ListLiteral,
            ExpressionPart::Template(_) => PartClass::Template,
            ExpressionPart::DictLiteral(_) => PartClass::DictLiteral,
            ExpressionPart::RecordLiteral(_) => PartClass::RecordLiteral,
            ExpressionPart::Literal(_) => PartClass::Literal,
            ExpressionPart::QuotedExpression(_) => PartClass::QuotedExpression,
            ExpressionPart::MarkedName(_, BinderSymbol::Type(_)) => PartClass::Type,
            ExpressionPart::MarkedName(..) => PartClass::Identifier,
            ExpressionPart::MarkedUse(..) | ExpressionPart::FilledUse(..) => PartClass::Expression,
            // Never dispatched: a node holding a splice is composed away when its quote is born.
            ExpressionPart::Splice(_) => PartClass::Identifier,
        }
    }

    /// This part's position in a bucket key: the symbol a keyword carries, `Slot` for every other
    /// part. What the stored key is built from, and what the pre-freeze form match compares.
    pub fn key_element(&self) -> KeyElement {
        match self.class() {
            PartClass::Keyword(symbol) => KeyElement::Keyword(symbol),
            _ => KeyElement::Slot,
        }
    }

    /// The code kind this part is written as, or `None` for a part that is no code: a bare literal
    /// is a value, and a keyword fills no slot. A bare group is code of its own kind, as a quote is.
    pub fn code_kind(&self) -> Option<KType> {
        match self {
            ExpressionPart::Type(_) | ExpressionPart::MarkedName(_, BinderSymbol::Type(_)) => {
                Some(KType::TYPE_NAME_TOKEN)
            }
            ExpressionPart::Identifier(_) | ExpressionPart::MarkedName(..) => {
                Some(KType::IDENTIFIER)
            }
            ExpressionPart::Expression(node) | ExpressionPart::QuotedExpression(node) => {
                Some(node.code_kind())
            }
            ExpressionPart::MarkedUse(..) | ExpressionPart::FilledUse(..) => {
                Some(KType::EXPRESSION)
            }
            ExpressionPart::SigiledTypeExpr(_) => Some(KType::SIGILED_TYPE_EXPR),
            ExpressionPart::RecordType(_) => Some(KType::RECORD_TYPE),
            ExpressionPart::Keyword(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::Template(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_)
            | ExpressionPart::Splice(_) => None,
        }
    }

    /// Whether this part is a name: an identifier, a type name, or a marked name.
    pub fn is_name(&self) -> bool {
        matches!(
            self,
            ExpressionPart::Identifier(_)
                | ExpressionPart::Type(_)
                | ExpressionPart::MarkedName(..)
        )
    }

    /// Whether this part is `_` — as a dict's key, the dict's default.
    pub fn is_wildcard(&self) -> bool {
        matches!(self, ExpressionPart::Keyword(symbol) if *symbol == WILDCARD.symbol())
    }

    /// Wrap a run of parts as a nested `Expression` part, writing both the run and the node into
    /// the program storage `brand` names. Takes a [`ProgramBrand`] because the arm it builds is a
    /// value-channel conduit: the marker on its payload is the proof the cell doors cite.
    pub fn expression(
        brand: ProgramBrand<'a>,
        parts: &[Spanned<ExpressionPart<'a>>],
        source: SourceRef,
    ) -> ExpressionPart<'a> {
        ExpressionPart::Expression(brand.nested_node(parts, source))
    }

    /// [`expression`](Self::expression)'s peer for a run whose slots are computed — see [`RunIter`].
    pub fn expression_from_iter<I>(
        brand: ProgramBrand<'a>,
        parts: I,
        source: SourceRef,
    ) -> ExpressionPart<'a>
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        ExpressionPart::Expression(brand.nested_node_from_iter(parts, source))
    }

    /// What a copy of what this part points at writes, past the part word itself: a nested node's
    /// weight, a literal's run and its parts' own, a string's bytes, and a filled use's key run,
    /// each run with its [`ALLOCATION_SLACK`].
    fn laid_weight(&self) -> u32 {
        let run = |len: usize, item: usize| (len * item + ALLOCATION_SLACK) as u32;
        let sum =
            |own: u32, parts: &mut dyn Iterator<Item = u32>| parts.fold(own, u32::saturating_add);
        match self {
            ExpressionPart::Expression(node)
            | ExpressionPart::SigiledTypeExpr(node)
            | ExpressionPart::RecordType(node)
            | ExpressionPart::QuotedExpression(node)
            | ExpressionPart::MarkedUse(_, node) => node.weight(),
            ExpressionPart::FilledUse(_, keys, node) => node
                .weight()
                .saturating_add(run(keys.len(), size_of::<KeySymbol>())),
            ExpressionPart::ListLiteral(items) => sum(
                run(items.len(), size_of::<ListItem<'_>>()),
                &mut items.iter().map(|item| item.part().laid_weight()),
            ),
            ExpressionPart::Template(template) => sum(
                run(1, size_of::<KTemplate<'_>>())
                    .saturating_add(run(template.pieces.len(), size_of::<&str>()))
                    .saturating_add(run(template.slots.len(), size_of::<TemplateSlot<'_>>())),
                &mut (template.pieces.iter().map(|piece| run(piece.len(), 1)))
                    .chain(template.slots.iter().map(|slot| slot.part().laid_weight())),
            ),
            ExpressionPart::DictLiteral(pairs) => sum(
                run(
                    pairs.len(),
                    size_of::<(ExpressionPart<'_>, ExpressionPart<'_>)>(),
                ),
                &mut pairs
                    .iter()
                    .map(|(key, value)| key.laid_weight().saturating_add(value.laid_weight())),
            ),
            ExpressionPart::RecordLiteral(pairs) => sum(
                run(pairs.len(), size_of::<(BinderSymbol, ExpressionPart<'_>)>()),
                &mut pairs.iter().map(|(_, value)| value.laid_weight()),
            ),
            ExpressionPart::Literal(KLiteral::String(text)) => run(text.len(), 1),
            ExpressionPart::Keyword(_)
            | ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::MarkedName(..)
            | ExpressionPart::Splice(_) => 0,
        }
    }

    /// Per-part subset of [`KExpression::write_summary`], written straight into `f`.
    pub fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        symbols: &SymbolInterner,
    ) -> std::fmt::Result {
        self.write_in(f, symbols, None)
    }

    /// [`write_summary`](Self::write_summary) inside a template delimited by `avoid`, whose string
    /// literals take the other quote.
    fn write_in(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        symbols: &SymbolInterner,
        avoid: Option<char>,
    ) -> std::fmt::Result {
        match self {
            ExpressionPart::Keyword(symbol) => write!(f, "{}", symbols.display(symbol.symbol())),
            ExpressionPart::Identifier(v) => write!(f, "{}", symbols.display(v.symbol())),
            ExpressionPart::Type(t) => write!(f, "{}", symbols.display(t.symbol())),
            ExpressionPart::Expression(e) => {
                f.write_str("(")?;
                e.write_in(f, symbols, avoid)?;
                f.write_str(")")
            }
            ExpressionPart::SigiledTypeExpr(e) => {
                f.write_str(":(")?;
                e.write_in(f, symbols, avoid)?;
                f.write_str(")")
            }
            ExpressionPart::RecordType(e) => {
                f.write_str(":{")?;
                e.write_in(f, symbols, avoid)?;
                f.write_str("}")
            }
            ExpressionPart::QuotedExpression(e) => {
                f.write_str("#(")?;
                e.write_in(f, symbols, avoid)?;
                f.write_str(")")
            }
            ExpressionPart::MarkedName(mark, name) => {
                if let Some(sigil) = mark.sigil() {
                    write!(f, "{sigil}")?;
                }
                write!(f, "{}", symbols.display(name.symbol()))
            }
            ExpressionPart::MarkedUse(mark, e) => {
                if let Some(sigil) = mark.sigil() {
                    write!(f, "{sigil}")?;
                }
                f.write_str("(")?;
                e.write_in(f, symbols, avoid)?;
                f.write_str(")")
            }
            ExpressionPart::FilledUse(_, _, e) => e.write_in(f, symbols, avoid),
            ExpressionPart::Splice(name) => write!(f, "$..{}", symbols.display(name.symbol())),
            ExpressionPart::ListLiteral(items) => {
                f.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" ")?;
                    }
                    if let ListItem::Spread(_) = item {
                        f.write_str("..")?;
                    }
                    item.part().write_in(f, symbols, avoid)?;
                }
                f.write_str("]")
            }
            ExpressionPart::Template(template) => {
                let quote = match template
                    .pieces
                    .iter()
                    .any(|piece| holds_unescaped(piece, '"'))
                {
                    true => '\'',
                    false => '"',
                };
                let quote = if avoid == Some(quote) {
                    other(quote)
                } else {
                    quote
                };
                write!(f, "%{quote}")?;
                let escaped = |piece: &str| piece.replace('{', "{{").replace('}', "}}");
                for (piece, slot) in template.pieces.iter().zip(template.slots.iter()) {
                    // `%{` always opens a kept brace, so no piece ends in `%` before a rendered one.
                    debug_assert!(slot.is_kept() || !piece.ends_with('%'));
                    f.write_str(&escaped(piece))?;
                    f.write_str(if slot.is_kept() { "%{" } else { "{" })?;
                    // A brace's node prints bare, spaced off where it opens with a brace of its
                    // own, since `{{` is text.
                    let (lead, node) = match slot.part() {
                        ExpressionPart::Expression(node) => (node.parts.first(), Some(node)),
                        _ => (None, None),
                    };
                    if lead.is_some_and(|lead| opens_brace(&lead.value)) {
                        f.write_str(" ")?;
                    }
                    match node {
                        Some(node) => node.write_in(f, symbols, Some(quote))?,
                        None => slot.part().write_in(f, symbols, Some(quote))?,
                    }
                    f.write_str("}")?;
                }
                let last = template.pieces.last().expect("one more piece than slots");
                write!(f, "{}{quote}", escaped(last))
            }
            ExpressionPart::DictLiteral(pairs) => {
                f.write_str("{")?;
                for (index, (k, v)) in pairs.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    k.write_in(f, symbols, avoid)?;
                    f.write_str(": ")?;
                    v.write_in(f, symbols, avoid)?;
                }
                f.write_str("}")
            }
            ExpressionPart::RecordLiteral(pairs) => {
                f.write_str("{")?;
                for (index, (k, v)) in pairs.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{} = ", symbols.display(k.symbol()))?;
                    v.write_in(f, symbols, avoid)?;
                }
                f.write_str("}")
            }
            ExpressionPart::Literal(lit) => match lit {
                KLiteral::Number(n) => write!(f, "{n}"),
                KLiteral::String(s) => {
                    let quote = if holds_unescaped(s, '"') { '\'' } else { '"' };
                    let quote = if avoid == Some(quote) {
                        other(quote)
                    } else {
                        quote
                    };
                    write!(f, "{quote}{s}{quote}")
                }
                KLiteral::Boolean(b) => write!(f, "{b}"),
                KLiteral::Null => f.write_str("null"),
            },
        }
    }

    /// [`write_summary`](Self::write_summary) as a `Display` view — what a `format!` argument
    /// naming a part uses.
    ///
    /// Parse renders a part here — a record literal's field-name error names the token it
    /// rejected — while still filling the interner a run frame has yet to adopt.
    pub fn summary<'x>(&'x self, symbols: &'x SymbolInterner) -> AstPartSummary<'x, 'a> {
        AstPartSummary {
            part: self,
            symbols,
        }
    }

    /// The part's surface as an owned `String`.
    pub fn summarize(&self, symbols: &SymbolInterner) -> String {
        self.summary(symbols).to_string()
    }
}

/// What one allocation may spend past its bytes on its alignment: a node's [`weight`] adds it per
/// allocation a copy makes, so the weight over-prices padding and never under-prices it.
///
/// [`weight`]: KExpression::weight
pub(crate) const ALLOCATION_SLACK: usize = 16;

/// Whether a string body, stored as written between its quotes, holds `quote` unescaped: preceded
/// by an even run of backslashes, since a backslash escapes the character after it. Printing reads
/// it to pick the quote that delimits the body; one of the two always does, since one delimited it.
fn holds_unescaped(body: &str, quote: char) -> bool {
    let mut backslashes = 0;
    for c in body.chars() {
        if c == quote && backslashes % 2 == 0 {
            return true;
        }
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
    }
    false
}

/// Whether `part` prints opening with `{`: a dict or record literal, or a filled use led by one.
fn opens_brace(part: &ExpressionPart<'_>) -> bool {
    match part {
        ExpressionPart::DictLiteral(_) | ExpressionPart::RecordLiteral(_) => true,
        ExpressionPart::FilledUse(_, _, node) => node
            .parts
            .first()
            .is_some_and(|lead| opens_brace(&lead.value)),
        _ => false,
    }
}

/// The quote that is not `quote`.
fn other(quote: char) -> char {
    if quote == '"' { '\'' } else { '"' }
}

/// A parsed Koan expression: an ordered run of [`ExpressionPart`]s borrowed from the storage that
/// parsed them.
///
/// Every node carries a [`SourceRef`]: code always comes from somewhere, so no door builds a node
/// without one.
///
/// [`cache`](Self::cache) is the structural cache the construction doors fill once the parts run is
/// complete — the bucket key, the dispatch shape, the matched builtin shape and the binder plan —
/// so the dispatch driver reads it rather than re-deriving on every call of the enclosing
/// function. The binder plan is per-node only: what this node installs when it is
/// submitted as a statement, and `None` when it is not itself a binder. A statement's namespace is
/// legible from its own spine, never from what its slots contain.
///
/// Every field is a shared borrow at `'a` or a `Copy` handle, so the node is covariant in `'a`: a
/// program-storage node flows into shorter-lived code by ordinary subtyping, with no reattach and no
/// witness.
#[derive(Clone, Copy)]
pub struct KExpression<'a> {
    pub parts: &'a [Spanned<ExpressionPart<'a>>],
    /// Where this node's code came from: its own text, or, for a node built from other code, the
    /// code it was built from or where it was generated.
    pub source: SourceRef,
    cache: NodeCache<'a>,
    body_layout: &'a SlotLayout<'a>,
    /// How deep this node's syntax nests, counted by [`node_depth`] at construction.
    depth: u32,
    /// The bytes a copy of this node's subtree writes, saturating: see [`Self::weight`].
    weight: u32,
    /// A node of more than [`BLOCK`](blocks::BLOCK) parts: what reads a stretch of them in O(1).
    blocks: Option<&'a Blocks<'a>>,
}

impl<'a> KExpression<'a> {
    /// Construction door for a borrowed run: copy it into `writer`'s store, then fill the
    /// structural cache.
    pub fn build(
        writer: Writer<'a, '_>,
        parts: &[Spanned<ExpressionPart<'a>>],
        source: SourceRef,
    ) -> Self {
        Self::from_run(writer, collect(writer, parts.iter().copied()), source)
    }

    /// [`build`](Self::build)'s peer for a run whose slots are computed — see [`RunIter`].
    pub fn build_from_iter<I>(writer: Writer<'a, '_>, parts: I, source: SourceRef) -> Self
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        Self::from_run(writer, collect(writer, parts.into_iter()), source)
    }

    /// Construction chokepoint, over a parts run **already resident** in `writer`'s store: fills the
    /// structural cache from it and does nothing else. Every door above lands here, differing only
    /// in how the run reached the store — so none ships with a stale or unfilled cache and no part
    /// run is mutated after it is frozen.
    fn from_run(
        writer: Writer<'a, '_>,
        parts: &'a [Spanned<ExpressionPart<'a>>],
        source: SourceRef,
    ) -> Self {
        let key = stored_untyped_key(writer, parts.iter().map(|part| part.value.key_element()));
        let cache = NodeCache::build(key, parts.first().map(|part| part.value.class()));
        let splices = parts
            .iter()
            .any(|part| matches!(part.value, ExpressionPart::Splice(_)));
        let cache = if splices { cache.splicing() } else { cache };
        let depth = node_depth(parts, &cache);
        Self::seal(writer, parts, source, cache, depth)
    }

    /// The node itself, over a resident run and a settled structural cache: fills the binder plan
    /// and freezes. The one place a `KExpression` is written, so neither door above can ship a node
    /// whose plan disagrees with its parts.
    fn seal(
        writer: Writer<'a, '_>,
        parts: &'a [Spanned<ExpressionPart<'a>>],
        source: SourceRef,
        cache: NodeCache<'a>,
        depth: u32,
    ) -> Self {
        let mut expression = KExpression {
            parts,
            source,
            cache,
            body_layout: SlotLayout::EMPTY,
            depth,
            weight: 0,
            blocks: Blocks::of(writer, parts),
        };
        // The extractors read the node, so the plan is filled once it stands. It is written behind
        // a reference rather than stored inline: it is the widest thing a node would carry, and
        // `KExpression` is copied on every part walk.
        let plan = binder_plan_for(writer, cache.builtin_shape(), &expression)
            .map(|key| resident(writer, key));
        expression.cache = cache.declaring(plan);
        // The value binders this node would open a frame over, read off the same statement plans
        // the claim stamp and the `CLOSE` capture walk read. Filled for every node — a node is a
        // body only where a callable names it as one, and the read is a walk of plans already
        // cached on the statements it wraps.
        expression.body_layout = SlotLayout::of_body(writer, &expression);
        expression.weight = expression.laid_weight();
        expression
    }

    /// What a copy of this node's subtree writes: the node, its parts run and key, each string's
    /// bytes, its binder plan and body layout as the seal laid them, and each nested node's own
    /// weight. Read off the sealed node, so it is O(parts).
    fn laid_weight(&self) -> u32 {
        // The node, its parts run and its key run: three allocations.
        let own = size_of::<KExpression<'_>>()
            + self.parts.len()
                * (size_of::<Spanned<ExpressionPart<'_>>>() + size_of::<KeyElement>())
            + 3 * ALLOCATION_SLACK
            + self.cache.laid_bytes()
            + self.body_layout.laid_bytes()
            + self.blocks.map_or(0, Blocks::laid_bytes);
        let parts = self.parts.iter().map(|part| part.value.laid_weight());
        parts.fold(own as u32, u32::saturating_add)
    }

    /// What a node over `parts` parts writes of its own, at most, past its parts' words and what
    /// they point at, where those weigh `below`: the node, its runs' slack, its body layout, its
    /// blocks and its binder plan. A code view prices a node it has not laid with it. The plan's
    /// one long key, an `FN`'s, is a signature's slots, each a part of a node among the parts, so
    /// it is priced as a share of `below`; a view passes the weight of its stretches alone, so its
    /// price does not compound as it nests.
    pub fn own_bound(parts: usize, below: u64) -> u64 {
        let word = (size_of::<Spanned<ExpressionPart<'_>>>() + size_of::<KeyElement>()) as u64;
        let key = size_of::<KeyElement>();
        let own = size_of::<KExpression<'_>>()
            + 3 * ALLOCATION_SLACK
            + SlotLayout::bound(parts)
            + Blocks::bound(parts)
            + size_of::<StoredBinderKey<'_>>()
            + 3 * ALLOCATION_SLACK
            + 6 * key;
        own as u64 + below.div_ceil(word) * key as u64
    }

    /// The bytes a copy of this node's subtree writes, saturating: a code view prices a stretch of
    /// syntax with it, so it may over-price and never under-price.
    pub fn weight(&self) -> u32 {
        self.weight
    }

    /// Whether this node holds a `$..` splice among its own parts.
    pub fn holds_splice(&self) -> bool {
        self.cache.splices()
    }

    /// Whether a quote of this node composes where it is born: a `$..` splice lies among the parts
    /// it owns — its own, its groups', and those of each quote a builtin reads as written.
    pub fn composes(&self) -> bool {
        let slots = self.written_slots();
        self.parts
            .iter()
            .enumerate()
            .any(|(index, part)| match &part.value {
                ExpressionPart::Splice(_) => true,
                ExpressionPart::Expression(node) => node.composes(),
                ExpressionPart::QuotedExpression(node) => slots.at(index) && node.composes(),
                _ => false,
            })
    }

    /// Visit each part this node owns: its own and, depth first, those of the syntax nested in
    /// them, a quote's only where a builtin reads it as written — the parts whose marks and
    /// splices a quote of this node holds at its root.
    pub fn each_owned(&self, visit: &mut impl FnMut(&ExpressionPart<'a>)) {
        let slots = self.written_slots();
        for (index, part) in self.parts.iter().enumerate() {
            owned_part(&part.value, slots.at(index), visit);
        }
    }

    /// Which of this node's parts are quotes that are syntax of the code around them rather than
    /// quote values: those in a slot a builtin reads as written, or every quote of a node holding
    /// a splice, which may make it a builtin's body.
    pub fn written_slots(&self) -> WrittenSlots {
        WrittenSlots::of(self.cache.builtin_shape(), self.holds_splice())
    }

    /// The summary of parts `[from, to)`, `from ≤ to ≤ parts.len()`: O(1) however long the node.
    pub fn stretch(&self, from: usize, to: usize) -> PartsSummary {
        blocks::stretch(self.parts, self.blocks, from, to)
    }

    /// Build a node and write it, for a part arm that nests one ([`ExpressionPart::Expression`] and
    /// its sigil siblings hold `&'a KExpression<'a>`).
    pub fn nested(
        writer: Writer<'a, '_>,
        parts: &[Spanned<ExpressionPart<'a>>],
        source: SourceRef,
    ) -> &'a KExpression<'a> {
        resident(writer, Self::build(writer, parts, source))
    }

    /// [`nested`](Self::nested)'s peer for a run whose slots are computed — see [`RunIter`].
    pub fn nested_from_iter<I>(
        writer: Writer<'a, '_>,
        parts: I,
        source: SourceRef,
    ) -> &'a KExpression<'a>
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        resident(writer, Self::build_from_iter(writer, parts, source))
    }

    /// The [`SlotLayout`] of this node **as a body**: the value binders its statements declare,
    /// each at the lexical position its statement submits at. Computed at construction beside the
    /// binder plan it is read from, and homed in this node's own region — never in a frame's, which
    /// a spliced-out array could outlive.
    pub fn body_layout(&self) -> &'a SlotLayout<'a> {
        self.body_layout
    }

    /// How deep this node's syntax nests, an operator run counted as the nesting its rewrite
    /// builds. A program deeper than [`MAX_SYNTAX_DEPTH`](crate::parse::MAX_SYNTAX_DEPTH) is
    /// refused at parse; see [`node_depth`].
    pub fn depth(&self) -> usize {
        self.depth as usize
    }

    /// The structural facts this node cached at construction. Every accessor below reads it, and
    /// the working copy carries it over whole.
    pub fn cache(&self) -> &NodeCache<'a> {
        &self.cache
    }

    /// This node's own binder plan — `Some` iff this node is itself a binder.
    pub fn binder_plan(&self) -> Option<StoredBinderKey<'a>> {
        self.cache.binder_plan()
    }

    /// The statement this node stands for. A written paren around a statement (`((…))`) is the
    /// same statement as its child, so it reads through every such wrapper; every other shape is
    /// its own statement.
    pub(crate) fn statement_spine(&self) -> &KExpression<'a> {
        let mut spine = self;
        while let [only] = spine.parts
            && let ExpressionPart::Expression(child) = only.value
        {
            spine = child;
        }
        spine
    }

    /// What this statement installs. The statement's *own* plan key, never anything its slots
    /// contain — the namespace a block introduces is legible from its statement spines alone,
    /// which is what lets the block fan-out rule on duplicate declarations before any statement
    /// runs.
    ///
    /// Every key the plan names is a borrow into the declaring node's own region, so reading a
    /// block's whole namespace allocates nothing.
    pub(crate) fn statement_binder_plan(&self) -> Option<StoredBinderKey<'a>> {
        self.statement_spine().binder_plan()
    }

    /// This node's statements **as a body**, beside the lexical position each submits at: statement
    /// `i` of a statement block writes at `i + 1`, and a single-statement body is its own statement
    /// at `1`.
    pub(crate) fn body_statements<'n>(
        &'n self,
    ) -> impl Iterator<Item = (&'n KExpression<'a>, usize)> + Clone {
        let block = self.is_statement_block().then_some(self.parts);
        let statements = block.unwrap_or_default().iter().map(|part| {
            let ExpressionPart::Expression(statement) = part.value else {
                unreachable!("a statement block's parts are all expressions");
            };
            statement
        });
        let single = (!self.is_statement_block()).then_some(self);
        statements
            .chain(single)
            .enumerate()
            .map(|(i, statement)| (statement, i + 1))
    }

    /// The code kind of a quote whose body is this node, read off the body as written — see the
    /// lattice [README.md](../../lattice/src/types/README.md) § The code family. Two or more statements are
    /// a `Block`; a statement of a member-declaring builtin shape a `Declaration`, and one that
    /// installs a `Binder`; a lone scalar literal or nested quote a `Literal`; a lone name, keyword,
    /// `:(…)` or `:{…}` its own kind; and every other statement an `Expression`. A written paren is
    /// a part of its own, so `((y))` and `((LET x = 1))` are expressions.
    pub fn code_kind(&self) -> KType {
        if self.is_statement_block() {
            return KType::BLOCK;
        }
        if self
            .cache
            .builtin_shape()
            .is_some_and(|shape| shape.id.declares_member())
        {
            return KType::DECLARATION;
        }
        if self.binder_plan().is_some() {
            return KType::BINDER;
        }
        match self.parts {
            [only] => Self::lone_kind(&only.value),
            _ => KType::EXPRESSION,
        }
    }

    /// The code kind of a node of `only` alone, where it is no block, declaration or binder.
    pub fn lone_kind(only: &ExpressionPart<'_>) -> KType {
        match only {
            ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_) => KType::LITERAL,
            ExpressionPart::Type(_) | ExpressionPart::MarkedName(_, BinderSymbol::Type(_)) => {
                KType::TYPE_NAME_TOKEN
            }
            ExpressionPart::Identifier(_) | ExpressionPart::MarkedName(..) => KType::IDENTIFIER,
            ExpressionPart::Keyword(_) => KType::KEYWORD,
            ExpressionPart::SigiledTypeExpr(_) => KType::SIGILED_TYPE_EXPR,
            ExpressionPart::RecordType(_) => KType::RECORD_TYPE,
            ExpressionPart::MarkedUse(..)
            | ExpressionPart::FilledUse(..)
            | ExpressionPart::Splice(_)
            | ExpressionPart::Expression(_)
            | ExpressionPart::ListLiteral(_)
            | ExpressionPart::Template(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => KType::EXPRESSION,
        }
    }

    /// The declared-name position of the binder form this node's bucket key matches
    /// ([`BinderFacts::name_slot`](crate::parse::builtin_shapes::binder::BinderFacts::name_slot)); `None`
    /// when the node is not a binder form, or the form's spine carries no declared name (`FN`,
    /// `OP`).
    pub fn binder_name_slot(&self) -> Option<usize> {
        self.cache.binder_name_slot()
    }

    /// True when this expression is a statement block: two or more parts, all of them
    /// `Expression`. The single definition every body splitter and the binder-install aggregation
    /// share, so the multi-statement cutoff is stated once.
    pub fn is_statement_block(&self) -> bool {
        self.parts.len() >= 2
            && self
                .parts
                .iter()
                .all(|part| matches!(part.value, ExpressionPart::Expression(_)))
    }

    /// Cached dispatch shape (see [`classify_dispatch_shape`]).
    pub fn shape(&self) -> DispatchShape {
        self.cache.shape()
    }

    /// The stored bucket key, as a borrow of the run written at construction: `Keyword` parts
    /// contribute `Keyword(symbol)`, every other variant a `Slot`. Must agree with
    /// `ExpressionSignature::untyped_key` for any signature that should match.
    pub fn stored_key(&self) -> &'a [KeyElement] {
        self.cache.stored_key()
    }

    /// Binder-name extractor for typed-binder builtins (`SIG <Name> = …`, `UNION <Name> = …`):
    /// if `parts[1]` is a single `Type(t)`, returns its symbol; `None` on shape
    /// mismatch. The builtin body surfaces the structured error.
    pub fn binder_name_from_type_part(&self) -> Option<TypeSymbol> {
        match &self.parts.get(1)?.value {
            ExpressionPart::Type(t) => Some(*t),
            _ => None,
        }
    }

    /// Surface rendering of the whole expression, written straight into `f`, resolving each
    /// symbol-carrying part through the run's interner.
    pub fn write_summary(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        symbols: &SymbolInterner,
    ) -> std::fmt::Result {
        self.write_in(f, symbols, None)
    }

    /// [`write_summary`](Self::write_summary) inside a template delimited by `avoid`.
    fn write_in(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        symbols: &SymbolInterner,
        avoid: Option<char>,
    ) -> std::fmt::Result {
        for (index, part) in self.parts.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            part.value.write_in(f, symbols, avoid)?;
        }
        Ok(())
    }

    /// [`write_summary`](Self::write_summary) as a `Display` view.
    pub fn summary<'x>(&'x self, symbols: &'x SymbolInterner) -> ExpressionSummary<'x, 'a> {
        ExpressionSummary {
            expression: self,
            symbols,
        }
    }

    /// The expression's surface as an owned `String`.
    pub fn summarize(&self, symbols: &SymbolInterner) -> String {
        self.summary(symbols).to_string()
    }
}

/// An [`ExpressionPart::summary`] view: one AST part plus the interner its symbols resolve
/// through.
pub struct AstPartSummary<'x, 'a> {
    part: &'x ExpressionPart<'a>,
    symbols: &'x SymbolInterner,
}

impl std::fmt::Display for AstPartSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.part.write_summary(f, self.symbols)
    }
}

/// A [`KExpression::summary`] view: one expression plus the interner its symbols resolve through.
pub struct ExpressionSummary<'x, 'a> {
    expression: &'x KExpression<'a>,
    symbols: &'x SymbolInterner,
}

impl std::fmt::Display for ExpressionSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.expression.write_summary(f, self.symbols)
    }
}

impl<'a> std::fmt::Debug for KExpression<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KExpression")
            .field("parts", &self.parts)
            .finish()
    }
}

/// [`KExpression::each_owned`] for `part`; `written` where a builtin reads it as written.
fn owned_part<'a>(
    part: &ExpressionPart<'a>,
    written: bool,
    visit: &mut impl FnMut(&ExpressionPart<'a>),
) {
    visit(part);
    match part {
        ExpressionPart::QuotedExpression(node) if written => node.each_owned(visit),
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node)
        | ExpressionPart::MarkedUse(_, node)
        | ExpressionPart::FilledUse(_, _, node) => node.each_owned(visit),
        ExpressionPart::ListLiteral(items) => {
            for item in items.iter() {
                owned_part(item.part(), written, visit);
            }
        }
        ExpressionPart::Template(template) => {
            for slot in template.slots.iter() {
                owned_part(slot.part(), written, visit);
            }
        }
        ExpressionPart::DictLiteral(pairs) => {
            for (key, value) in pairs.iter() {
                owned_part(key, written, visit);
                owned_part(value, written, visit);
            }
        }
        ExpressionPart::RecordLiteral(fields) => {
            for (_, value) in fields.iter() {
                owned_part(value, written, visit);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) mod tests;
