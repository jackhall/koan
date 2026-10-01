//! Surface-syntax rendering — the one recursion in the lattice written by hand.
//!
//! Rendering spells syntax *between* children and inherits the quantifier binder from above, which
//! a fold cannot express, so it is exempt from the driver rule: one exhaustive match per surface,
//! the binder threaded top-down, and a new node variant must be spelled here.
//!
//! Every entry point takes the registry and the symbol interner, never a bundle: the lattice
//! knows about types and symbols and nothing else.

use std::fmt::Write as _;

use crate::symbols::{
    BinderSymbol, KeywordSymbol, Symbol, SymbolDisplay, SymbolInterner, TypeSymbol,
};

use super::digest::empty_schema_digest;
use super::handle::{
    ANY_NAME, BINDER_NAME, BLOCK_NAME, BOOL_NAME, CODE_NAME, DECLARATION_NAME, EXPRESSION_NAME,
    IDENTIFIER_NAME, KEYWORD_NAME, KType, LITERAL_NAME, MODULE_NAME, NAME_NAME, NEVER_NAME,
    NULL_NAME, NUMBER_NAME, RECORD_TYPE_NAME, SIGILED_TYPE_EXPR_NAME, STR_NAME, SYMBOL_NAME,
    TYPE_NAME_TOKEN_NAME, VALUE_NAME,
};
use super::node::TypeNode;
use super::operators::{FoldDirection, ReductionMode};
use super::ranking::Ranked;
use super::record::Record;
use super::registry::TypeRegistry;
use super::schema::{DeclaredGroup, SigSchema, shape_elements, shape_return, shape_slots};
use super::shape::DispatchTokenElement;
use super::sig_relations::FitsFailure;

/// A symbol's text, resolved through the run's interner. Rendering stays total: a miss prints a
/// placeholder rather than panicking, because error formatting must never be the thing that fails.
pub fn render_symbol(symbol: Symbol, symbols: &SymbolInterner) -> String {
    symbols.render(symbol)
}

/// [`render_symbol`] as a `Display` view, so the text lands in the message's buffer without a
/// `String` of its own on the way.
pub fn display_symbol(symbol: Symbol, symbols: &SymbolInterner) -> SymbolDisplay<'_> {
    symbols.display(symbol)
}

