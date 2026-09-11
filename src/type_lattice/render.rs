//! Surface-syntax rendering — the one recursion in the lattice written by hand.
//!
//! Rendering spells syntax *between* children and inherits the quantifier binder from above, which
//! a fold cannot express, so it is exempt from the driver rule: one exhaustive match per surface,
//! the binder threaded top-down, and a new node variant must be spelled here.
//!
//! Every entry point takes the registry and the label interner, never a bundle: the lattice knows
//! about types and labels and nothing else.

use std::fmt::Write as _;

use smallvec::SmallVec;

use crate::parse::{LabelDisplay, LabelInterner, Symbol, TypeSymbol};

use super::digest::empty_schema_digest;
use super::handle::{
    ANY_NAME, BOOL_NAME, IDENTIFIER_NAME, KEXPRESSION_NAME, KType, MODULE_NAME, NAME_TOKEN_NAME,
    NEVER_NAME, NULL_NAME, NUMBER_NAME, RECORD_TYPE_NAME, SIGILED_TYPE_EXPR_NAME, STR_NAME,
    TYPE_NAME_TOKEN_NAME,
};
use super::node::TypeNode;
use super::operators::{FoldDirection, ReductionMode};
use super::record::Record;
use super::registry::TypeRegistry;
use super::schema::{DeclaredGroup, OperatorMembers, SigSchema, shape_elements, shape_slots};
use super::shape::DispatchTokenElement;
use super::sig_relations::SigSubtypeFailure;
use super::substitute::quantifier_bounds;

/// A label's text, resolved through the run's interner. Rendering stays total: a miss prints a
/// placeholder rather than panicking, because error formatting must never be the thing that fails.
pub fn render_label(symbol: Symbol, labels: &LabelInterner) -> String {
    labels.render(symbol)
}

/// [`render_label`] as a `Display` view, so the text lands in the message's buffer without a
/// `String` of its own on the way.
pub fn display_label(symbol: Symbol, labels: &LabelInterner) -> LabelDisplay<'_> {
    labels.display(symbol)
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
    types: &TypeRegistry,
    labels: &LabelInterner,
    binder: &[TypeSymbol],
) -> std::fmt::Result {
    types.with_node(kt, |node| match node {
        TypeNode::Number => f.write_str(NUMBER_NAME.text()),
        TypeNode::Str => f.write_str(STR_NAME.text()),
        TypeNode::Bool => f.write_str(BOOL_NAME.text()),
        TypeNode::Null => f.write_str(NULL_NAME.text()),
        TypeNode::Identifier => f.write_str(IDENTIFIER_NAME.text()),
        TypeNode::NameToken => f.write_str(NAME_TOKEN_NAME.text()),
        TypeNode::TypeNameToken => f.write_str(TYPE_NAME_TOKEN_NAME.text()),
        TypeNode::KExpression => f.write_str(KEXPRESSION_NAME.text()),
        TypeNode::SigiledTypeExpr => f.write_str(SIGILED_TYPE_EXPR_NAME.text()),
        TypeNode::RecordType => f.write_str(RECORD_TYPE_NAME.text()),
        TypeNode::Any => f.write_str(ANY_NAME.text()),
        TypeNode::Never => f.write_str(NEVER_NAME.text()),
        TypeNode::OfKind(kind) => f.write_str(kind.surface_keyword()),
        TypeNode::List { element } => {
            f.write_str(":(LIST OF ")?;
            write_name_in(*element, f, types, labels, binder)?;
            f.write_str(")")
        }
        TypeNode::Dict { key, value } => {
            f.write_str(":(MAP ")?;
            write_name_in(*key, f, types, labels, binder)?;
            f.write_str(" -> ")?;
            write_name_in(*value, f, types, labels, binder)?;
            f.write_str(")")
        }
        // `:{x :Number y :Str}` — the braced type-sigil surface. Fields render space-separated like
        // FN params, which the field-list parser accepts.
        TypeNode::Record { fields } => {
            f.write_str(":{")?;
            write_param_record(f, fields, types, labels, binder)?;
            f.write_str("}")
        }
        TypeNode::KFunction { params, ret } => {
            f.write_str(":(FN :{")?;
            write_param_record(f, params, types, labels, binder)?;
            f.write_str("} -> ")?;
            write_name_in(*ret, f, types, labels, binder)?;
            f.write_str(")")
        }
        TypeNode::ExpressionShape {
            quantifiers,
            elements,
            ret,
        } => {
            f.write_str(":(EXPR ")?;
            write_shape_surface(f, kt, quantifiers, elements, *ret, types, labels)?;
            f.write_str(")")
        }
        // A quantified position renders as the name its enclosing shape bound it to. The
        // placeholder is diagnostic-only: a bare leaf outside a shape is unreachable from any
        // spelling, since the only door that mints one is the shape builder.
        TypeNode::Quantified { index, .. } => match binder.get(*index) {
            Some(name) => write!(f, "{}", display_label(name.symbol(), labels)),
            None => write!(f, "<quantified {index}>"),
        },
        TypeNode::DeferredReturn(surface) => surface.write_surface(f, labels),
        // `:(A | B)` — members separated by ` | ` and wrapped in the type sigil. A compound member
        // already opens its own sigil, which nests fine.
        TypeNode::Union { members } => {
            f.write_str(":(")?;
            for (index, member) in members.iter().enumerate() {
                if index > 0 {
                    f.write_str(" | ")?;
                }
                write_name_in(*member, f, types, labels, binder)?;
            }
            f.write_str(")")
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            f.write_str(":(")?;
            write_name_in(*constructor, f, types, labels, binder)?;
            f.write_str(" {")?;
            for (index, (name, argument)) in arguments.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{} = ", display_label(name.symbol(), labels))?;
                write_name_in(*argument, f, types, labels, binder)?;
            }
            f.write_str("})")
        }
        TypeNode::AbstractType { name, .. } => {
            write!(f, "{}", display_label(name.symbol(), labels))
        }
        // A sealed nominal member renders by its own member name — a bare newtype (`:Wrapper`) or a
        // per-variant member reached through its union.
        TypeNode::SetMember { name, .. } => {
            write!(f, "{}", display_label(name.symbol(), labels))
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
                write_sig_schema(f, schema, types, labels)
            }
        }
        // Diagnostic only: a sibling reference is meaningful against its window and never survives
        // a seal.
        TypeNode::Sibling(index) => write!(f, "<sibling {index}>"),
    })
}

/// A [`display_name`] view: one handle plus the registries its content and labels live in. The one
/// render: `Display` writes it straight into the caller's formatter, `to_string` owns it.
pub struct TypeNameDisplay<'r> {
    ktype: KType,
    types: &'r TypeRegistry,
    labels: &'r LabelInterner,
    binder: &'r [TypeSymbol],
}

impl<'r> TypeNameDisplay<'r> {
    /// The same render under a quantifier binder, so a diagnostic about a quantified position
    /// prints the name its group gave it.
    pub fn under(self, binder: &'r [TypeSymbol]) -> Self {
        Self { binder, ..self }
    }
}

impl std::fmt::Display for TypeNameDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_name_in(self.ktype, f, self.types, self.labels, self.binder)
    }
}

/// Surface-syntax rendering as a `Display` view — what a `format!` argument naming a type uses,
/// so the surface lands in the message's own buffer with nothing owned on the way.
pub fn display_name<'r>(
    kt: KType,
    types: &'r TypeRegistry,
    labels: &'r LabelInterner,
) -> TypeNameDisplay<'r> {
    TypeNameDisplay {
        ktype: kt,
        types,
        labels,
        binder: &[],
    }
}

/// Whether this type's surface opens with the type sigil `:` — the predicate a parameter position
/// consults to decide whether to prefix one of its own, without inspecting rendered text.
pub(super) fn surface_opens_sigil(kt: KType, types: &TypeRegistry) -> bool {
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
    params: &Record<KType>,
    types: &TypeRegistry,
    labels: &LabelInterner,
    binder: &[TypeSymbol],
) -> std::fmt::Result {
    for (index, (key, kt)) in params.iter().enumerate() {
        if index > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{} ", display_label(key.symbol(), labels))?;
        if !surface_opens_sigil(*kt, types) {
            f.write_str(":")?;
        }
        write_name_in(*kt, f, types, labels, binder)?;
    }
    Ok(())
}