/// Surface-syntax rendering, straight into `f`. The one place the surface arms are written.
///
/// `binder` is the quantifier binder in force — the enclosing shape's parameter names, which its
/// element and return positions dereference through [`TypeNode::Quantified`]. Threaded rather than
/// looked up, because a quantified leaf carries an index and nothing else; a nested shape rebinds
/// it with its own list, exactly as it shadows one in the relations.
fn write_name_in(
    kt: KType,
    f: &mut std::fmt::Formatter<'_>,
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
    binder: &[TypeSymbol],
) -> std::fmt::Result {
    types.with_node(kt, |node| match node {
        TypeNode::Number => f.write_str(NUMBER_NAME.text()),
        TypeNode::Str => f.write_str(STR_NAME.text()),
        TypeNode::Bool => f.write_str(BOOL_NAME.text()),
        TypeNode::Null => f.write_str(NULL_NAME.text()),
        TypeNode::Identifier => f.write_str(IDENTIFIER_NAME.text()),
        TypeNode::Symbol => f.write_str(SYMBOL_NAME.text()),
        TypeNode::TypeNameToken => f.write_str(TYPE_NAME_TOKEN_NAME.text()),
        TypeNode::Expression => f.write_str(EXPRESSION_NAME.text()),
        TypeNode::SigiledTypeExpr => f.write_str(SIGILED_TYPE_EXPR_NAME.text()),
        TypeNode::RecordType => f.write_str(RECORD_TYPE_NAME.text()),
        TypeNode::Literal => f.write_str(LITERAL_NAME.text()),
        TypeNode::Block => f.write_str(BLOCK_NAME.text()),
        TypeNode::Declaration => f.write_str(DECLARATION_NAME.text()),
        TypeNode::Binder => f.write_str(BINDER_NAME.text()),
        TypeNode::Name => f.write_str(NAME_NAME.text()),
        TypeNode::Keyword => f.write_str(KEYWORD_NAME.text()),
        TypeNode::Any => f.write_str(ANY_NAME.text()),
        TypeNode::AnyValue => f.write_str(VALUE_NAME.text()),
        TypeNode::AnyCode => f.write_str(CODE_NAME.text()),
        TypeNode::Never => f.write_str(NEVER_NAME.text()),
        TypeNode::OfKind(kind) => f.write_str(kind.surface_keyword()),
        TypeNode::CodeNeeding { kind, names } => {
            f.write_str(":(")?;
            write_name_in(*kind, f, types, symbols, binder)?;
            f.write_str(" NEEDING #[")?;
            for (index, name) in names.iter().enumerate() {
                if index > 0 {
                    f.write_str(" ")?;
                }
                match name {
                    BinderSymbol::Key(key) => write!(f, "({})", symbols.display(key.symbol()))?,
                    _ => write!(f, "{}", symbols.display(name.symbol()))?,
                }
            }
            f.write_str("])")
        }
        TypeNode::List { element } => {
            f.write_str(":(LIST OF ")?;
            write_name_in(*element, f, types, symbols, binder)?;
            f.write_str(")")
        }
        TypeNode::Dict { key, value } => {
            f.write_str(":(MAP ")?;
            write_name_in(*key, f, types, symbols, binder)?;
            f.write_str(" -> ")?;
            write_name_in(*value, f, types, symbols, binder)?;
            f.write_str(")")
        }
        // `:{x :Number y :Str}` — the braced type-sigil surface. Fields render space-separated like
        // FN params, which the field-list parser accepts.
        TypeNode::Record { fields } => {
            f.write_str(":{")?;
            write_param_record(f, *fields, types, symbols, binder)?;
            f.write_str("}")
        }
        // `:(FN FOR ALL #[Elt] :{x :Elt} -> Elt)`, and without the group where it binds none — the
        // group writer emits its own trailing space and nothing at all for an empty group, so the
        // monomorphic surface is unchanged. Params and return read against the function's own
        // group where it has one, and against the enclosing binder's where it has none.
        TypeNode::KFunction {
            quantifiers,
            bounds,
            params,
            ret,
        } => {
            let inner = if quantifiers.is_empty() {
                binder
            } else {
                quantifiers
            };
            f.write_str(":(FN ")?;
            write_quantifier_group(f, quantifiers, bounds, types, symbols)?;
            f.write_str(":{")?;
            write_param_record(f, *params, types, symbols, inner)?;
            f.write_str("} -> ")?;
            write_name_in(*ret, f, types, symbols, inner)?;
            f.write_str(")")
        }
        TypeNode::ExpressionShape {
            quantifiers,
            bounds,
            elements,
            classes,
            ret,
        } => {
            f.write_str(":(EXPR ")?;
            let shape = Ranked {
                quantifiers,
                bounds,
                elements,
                classes,
                ret: *ret,
            };
            write_shape_surface(f, shape, types, symbols)?;
            f.write_str(")")
        }
        // A quantified position renders as the name its enclosing binder bound it to. The
        // placeholder is diagnostic-only: a bare leaf outside a binder is unreachable from any
        // spelling, since the only doors that mint one are the two canonicalizing interns.
        TypeNode::Quantified { index, .. } => match binder.get(*index) {
            Some(name) => write!(f, "{}", display_symbol(name.symbol(), symbols)),
            None => write!(f, "<quantified {index}>"),
        },
        TypeNode::DeferredReturn(surface) => surface.write_surface(f, symbols),
        // `:(A | B)` — members separated by ` | ` and wrapped in the type sigil. A compound member
        // already opens its own sigil, which nests fine.
        TypeNode::Union { members } => {
            f.write_str(":(")?;
            for (index, member) in members.iter().enumerate() {
                if index > 0 {
                    f.write_str(" | ")?;
                }
                write_name_in(*member, f, types, symbols, binder)?;
            }
            f.write_str(")")
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            f.write_str(":(")?;
            write_name_in(*constructor, f, types, symbols, binder)?;
            f.write_str(" {")?;
            for (index, (name, argument)) in arguments.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{} = ", display_symbol(name.symbol(), symbols))?;
                write_name_in(argument, f, types, symbols, binder)?;
            }
            f.write_str("})")
        }
        TypeNode::Parameter { name, .. } | TypeNode::Lexical { name, .. } => {
            write!(f, "{}", display_symbol(name.symbol(), symbols))
        }
        // A sealed nominal member renders by its own member name — a bare newtype (`:Wrapper`) or a
        // per-variant member reached through its union.
        TypeNode::SetMember { name, .. } => {
            write!(f, "{}", display_symbol(name.symbol(), symbols))
        }
        // A signature names itself by its content: the empty interface is the lattice top `Module`,
        // and any other interface renders its members structurally. There is no declaration label
        // to print — two textually identical `SIG` declarations are one type, so naming either one
        // would be a lie about the other.
        TypeNode::Signature {
            schema,
            schema_digest,
        } => {
            if *schema_digest == empty_schema_digest() {
                f.write_str(MODULE_NAME.text())
            } else {
                write_sig_schema(f, *schema, types, symbols)
            }
        }
        // `<signature> WITH {Elt = Number}`, the pins in their written order.
        TypeNode::SignatureApply { signature, pins } => {
            write_name_in(*signature, f, types, symbols, binder)?;
            f.write_str(" WITH {")?;
            for (index, (name, pinned)) in pins.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{} = ", display_symbol(name.symbol(), symbols))?;
                write_name_in(pinned, f, types, symbols, binder)?;
            }
            f.write_str("}")
        }
        // Each application as it renders alone, joined by `&`; one with pins is parenthesized, as
        // a meet of applications is written.
        TypeNode::SignatureMeet { members } => {
            for (index, member) in members.iter().enumerate() {
                if index > 0 {
                    f.write_str(" & ")?;
                }
                let pinned = matches!(types.node(*member), TypeNode::SignatureApply { .. });
                if pinned {
                    f.write_str("(")?;
                }
                write_name_in(*member, f, types, symbols, binder)?;
                if pinned {
                    f.write_str(")")?;
                }
            }
            Ok(())
        }
        // Diagnostic only: a sibling reference is meaningful against its window and never survives
        // a seal.
        TypeNode::Sibling(index) => write!(f, "<sibling {index}>"),
    })
}