/// `FOR ALL (Elt :Number) (PURE _ :Elt) -> :(Elt AS Wrap)` — an expression shape's surface below
/// the `:(EXPR …)` wrapper. The one spelling of a shape, shared by the type surface and by the head
/// a signature's rendered member is named with, so a declaration and the error naming it read
/// alike.
///
/// `shape` is the handle the group's bounds are read off: a variable's bound rides on its own
/// occurrences, so the group is spelled from the interned node rather than carried beside it.
pub(super) fn write_shape_surface(
    f: &mut std::fmt::Formatter<'_>,
    shape: KType,
    quantifiers: &[TypeSymbol],
    elements: &[DispatchTokenElement],
    ret: KType,
    types: &TypeRegistry,
    labels: &LabelInterner,
) -> std::fmt::Result {
    write_quantifier_group(f, shape, quantifiers, types, labels)?;
    write_shape_head(f, elements, types, labels, quantifiers)?;
    f.write_str(" -> ")?;
    write_name_in(ret, f, types, labels, quantifiers)
}

/// `FOR ALL (<names>) ` — the quantifier group a shape's surface opens with, or nothing at all when
/// the shape quantifies over nothing. A variable whose bound is not `Any` spells it: `Elt :Number`.
/// The trailing space is the group's, so the head that follows spells the same either way.
fn write_quantifier_group(
    f: &mut std::fmt::Formatter<'_>,
    shape: KType,
    quantifiers: &[TypeSymbol],
    types: &TypeRegistry,
    labels: &LabelInterner,
) -> std::fmt::Result {
    if quantifiers.is_empty() {
        return Ok(());
    }
    let bounds = quantifier_bounds(types, shape);
    f.write_str("FOR ALL (")?;
    for (index, name) in quantifiers.iter().enumerate() {
        if index > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{}", display_label(name.symbol(), labels))?;
        match bounds.get(index) {
            Some(bound) if *bound != KType::ANY => {
                if !surface_opens_sigil(*bound, types) {
                    f.write_str(" :")?;
                } else {
                    f.write_str(" ")?;
                }
                write_name_in(*bound, f, types, labels, &[])?;
            }
            _ => {}
        }
    }
    f.write_str(") ")
}

/// `(<keyword> _ :<Type> …)` — an expression shape's head. Every argument position is the wildcard
/// `_`: the type carries no argument names, so there is none to print.
fn write_shape_head(
    f: &mut std::fmt::Formatter<'_>,
    elements: &[DispatchTokenElement],
    types: &TypeRegistry,
    labels: &LabelInterner,
    binder: &[TypeSymbol],
) -> std::fmt::Result {
    f.write_str("(")?;
    for (index, element) in elements.iter().enumerate() {
        if index > 0 {
            f.write_str(" ")?;
        }
        match element {
            DispatchTokenElement::Keyword(symbol) => {
                write!(f, "{}", display_label(symbol.symbol(), labels))?;
            }
            DispatchTokenElement::Slot(kt) => {
                f.write_str("_ ")?;
                if !surface_opens_sigil(*kt, types) {
                    f.write_str(":")?;
                }
                write_name_in(*kt, f, types, labels, binder)?;
            }
        }
    }
    f.write_str(")")
}

/// The structural rendering of a non-empty interface: `SIG (member: Type, …)` over every member the
/// schema names — abstract, manifest and value slot alike — in member-name order, which is the only
/// order the schema's unordered maps admit deterministically.
fn write_sig_schema(
    f: &mut std::fmt::Formatter<'_>,
    schema: &SigSchema,
    types: &TypeRegistry,
    labels: &LabelInterner,
) -> std::fmt::Result {
    // Presentation order is alphabetical by member *text*, compared in the interner rather than
    // rendered first. The digest sorts by symbol instead — identity needs a canonical order, not a
    // readable one. Eight members inline covers every interface the tree declares.
    let mut members: SmallVec<[(Symbol, KType); 8]> = schema
        .abstract_members
        .iter()
        .chain(schema.manifest_members.iter())
        .map(|(name, kt)| (name.symbol(), *kt))
        .chain(
            schema
                .value_slots
                .iter()
                .map(|(name, kt)| (name.symbol(), *kt)),
        )
        .collect();
    members.sort_by(|a, b| labels.compare_texts(a.0, b.0));
    f.write_str("SIG (")?;
    for (index, (name, kt)) in members.iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        write!(
            f,
            "{}: {}",
            display_label(*name, labels),
            display_name(*kt, types, labels)
        )?;
    }
    // Keyworded members follow the named ones, each as the head declaring it. They are named by a
    // call shape rather than by a name, so they follow the schema's canonical member order. A unary
    // triple's bridge entry names the same head as its list entry, so one of the two is dropped:
    // printing it twice would spell an interface no signature can be written to declare.
    let mut written = members.len();
    let mut heads: Vec<String> = Vec::new();
    for member in &schema.keyworded {
        let head = render_keyworded_head(*member, &schema.operators, types, labels);
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
    for group in &schema.operators {
        let Some(head) = render_declared_group(group, labels) else {
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

/// Render a keyworded member as the head declaring it — `(PURE _ :Number) -> Number`, the `EXPR`
/// head minus its keyword and its type sigil.
///
/// The one diagnostic currency for a keyworded member: the subtyping failures name a head with it,
/// and a schema's rendered members read from the same spelling, so a declaration and the error
/// naming it read alike.
pub fn render_keyworded_head(
    shape: KType,
    operators: &OperatorMembers,
    types: &TypeRegistry,
    labels: &LabelInterner,
) -> String {
    if let Some(head) = render_operator_head(shape, operators, types, labels) {
        return head;
    }
    // Owns: the surface is written after the read closes, so it cannot borrow the node.
    let read: Option<(Vec<TypeSymbol>, Vec<DispatchTokenElement>, KType)> =
        types.with_node(shape, |node| match node {
            TypeNode::ExpressionShape {
                quantifiers,
                elements,
                ret,
            } => Some((quantifiers.clone(), elements.to_vec(), *ret)),
            _ => None,
        });
    match read {
        Some((quantifiers, elements, ret)) => ShapeSurface {
            shape,
            quantifiers: &quantifiers,
            elements: &elements,
            ret,
            types,
            labels,
        }
        .to_string(),
        None => display_name(shape, types, labels).to_string(),
    }
}

/// [`write_shape_surface`] as a `Display` view, for the diagnostics that keep the text.
struct ShapeSurface<'r> {
    shape: KType,
    quantifiers: &'r [TypeSymbol],
    elements: &'r [DispatchTokenElement],
    ret: KType,
    types: &'r TypeRegistry,
    labels: &'r LabelInterner,
}

impl std::fmt::Display for ShapeSurface<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_shape_surface(
            f,
            self.shape,
            self.quantifiers,
            self.elements,
            self.ret,
            self.types,
            self.labels,
        )
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
    operators: &OperatorMembers,
    types: &TypeRegistry,
    labels: &LabelInterner,
) -> Option<String> {
    let (symbol, is_list_form) = types.with_node(shape, |node| match shape_elements(node) {
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
    })?;
    let mode = operators
        .iter()
        .find(|record| record.members.contains(&symbol))?
        .mode;
    let slots = shape_slots(shape, types);
    let ret = super::schema::shape_return(shape, types)?;
    let first_slot = *slots.first()?;
    // The list form's sole parameter is the whole run, so the declared operand is its element.
    let operand = if is_list_form {
        types.with_node(first_slot, |node| match node {
            TypeNode::List { element } => Some(*element),
            _ => None,
        })?
    } else {
        first_slot
    };
    let symbol = display_label(symbol.symbol(), labels);
    let operand = display_name(operand, types, labels);
    Some(if mode == ReductionMode::Unary {
        format!(
            "UNARY OP #({symbol}) OVER {operand} -> {}",
            display_name(ret, types, labels)
        )
    } else if ret == first_slot {
        // A fold member's result is its operand type, which the bare head already says.
        format!("OP #({symbol}) OVER {operand}")
    } else {
        format!(
            "OP #({symbol}) OVER {operand} -> {}",
            display_name(ret, types, labels)
        )
    })
}

/// Render one declared record as the `GROUP` head declaring it — `GROUP FOLD RIGHT {+ -}`.
///
/// `None` for a record one of its own members' heads already spells in full: a bare `OP` head
/// declares exactly a fold-left singleton, and a `UNARY OP` head exactly a unary one, so rendering
/// those again would print one declaration twice.
pub(super) fn render_declared_group(
    group: &DeclaredGroup,
    labels: &LabelInterner,
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
                display_label(combiner.symbol(), labels),
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
        let _ = write!(head, "{}", display_label(member.symbol(), labels));
    }
    head.push('}');
    Some(head)
}