/// A [`display_name`] view: one handle plus the registries its content and symbols live in. The one
/// render: `Display` writes it straight into the caller's formatter, `to_string` owns it.
pub struct TypeNameDisplay<'r, 'run> {
    ktype: KType,
    types: &'r TypeRegistry<'run>,
    symbols: &'r SymbolInterner,
    binder: &'r [TypeSymbol],
}

impl<'r> TypeNameDisplay<'r, '_> {
    /// The same render under a quantifier binder, so a diagnostic about a quantified position
    /// prints the name its group gave it.
    pub fn under(self, binder: &'r [TypeSymbol]) -> Self {
        Self { binder, ..self }
    }
}

impl std::fmt::Display for TypeNameDisplay<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_name_in(self.ktype, f, self.types, self.symbols, self.binder)
    }
}

/// Surface-syntax rendering as a `Display` view — what a `format!` argument naming a type uses,
/// so the surface lands in the message's own buffer with nothing owned on the way.
pub fn display_name<'r, 'run>(
    kt: KType,
    types: &'r TypeRegistry<'run>,
    symbols: &'r SymbolInterner,
) -> TypeNameDisplay<'r, 'run> {
    TypeNameDisplay {
        ktype: kt,
        types,
        symbols,
        binder: &[],
    }
}