/// The member run of every declared record, joined for a diagnostic that names a record by its
/// members alone.
fn render_members(members: &[crate::parse::KeywordSymbol], labels: &LabelInterner) -> String {
    members
        .iter()
        .map(|member| render_label(member.symbol(), labels))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A chaining mode's surface spelling, for the two operator satisfaction diagnostics.
fn render_mode(mode: ReductionMode, labels: &LabelInterner) -> String {
    match mode {
        ReductionMode::Unary => "as a unary run".to_string(),
        ReductionMode::FoldLeft => "folding left".to_string(),
        ReductionMode::FoldRight => "folding right".to_string(),
        ReductionMode::Pairwise {
            combiner,
            direction,
        } => format!(
            "pairwise through `{}`, folding {}",
            display_label(combiner.symbol(), labels),
            match direction {
                FoldDirection::Left => "left",
                FoldDirection::Right => "right",
            }
        ),
    }
}

/// Render a signature-subtyping failure as the message fragment an ascription error embeds after
/// `` module does not satisfy signature `{path}`: ``.
///
/// `operators` is the declaring side's chaining channel, so a keyworded head renders as the `OP`
/// surface that declared it rather than as a bare `EXPR` head.
pub fn render_sig_failure(
    failure: &SigSubtypeFailure,
    operators: &OperatorMembers,
    types: &TypeRegistry,
    labels: &LabelInterner,
) -> String {
    let head = |shape: KType| render_keyworded_head(shape, operators, types, labels);
    let show = |kt: KType| display_name(kt, types, labels);
    match failure {
        SigSubtypeFailure::MissingTypeMember { name } => {
            format!(
                "missing type member `{}`",
                render_label(name.symbol(), labels)
            )
        }
        SigSubtypeFailure::ManifestMismatch {
            name,
            got,
            expected,
        } => format!(
            "type member `{}` is `{}` but the signature fixes it to `{}`",
            render_label(name.symbol(), labels),
            show(*got),
            show(*expected)
        ),
        SigSubtypeFailure::KindMismatch {
            name,
            expected_params: Some(params),
            got,
        } => {
            let mut sorted: Vec<String> = params
                .iter()
                .map(|p| render_label(p.symbol(), labels))
                .collect();
            sorted.sort_unstable();
            format!(
                "type member `{}` must be a type constructor with parameters {{{}}}, got `{}`",
                render_label(name.symbol(), labels),
                sorted.join(", "),
                show(*got)
            )
        }
        SigSubtypeFailure::KindMismatch {
            name,
            expected_params: None,
            got,
        } => format!(
            "type member `{}` must be a proper type, got the type constructor `{}`",
            render_label(name.symbol(), labels),
            show(*got)
        ),
        SigSubtypeFailure::BoundMismatch {
            name,
            got,
            expected,
        } => format!(
            "type member `{}` is `{}` but the signature bounds it by `{}`",
            render_label(name.symbol(), labels),
            show(*got),
            show(*expected)
        ),
        SigSubtypeFailure::MissingValueSlot { name } => {
            format!("missing member `{}`", render_label(name.symbol(), labels))
        }
        SigSubtypeFailure::ValueSlotMismatch {
            name,
            got,
            expected,
        } => format!(
            "member `{}` has type `{}` but the signature declares `{}`",
            render_label(name.symbol(), labels),
            show(*got),
            show(*expected)
        ),
        SigSubtypeFailure::MissingKeyworded { head: shape } => {
            format!("missing keyworded member `{}`", head(*shape))
        }
        SigSubtypeFailure::KeywordedMismatch { head: shape, got } => format!(
            "no overload satisfies keyworded member `{}` (found {})",
            head(*shape),
            got.iter()
                .map(|one| format!("`{}`", head(*one)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        SigSubtypeFailure::QuantifiedMismatch {
            head: shape,
            parameter,
            got,
        } => format!(
            "keyworded member `{}` quantifies over `{parameter_name}`, but the overload fixes that \
             position to `{}` — one implementation must hold at every `{parameter_name}`",
            head(*shape),
            show(*got),
            parameter_name = render_label(parameter.symbol(), labels)
        ),
        SigSubtypeFailure::AmbiguousKeyworded {
            head: shape,
            candidates,
        } => format!(
            "keyworded member `{}` is satisfied by {} with no most specific one",
            head(*shape),
            candidates
                .iter()
                .map(|one| format!("`{}`", head(*one)))
                .collect::<Vec<_>>()
                .join(" and ")
        ),
        SigSubtypeFailure::MissingOperatorGroup { members } => format!(
            "no chaining mode covers `{}` (the module defines the buckets but declares no group \
             over them)",
            render_members(members, labels)
        ),
        SigSubtypeFailure::OperatorModeMismatch {
            members,
            expected,
            got,
        } => format!(
            "operators `{}` chain {} in the signature but {} in the module",
            render_members(members, labels),
            render_mode(*expected, labels),
            render_mode(*got, labels)
        ),
    }
}