/// Whether this type's surface opens with the type sigil `:` — the predicate a parameter position
/// consults to decide whether to prefix one of its own, without inspecting rendered text.
pub(super) fn surface_opens_sigil(kt: KType, types: &TypeRegistry<'_>) -> bool {
    types.with_node(kt, |node| match node {
        TypeNode::List { .. }
        | TypeNode::Dict { .. }
        | TypeNode::Record { .. }
        | TypeNode::KFunction { .. }
        | TypeNode::ExpressionShape { .. }
        | TypeNode::Union { .. }
        | TypeNode::ConstructorApply { .. } => true,
        TypeNode::DeferredReturn(surface) => surface.opens_sigil(),
        _ => false,
    })
}

/// Write a record's fields as the comma-free `name :type` group the `:{…}` surface re-parses — a
/// record type's own body, and the parameter list of an `:(FN :{…} -> _)`. A leaf type surface gets
/// a `:` prefix; one that already opens a sigil is left as-is, decided by [`surface_opens_sigil`]
/// rather than by looking at text already written.
fn write_param_record(
    f: &mut std::fmt::Formatter<'_>,
    params: Record<'_>,
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
    binder: &[TypeSymbol],
) -> std::fmt::Result {
    for (index, (key, kt)) in params.iter().enumerate() {
        if index > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{} ", display_symbol(key.symbol(), symbols))?;
        if !surface_opens_sigil(kt, types) {
            f.write_str(":")?;
        }
        write_name_in(kt, f, types, symbols, binder)?;
    }
    Ok(())
}

/// `FOR ALL #{Elt: Number} #(PURE _ :Elt) -> :(Elt AS Wrap)` — an expression shape's surface below
/// the `:(EXPR …)` wrapper. The one spelling of a shape, shared by the type surface and by the head
/// a signature's rendered member is named with, so a declaration and the error naming it read
/// alike.
///
/// The group's bounds are read as the shape node stores them, one per quantifier.
fn write_shape_surface(
    f: &mut std::fmt::Formatter<'_>,
    shape: Ranked<'_>,
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
) -> std::fmt::Result {
    let quantifiers = shape.quantifiers;
    write_quantifier_group(f, quantifiers, shape.bounds, types, symbols)?;
    write_shape_head(
        f,
        shape.elements,
        shape.classes,
        types,
        symbols,
        quantifiers,
    )?;
    f.write_str(" -> ")?;
    write_name_in(shape.ret, f, types, symbols, quantifiers)
}

/// `FOR ALL #[<names>] ` — the quantifier group a binder's surface opens with, or nothing at all
/// when it quantifies over nothing. A group where some variable's bound is not `Any` is a dict of
/// each name to its bound, `FOR ALL #{Elt: Number, Key: Any} `. The trailing space is the group's,
/// so the head that follows spells the same either way.
fn write_quantifier_group(
    f: &mut std::fmt::Formatter<'_>,
    quantifiers: &[TypeSymbol],
    bounds: &[KType],
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
) -> std::fmt::Result {
    if quantifiers.is_empty() {
        return Ok(());
    }
    let bound_of = |index: usize| bounds.get(index).copied().unwrap_or(KType::ANY);
    let bounded = (0..quantifiers.len()).any(|index| bound_of(index) != KType::ANY);
    f.write_str(if bounded { "FOR ALL #{" } else { "FOR ALL #[" })?;
    for (index, name) in quantifiers.iter().enumerate() {
        if index > 0 {
            f.write_str(if bounded { ", " } else { " " })?;
        }
        write!(f, "{}", display_symbol(name.symbol(), symbols))?;
        if bounded {
            f.write_str(": ")?;
            write_name_in(bound_of(index), f, types, symbols, &[])?;
        }
    }
    f.write_str(if bounded { "} " } else { "] " })
}

/// `#(<keyword> _ :<Type> …)` — an expression shape's head, quoted as an `EXPR` head is written.
/// Every argument position is the wildcard `_`, as the type carries no argument names — or, in a
/// ranked shape, its priority class counted from 1, as a `SIG` member writes it (`#(MOVE 2 :Any TO
/// 1 :Any)`).
fn write_shape_head(
    f: &mut std::fmt::Formatter<'_>,
    elements: &[DispatchTokenElement],
    classes: &[u8],
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
    binder: &[TypeSymbol],
) -> std::fmt::Result {
    f.write_str("#(")?;
    let mut slot = 0;
    for (index, element) in elements.iter().enumerate() {
        if index > 0 {
            f.write_str(" ")?;
        }
        match element {
            DispatchTokenElement::Keyword(symbol) => {
                write!(f, "{}", display_symbol(symbol.symbol(), symbols))?;
            }
            DispatchTokenElement::Slot(kt) => {
                match classes.get(slot) {
                    Some(class) => write!(f, "{} ", u32::from(*class) + 1)?,
                    None => f.write_str("_ ")?,
                }
                slot += 1;
                if !surface_opens_sigil(*kt, types) {
                    f.write_str(":")?;
                }
                write_name_in(*kt, f, types, symbols, binder)?;
            }
        }
    }
    f.write_str(")")
}

/// The structural rendering of a non-empty interface: `SIG FOR ALL #{Elt: Bound} (member: Type, …)`,
/// its head parameters in the group as a `SIG` declares them and every other member the schema
/// names in the parentheses — manifest members, then value slots, each table in its stored symbol
/// order, which is deterministic across runs because a symbol is a digest of its text.
fn write_sig_schema(
    f: &mut std::fmt::Formatter<'_>,
    schema: SigSchema<'_>,
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
) -> std::fmt::Result {
    let members = schema
        .manifest_members
        .iter()
        .map(|(name, kt)| (name.symbol(), *kt))
        .chain(
            schema
                .value_slots
                .iter()
                .map(|(name, kt)| (name.symbol(), *kt)),
        );
    f.write_str("SIG ")?;
    if !schema.parameters.is_empty() {
        f.write_str("FOR ALL #{")?;
        for (index, (name, parameter)) in schema.parameters.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            let bound = types.node(*parameter).rigid_bound().unwrap_or(KType::ANY);
            write!(
                f,
                "{}: {}",
                display_symbol(name.symbol(), symbols),
                display_name(bound, types, symbols)
            )?;
        }
        f.write_str("} ")?;
    }
    f.write_str("(")?;
    let mut written = 0;
    for (name, kt) in members {
        if written > 0 {
            f.write_str(", ")?;
        }
        write!(
            f,
            "{}: {}",
            display_symbol(name, symbols),
            display_name(kt, types, symbols)
        )?;
        written += 1;
    }
    // Keyworded members follow the named ones, each as the head declaring it. They are named by a
    // call shape rather than by a name, so they follow the schema's canonical member order. A unary
    // triple's bridge entry names the same head as its list entry, so one of the two is dropped:
    // printing it twice would spell an interface no signature can be written to declare.
    let mut heads: Vec<String> = Vec::new();
    for member in schema.keyworded {
        let head = render_keyworded_head(*member, schema.operators, types, symbols);
        if !heads.contains(&head) {
            heads.push(head);
        }
    }
    for head in heads {
        if written > 0 {
            f.write_str(", ")?;
        }
        f.write_str(&head)?;
        written += 1;
    }
    // The chaining records follow the members, each as the `GROUP` head declaring it. A record one
    // of its own members' heads already spells in full renders nothing.
    for group in schema.operators {
        let Some(head) = render_declared_group(group, symbols) else {
            continue;
        };
        if written > 0 {
            f.write_str(", ")?;
        }
        f.write_str(&head)?;
        written += 1;
    }
    f.write_str(")")
}

/// Render a keyworded member as the head declaring it — `#(PURE _ :Number) -> Number`, the `EXPR`
/// head minus its keyword and its type sigil.
///
/// The one diagnostic currency for a keyworded member: the subtyping failures name a head with it,
/// and a schema's rendered members read from the same spelling, so a declaration and the error
/// naming it read alike.
pub fn render_keyworded_head(
    shape: KType,
    operators: &[DeclaredGroup<'_>],
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
) -> String {
    if let Some(head) = render_operator_head(shape, operators, types, symbols) {
        return head;
    }
    match Ranked::of(types, shape) {
        Some(shape) => ShapeSurface {
            shape,
            types,
            symbols,
        }
        .to_string(),
        None => display_name(shape, types, symbols).to_string(),
    }
}

/// [`write_shape_surface`] as a `Display` view, for the diagnostics that keep the text.
struct ShapeSurface<'r, 'run> {
    shape: Ranked<'run>,
    types: &'r TypeRegistry<'run>,
    symbols: &'r SymbolInterner,
}

impl std::fmt::Display for ShapeSurface<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_shape_surface(f, self.shape, self.types, self.symbols)
    }
}

/// The operator-surface reading of a keyworded member, or `None` when the member is not one.
///
/// A member is an operator member iff its key is one of the two an operator declaration writes —
/// `[Slot, Keyword(s), Slot]` or `[Keyword(s), Slot]` — **and** `s` is a member of one of the
/// schema's declared chaining records. That second half is what keeps the plain `EXPR`-head
/// spelling of an operator key reading as an `EXPR` head.
fn render_operator_head(
    shape: KType,
    operators: &[DeclaredGroup<'_>],
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
) -> Option<String> {
    let (symbol, is_list_form) = match shape_elements(&types.node(shape)) {
        [
            DispatchTokenElement::Slot(_),
            DispatchTokenElement::Keyword(symbol),
            DispatchTokenElement::Slot(_),
        ] => Some((*symbol, false)),
        [
            DispatchTokenElement::Keyword(symbol),
            DispatchTokenElement::Slot(_),
        ] => Some((*symbol, true)),
        _ => None,
    }?;
    let mode = operators
        .iter()
        .find(|record| record.members.contains(&symbol))?
        .mode;
    let ret = shape_return(shape, types)?;
    let first_slot = shape_slots(shape, types).next()?;
    // The list form's sole parameter is the whole run, so the declared operand is its element.
    let operand = if is_list_form {
        match types.node(first_slot) {
            TypeNode::List { element } => element,
            _ => return None,
        }
    } else {
        first_slot
    };
    let symbol = display_symbol(symbol.symbol(), symbols);
    let operand = display_name(operand, types, symbols);
    Some(if mode == ReductionMode::Unary {
        format!(
            "UNARY OP #({symbol}) OVER {operand} -> {}",
            display_name(ret, types, symbols)
        )
    } else if ret == first_slot {
        // A fold member's result is its operand type, which the bare head already says.
        format!("OP #({symbol}) OVER {operand}")
    } else {
        format!(
            "OP #({symbol}) OVER {operand} -> {}",
            display_name(ret, types, symbols)
        )
    })
}

/// Render one declared record as the `GROUP` head declaring it — `GROUP FOLD RIGHT {+ -}`.
///
/// `None` for a record one of its own members' heads already spells in full: a bare `OP` head
/// declares exactly a fold-left singleton, and a `UNARY OP` head exactly a unary one, so rendering
/// those again would print one declaration twice.
pub(super) fn render_declared_group(
    group: &DeclaredGroup<'_>,
    symbols: &SymbolInterner,
) -> Option<String> {
    let singleton = group.members.len() == 1;
    if singleton && matches!(group.mode, ReductionMode::FoldLeft | ReductionMode::Unary) {
        return None;
    }
    let mut head = String::from("GROUP ");
    match group.mode {
        ReductionMode::Unary => head.push_str("UNARY "),
        ReductionMode::FoldLeft => head.push_str("FOLD LEFT "),
        ReductionMode::FoldRight => head.push_str("FOLD RIGHT "),
        ReductionMode::Pairwise {
            combiner,
            direction,
        } => {
            let _ = write!(
                head,
                "PAIRWISE FOLD #({}) {} ",
                display_symbol(combiner.symbol(), symbols),
                match direction {
                    FoldDirection::Left => "LEFT",
                    FoldDirection::Right => "RIGHT",
                }
            );
        }
    }
    head.push('{');
    for (index, member) in group.members.iter().enumerate() {
        if index > 0 {
            head.push(' ');
        }
        let _ = write!(head, "{}", display_symbol(member.symbol(), symbols));
    }
    head.push('}');
    Some(head)
}

/// The member run of every declared record, joined for a diagnostic that names a record by its
/// members alone.
fn render_members(members: &[KeywordSymbol], symbols: &SymbolInterner) -> String {
    members
        .iter()
        .map(|member| render_symbol(member.symbol(), symbols))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A chaining mode's surface spelling, for the two operator satisfaction diagnostics.
fn render_mode(mode: ReductionMode, symbols: &SymbolInterner) -> String {
    match mode {
        ReductionMode::Unary => "as a unary run".to_string(),
        ReductionMode::FoldLeft => "folding left".to_string(),
        ReductionMode::FoldRight => "folding right".to_string(),
        ReductionMode::Pairwise {
            combiner,
            direction,
        } => format!(
            "pairwise through `{}`, folding {}",
            display_symbol(combiner.symbol(), symbols),
            match direction {
                FoldDirection::Left => "left",
                FoldDirection::Right => "right",
            }
        ),
    }
}

/// Render a *fits* failure as the message fragment an ascription error embeds after
/// `` module does not satisfy signature `{path}`: ``.
///
/// `operators` is the declaring side's chaining channel, so a keyworded head renders as the `OP`
/// surface that declared it rather than as a bare `EXPR` head.
pub fn render_fits_failure(
    failure: &FitsFailure<'_, '_>,
    operators: &[DeclaredGroup<'_>],
    types: &TypeRegistry<'_>,
    symbols: &SymbolInterner,
) -> String {
    let head = |shape: KType| render_keyworded_head(shape, operators, types, symbols);
    let show = |kt: KType| display_name(kt, types, symbols);
    match failure {
        FitsFailure::MissingTypeMember { name } => {
            format!(
                "missing type member `{}`",
                render_symbol(name.symbol(), symbols)
            )
        }
        FitsFailure::ManifestMismatch {
            name,
            got,
            expected,
        } => format!(
            "type member `{}` is `{}` but the signature fixes it to `{}`",
            render_symbol(name.symbol(), symbols),
            show(*got),
            show(*expected)
        ),
        FitsFailure::MissingValueSlot { name } => {
            format!("missing member `{}`", render_symbol(name.symbol(), symbols))
        }
        FitsFailure::ValueSlotMismatch {
            name,
            got,
            expected,
        } => format!(
            "member `{}` has type `{}` but the signature declares `{}`",
            render_symbol(name.symbol(), symbols),
            show(*got),
            show(*expected)
        ),
        FitsFailure::MissingKeyworded { head: shape } => {
            format!("missing keyworded member `{}`", head(*shape))
        }
        FitsFailure::RankingMismatch { head: shape, got } => format!(
            "keyworded member `{}` is ranked two ways (found `{}`)",
            head(*shape),
            head(*got)
        ),
        FitsFailure::KeywordedMismatch { head: shape, got } => format!(
            "no overload satisfies keyworded member `{}` (found {})",
            head(*shape),
            got.iter()
                .map(|one| format!("`{}`", head(*one)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        FitsFailure::QuantifiedMismatch {
            head: shape,
            parameter,
            got,
        } => format!(
            "keyworded member `{}` quantifies over `{parameter_name}`, but the overload fixes that \
             position to `{}` — one implementation must hold at every `{parameter_name}`",
            head(*shape),
            show(*got),
            parameter_name = render_symbol(parameter.symbol(), symbols)
        ),
        FitsFailure::MissingOperatorGroup { members } => format!(
            "no chaining mode covers `{}` (the module defines the buckets but declares no group \
             over them)",
            render_members(members, symbols)
        ),
        FitsFailure::OperatorModeMismatch {
            members,
            expected,
            got,
        } => format!(
            "operators `{}` chain {} in the signature but {} in the module",
            render_members(members, symbols),
            render_mode(*expected, symbols),
            render_mode(*got, symbols)
        ),
        FitsFailure::Unsolved { parameter } => format!(
            "what the module offers solves parameter `{}` to no type",
            render_symbol(parameter.symbol(), symbols)
        ),
        FitsFailure::SelfSignature { expected } => format!(
            "only the module whose own signature is `{}` fits it",
            show(*expected)
        ),
    }
}
