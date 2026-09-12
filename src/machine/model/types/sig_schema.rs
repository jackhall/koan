//! The signature-subtyping relation and the schema it is defined over.
//!
//! A [`SigSchema`] is the normalized carrier of a signature's shape — the abstract type
//! members, the manifest (fixed) type members, and the value slots — projected out of either
//! a SIG declaration ([`SigSchema::project_decl`]) or a [`Module`]'s own body
//! ([`SigSchema::raw_self_sig`], the self-sig). [`sig_subtype`] is the canonical relation:
//! `Sub <: Super` iff `Sub` supplies every member `Super` names, with each manifest member
//! equal, each abstract member present at the right kind and over the same parameter names, and
//! each value slot covariantly compatible after abstract-member substitution.
//!
//! A `SigSchema` is what the `Signature` [`TypeNode`] owns; the node computes and stores the
//! schema's content digest once at intern time, so the schema itself carries no digest field.
//!
//! See [old_design/typing/modules.md](../../../../old_design/typing/modules.md).

use std::borrow::Cow;
use std::collections::HashMap;

use crate::machine::core::Scope;
use crate::memory::ScopeId;

use super::kkind::KKind;
use super::ktype::KType;
use super::node::{NodeSchema, TypeNode};
use super::registry::TypeRegistry;
use super::signature::{DispatchTokenElement, Specificity, shape_specificity};
use crate::machine::model::RunRegistries;
use crate::machine::model::values::ModuleDraft;
use crate::machine::model::{FoldDirection, ReductionMode};
use crate::machine::model::{display_label, render_label};
use crate::parse::ast::KeyElement;
use crate::parse::{IdentityBuildHasher, KeywordSymbol, TypeSymbol, ValueSymbol};

/// A schema's type-member table: Type-class name → the member's type, identity-hashed on the
/// symbol's digest bits.
pub type TypeMemberMap = HashMap<TypeSymbol, KType, IdentityBuildHasher>;

/// Normalized signature schema — the carrier the subtyping relation is defined over.
///
/// Members are split by *representation*, not by surface syntax: an abstract member carries no
/// concrete witness (an `AbstractType` node, of either order), a manifest member fixes a
/// concrete type. A module self-sig never has abstract members — `TYPE` is a SIG-body-only
/// construct.
#[derive(Clone)]
pub struct SigSchema {
    /// The binder this schema's own abstract members are sourced at: `Some(ScopeId::SENTINEL)`
    /// for a SIG declaration, `None` for a module self-sig (whose slot types name no
    /// SIG-declared refs).
    ///
    /// The binder is *canonical*, not the declaring scope's id: [`Self::project_decl`] rewrites
    /// every SIG-own member's `source` to [`ScopeId::SENTINEL`] as it projects, so two textually
    /// identical `SIG` declarations project to one schema and intern to one type. `SENTINEL` is
    /// never a minted scope id, so a canonical binder cannot alias a real one, and the
    /// substitution and comparison walks below keep testing `source == sig_id` unchanged.
    pub sig_id: Option<ScopeId>,
    /// Abstract type members: name → the bound `AbstractType` as found in the decl scope. Its
    /// `param_names` carry the member's order (empty = first-order, non-empty = a constructor
    /// over those parameters), read on demand through [`constructor_param_names`].
    pub abstract_members: TypeMemberMap,
    /// Manifest type members: name → the fixed type.
    pub manifest_members: TypeMemberMap,
    /// Value slots: name → declared (SIG) or derived (self-sig) type.
    pub value_slots: HashMap<ValueSymbol, KType, IdentityBuildHasher>,
    /// Keyworded (dispatch-bucket) members: the expression shapes the interface declares, in
    /// [`canonical_overloads`] order so equality, digesting and iteration are deterministic. The
    /// bucket key is each member's own element run with its slot types erased ([`shape_keys_equal`],
    /// [`shape_key_is`]) — read off the member's type, never stored
    /// beside it — so two overloads under one key are two entries here and an exact duplicate is
    /// illegal at declaration.
    pub keyworded: Vec<KType>,
    /// Operator members: the chaining records the interface declares, in [`canonical_groups`]
    /// order. A record says which operators chain together and how a run of them reduces —
    /// the half of an operator declaration that lives in the scope's operator registry rather than
    /// in a dispatch bucket, and the half a `USING <view> SCOPE` window needs to reduce a run at
    /// all. The buckets themselves are ordinary [`keyworded`](Self::keyworded) members.
    pub operators: OperatorMembers,
}

/// One declared chaining record: which operators chain together, and how a run of them reduces.
/// `members` is sorted by symbol bits and deduped, exactly as
/// [`OperatorGroup::alloc`](crate::machine::model::OperatorGroup) stores them, so two records over
/// the same set compare and digest alike whatever order they were written in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeclaredGroup {
    pub members: Vec<KeywordSymbol>,
    pub mode: ReductionMode,
}

/// A schema's operator channel, in [`canonical_groups`] order. Two records in one channel never
/// share a member: a scope declares one chaining mode per operator, which both the SIG collector
/// and the module registry enforce at their own write doors.
pub type OperatorMembers = Vec<DeclaredGroup>;

/// A schema's operator channel in canonical order — sorted by member run, then by mode, exact
/// duplicates collapsed. Every channel is stored through this, so the digest, the rendering and
/// equality all read one order rather than the order of declaration.
pub fn canonical_groups(mut groups: OperatorMembers) -> OperatorMembers {
    groups.sort_by(|a, b| {
        a.members
            .cmp(&b.members)
            .then_with(|| mode_order(a.mode).cmp(&mode_order(b.mode)))
    });
    groups.dedup();
    groups
}

/// A mode's total order, for [`canonical_groups`] — the same discriminant the digest feeds, with a
/// pairwise mode's combiner and direction breaking its own ties.
fn mode_order(mode: ReductionMode) -> (u8, Option<KeywordSymbol>, u8) {
    match mode {
        ReductionMode::Unary => (0, None, 0),
        ReductionMode::FoldLeft => (1, None, 0),
        ReductionMode::FoldRight => (2, None, 0),
        ReductionMode::Pairwise {
            combiner,
            direction,
        } => (3, Some(combiner), direction_byte(direction)),
    }
}

/// A fold direction as the byte the digest and the mode order both read.
fn direction_byte(direction: FoldDirection) -> u8 {
    match direction {
        FoldDirection::Left => 0,
        FoldDirection::Right => 1,
    }
}

/// Render one declared record as the `GROUP` head declaring it — `GROUP FOLD RIGHT {+ -}`,
/// `GROUP PAIRWISE FOLD #(BOTH) LEFT {< <=}`. The members render in their stored (sorted) order,
/// so a signature's name and a satisfaction diagnostic spell one record alike.
///
/// `None` for a record one of its own members' heads already spells in full: a bare `OP` head
/// declares exactly a **fold-left singleton**, and a `UNARY OP` head exactly a unary one, so
/// rendering those again would print one declaration twice. Every other record is a claim only a
/// `GROUP` head makes — a wider one, or a singleton at a mode no bare head implies, such as
/// `(GROUP FOLD RIGHT = ((OP #(-) OVER Carrier)))`. Deciding that here rather than at the call
/// site is what keeps two signatures differing only in a singleton's mode from sharing a name.
pub fn render_declared_group(group: &DeclaredGroup, registries: &RunRegistries) -> Option<String> {
    use std::fmt::Write;
    if group.members.len() == 1
        && matches!(group.mode, ReductionMode::FoldLeft | ReductionMode::Unary)
    {
        return None;
    }
    let mut out = match group.mode {
        // Unreachable by construction — a `Unary` record is written only by a `UNARY OP` head,
        // always over its one symbol, so the singleton test above already returned. Rendered
        // rather than panicked: a rendering path must never abort, and there is no `GROUP UNARY`
        // declarator for this to be mistaken for.
        ReductionMode::Unary => "GROUP UNARY".to_string(),
        ReductionMode::FoldLeft => "GROUP FOLD LEFT".to_string(),
        ReductionMode::FoldRight => "GROUP FOLD RIGHT".to_string(),
        ReductionMode::Pairwise {
            combiner,
            direction,
        } => format!(
            "GROUP PAIRWISE FOLD #({}) {}",
            display_label(combiner.symbol(), registries),
            match direction {
                FoldDirection::Left => "LEFT",
                FoldDirection::Right => "RIGHT",
            }
        ),
    };
    out.push_str(" {");
    for (index, member) in group.members.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{}", display_label(member.symbol(), registries));
    }
    out.push('}');
    Some(out)
}

/// The member run of every declared record, joined for a diagnostic that names a record by its
/// members alone.
fn render_members(members: &[KeywordSymbol], registries: &RunRegistries) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (index, member) in members.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{}", display_label(member.symbol(), registries));
    }
    out
}

/// A chaining mode's surface spelling, for the two operator satisfaction diagnostics.
fn render_mode(mode: ReductionMode, registries: &RunRegistries) -> String {
    match mode {
        ReductionMode::Unary => "unary".to_string(),
        ReductionMode::FoldLeft => "fold-left".to_string(),
        ReductionMode::FoldRight => "fold-right".to_string(),
        ReductionMode::Pairwise {
            combiner,
            direction,
        } => format!(
            "pairwise through `{}`, folding {}",
            display_label(combiner.symbol(), registries),
            match direction {
                FoldDirection::Left => "left",
                FoldDirection::Right => "right",
            }
        ),
    }
}

/// A keyworded member set in canonical order — sorted by content digest, exact duplicates
/// collapsed. Every schema's channel is stored through this, so two schemas declaring the same
/// members hold the same vector and digest alike.
pub fn canonical_overloads(mut overloads: Vec<KType>) -> Vec<KType> {
    overloads.sort_unstable();
    overloads.dedup();
    overloads
}

/// Whether two shapes key the same dispatch bucket — each one's element sequence with the slot
/// types erased, compared position by position.
///
/// The bucket key is a *reading* of the member's type, never a second copy of it, and a reading
/// only ever has to be compared: against another member's, or against a key already in hand
/// ([`shape_key_is`]). So no consumer materializes one, and the pairwise readers — which group a
/// candidate run by bucket, and are therefore quadratic in the run — allocate nothing at all.
pub(crate) fn shape_keys_equal(left: KType, right: KType, types: &TypeRegistry) -> bool {
    types.with_node(left, |left_node| {
        types.with_node(right, |right_node| {
            let (left, right) = (shape_elements(left_node), shape_elements(right_node));
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(a, b)| key_element(a) == key_element(b))
        })
    })
}

/// Whether `shape` keys exactly `key` — [`shape_keys_equal`] against a key already in hand, for
/// the readers that walk a bucket table and ask which declared members draw from a given bucket.
pub fn shape_key_is(shape: KType, key: &[KeyElement], types: &TypeRegistry) -> bool {
    types.with_node(shape, |node| {
        let elements = shape_elements(node);
        elements.len() == key.len()
            && elements
                .iter()
                .zip(key)
                .all(|(element, wanted)| key_element(element) == *wanted)
    })
}

/// A shape node's element sequence; empty for a node that is not a shape, which no schema member
/// ever is. The one place the "not a shape reads as the empty key" convention lives.
fn shape_elements(node: &TypeNode) -> &[DispatchTokenElement] {
    match node {
        TypeNode::ExpressionShape { elements, .. } => elements,
        _ => &[],
    }
}

/// One element's key reading: a keyword keeps its symbol, a slot erases its type.
fn key_element(element: &DispatchTokenElement) -> KeyElement {
    match element {
        DispatchTokenElement::Keyword(symbol) => KeyElement::Keyword(*symbol),
        DispatchTokenElement::Slot(_) => KeyElement::Slot,
    }
}

/// A shape's argument-position types, in order — the bucket key's typed half, for the readers that
/// compare or render one position at a time.
pub(super) fn shape_slots(kt: KType, types: &TypeRegistry) -> Vec<KType> {
    // Owns: the slot list is the function's return value, so it outlives the read.
    types.with_node(kt, |node| {
        shape_elements(node)
            .iter()
            .filter_map(|element| match element {
                DispatchTokenElement::Slot(kt) => Some(*kt),
                DispatchTokenElement::Keyword(_) => None,
            })
            .collect()
    })
}

/// A shape's return type, or `None` for anything that is not a shape.
fn shape_return(kt: KType, types: &TypeRegistry) -> Option<KType> {
    types.with_node(kt, |node| match node {
        TypeNode::ExpressionShape { ret, .. } => Some(*ret),
        _ => None,
    })
}

/// Render a keyworded member as the head declaring it — `(PURE _ :Number) -> Number`, the
/// `EXPR` head minus its keyword and its type sigil. Every argument position is the wildcard `_`:
/// a shape type carries no argument names, so there is none to print.
///
/// The one diagnostic currency for a keyworded member: the subtyping failures name a head with it,
/// and [`KType::write_name`](super::ktype::KType::write_name) renders a schema's keyworded members
/// through the same spelling, so a declaration and the error naming it read alike.
pub fn render_keyworded_head(
    shape: KType,
    operators: &OperatorMembers,
    registries: &RunRegistries,
) -> String {
    let types = &registries.types;
    if let Some(head) = render_operator_head(shape, operators, registries) {
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
        Some((quantifiers, elements, ret)) => super::ktype::ShapeSurface {
            quantifiers: &quantifiers,
            elements: &elements,
            ret,
            registries,
        }
        .to_string(),
        None => shape.name(registries),
    }
}

/// The operator-surface reading of a keyworded member, or `None` when the member is not one.
///
/// A member is an operator member iff its key is one of the two an operator declaration writes —
/// `[Slot, Keyword(s), Slot]` or `[Keyword(s), Slot]` — **and** `s` is a member of one of the
/// schema's declared chaining records. That second half is what keeps the plain `EXPR`-head
/// spelling of an operator key reading as an `EXPR` head: it declares the bucket and claims no
/// chaining, so it is not an operator member and does not render as one.
///
/// Both keys of a unary triple render as the one `UNARY OP` head — the head declares the triple,
/// so naming either half separately would spell a surface no one can write.
fn render_operator_head(
    shape: KType,
    operators: &OperatorMembers,
    registries: &RunRegistries,
) -> Option<String> {
    let types = &registries.types;
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
    let ret = shape_return(shape, types)?;
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
    let symbol = display_label(symbol.symbol(), registries);
    let operand = operand.display_name(registries);
    Some(if mode == ReductionMode::Unary {
        format!(
            "UNARY OP #({symbol}) OVER {operand} -> {}",
            ret.display_name(registries)
        )
    } else if ret == first_slot {
        // A fold member's result is its operand type, which the bare head already says.
        format!("OP #({symbol}) OVER {operand}")
    } else {
        format!(
            "OP #({symbol}) OVER {operand} -> {}",
            ret.display_name(registries)
        )
    })
}

impl SigSchema {
    /// The member-free schema — the module-lattice top the `:Module` name lowers to, and the
    /// content any zero-member `SIG E = ()` declaration projects to. `sig_id` is `None`: an empty
    /// interface names no abstract member for a slot type to substitute against.
    pub fn empty() -> SigSchema {
        SigSchema {
            sig_id: None,
            abstract_members: TypeMemberMap::default(),
            manifest_members: TypeMemberMap::default(),
            value_slots: HashMap::default(),
            keyworded: Vec::new(),
            operators: OperatorMembers::default(),
        }
    }

    /// Project a SIG decl scope into its schema, at SIG finish. Every type-table entry is a
    /// genuine type member (the token-class partition holds — a value slot's name is a value
    /// token and lives in the scope's slot collector, not in `types`), classified
    /// abstract/manifest by representation; the value slots come from the scope's own slot
    /// collector. Runs once per SIG.
    ///
    /// The schema and the scope tables share one currency — the classified symbols the decl scope
    /// already keys by — so projection copies keys straight across.
    pub(crate) fn project_decl(decl_scope: &Scope<'_>, registries: &RunRegistries) -> SigSchema {
        let types = &registries.types;
        let declared = decl_scope.id;
        let mut abstract_members = TypeMemberMap::default();
        let mut manifest_members = TypeMemberMap::default();
        for (name, kt) in decl_scope.bindings().iter_types() {
            let canonical = canonicalize_binder(kt, declared, types);
            if is_abstract_sig_member(canonical, types) {
                abstract_members.insert(name, canonical);
            } else {
                manifest_members.insert(name, canonical);
            }
        }
        let mut value_slots = HashMap::default();
        for (name, kt) in decl_scope.sig_value_slots() {
            value_slots.insert(name, canonicalize_binder(kt, declared, types));
        }
        let keyworded = canonical_overloads(
            decl_scope
                .sig_keyworded_members()
                .into_iter()
                .map(|shape| canonicalize_binder(shape, declared, types))
                .collect(),
        );
        SigSchema {
            sig_id: Some(ScopeId::SENTINEL),
            abstract_members,
            manifest_members,
            value_slots,
            keyworded,
            // The operator channel needs no canonicalization pass of its own — the collector keys
            // records by their member run and stores members sorted — but it is stored through the
            // canonical door anyway, so one function is the whole answer to "what order is this in".
            operators: canonical_groups(decl_scope.sig_operator_groups()),
        }
    }

    /// Fold `WITH` pins into a schema, eagerly and completely: each pinned abstract member
    /// becomes a manifest member fixed to the pin's type, and every reference to it in the
    /// remaining member and slot types is substituted with the pinned type
    /// ([`substitute_sig_members`]). The folded schema is fully concrete in the pinned members —
    /// `Ordered WITH {Carrier = Number}` interns the same content a SIG declaring
    /// `Carrier = Number` outright (or a structurally identical module self-sig) carries, so
    /// specialization introduces no second spelling of a concrete interface. A no-op clone when
    /// `pins` is empty.
    pub fn fold_pins(&self, pins: &[(TypeSymbol, KType)], types: &TypeRegistry) -> SigSchema {
        let mut schema = self.clone();
        if pins.is_empty() {
            return schema;
        }
        let substitutions: TypeMemberMap = pins.iter().copied().collect();
        for (name, kt) in pins {
            schema.abstract_members.remove(name);
            schema.manifest_members.insert(*name, *kt);
        }
        if let Some(sig_id) = schema.sig_id {
            for kt in schema
                .manifest_members
                .values_mut()
                .chain(schema.value_slots.values_mut())
            {
                *kt = substitute_sig_members(*kt, sig_id, &substitutions, types);
            }
            // Two declared overloads that became identical under a pin are one overload: the
            // canonical order dedupes them, so the folded schema names each surviving shape once.
            schema.keyworded = canonical_overloads(
                schema
                    .keyworded
                    .iter()
                    .map(|shape| substitute_sig_members(*shape, sig_id, &substitutions, types))
                    .collect(),
            );
        }
        schema
    }

    /// Derive a module's principal signature (self-sig) directly from the body it is being built
    /// over — `child_scope` is the module's child scope and `draft` the members its construction
    /// gathered, both read before the module value exists.
    ///
    /// A module never carries abstract members. The manifest members are the union of the
    /// draft's `type_members` map (the seeded type interface an ascription installs — per-call
    /// mints for `:|`, the source's own bindings for `:!` — plus its mirrored manifests) and the
    /// child scope's type-class entries — the map wins on a shared name, so this covers a plain
    /// module and either ascription view alike, the map being a mirror of the scope.
    /// Value slots are the child scope's data bindings read through [`KObject::ktype`]. An opaque
    /// view needs no override for its abstract slot identities: its scope is born holding *coerced*
    /// member values, so the ktype read off a member already reports the view's own identity.
    ///
    /// [`KObject::ktype`]: crate::machine::model::values::KObject::ktype
    pub fn raw_self_sig(child: &Scope<'_>, draft: &ModuleDraft) -> SigSchema {
        let mut manifest_members = TypeMemberMap::default();
        for (name, kt) in child.bindings().iter_types() {
            manifest_members.insert(name, kt);
        }
        for (name, kt) in draft.type_members.iter() {
            manifest_members.insert(*name, *kt);
        }
        let mut value_slots: HashMap<ValueSymbol, KType, IdentityBuildHasher> = HashMap::default();
        // A value member's slot type: the seal's own `'home` brand covers the re-anchor, and only
        // the `Copy` `KType` leaves the open.
        for (name, sealed) in child.bindings().iter_data() {
            value_slots.insert(name, sealed.open_at().value().object().ktype());
        }
        // The keyworded surface: every dispatch bucket the body registered, each overload named by
        // the shape type its callable reports — one flat canonical list, since a member's key is
        // read back off its own type. The read is the probe form — nothing is minted and only the
        // `Copy` `KType` leaves the confined open.
        let mut keyworded: Vec<KType> = Vec::new();
        for (_, overloads) in child.bindings().iter_functions() {
            keyworded.extend(
                overloads
                    .iter()
                    .map(|sealed| child.read_function(sealed, |function| function.shape_ktype())),
            );
        }
        let keyworded = canonical_overloads(keyworded);
        // The operator channel: every distinct group record the body registered. The registry
        // holds one entry per nonempty member subset, all sealing the one record, so the read
        // dedupes by record address before describing each — the same per-record fold the
        // environment copy's rebirth makes.
        let operators = canonical_groups(
            child
                .bindings()
                .iter_operator_groups()
                .iter()
                .map(|sealed| {
                    child.read_operator_group(sealed, |group| DeclaredGroup {
                        members: group.member_symbols().collect(),
                        mode: group.mode(),
                    })
                })
                .collect(),
        );
        SigSchema {
            sig_id: None,
            abstract_members: TypeMemberMap::default(),
            manifest_members,
            value_slots,
            keyworded,
            operators,
        }
    }
}

/// `Some(parameter names)` iff `kt` is a type constructor — a declared family (a
/// `TypeConstructor`-kind member, whose names ride its sealed schema) or a SIG's abstract
/// higher-kinded member (an `AbstractType` node carrying them directly). `None` for a
/// first-order type. Arity is the returned list's length.
pub fn constructor_param_names(kt: KType, types: &TypeRegistry) -> Option<Vec<TypeSymbol>> {
    // Owns: the parameter list is the function's own return value, so it must outlive this read.
    types.with_node(kt, |node| match node {
        TypeNode::AbstractType { param_names, .. } if !param_names.is_empty() => {
            Some(param_names.clone())
        }
        TypeNode::SetMember {
            kind: KKind::TypeConstructor,
            schema: NodeSchema::TypeConstructor { param_names, .. },
            ..
        } => Some(param_names.clone()),
        _ => None,
    })
}

/// The diagnostic for a bare type constructor standing in a value type position, or `None` when
/// `kt` is well-kinded there.
///
/// A value's type must be a proper type — kind `*`. The ill-kinded shapes are exactly the two
/// [`constructor_param_names`] names: a declared family at `TypeConstructor` kind and
/// a SIG's higher-kinded abstract member, each of kind `* -> *` and standing with none of its
/// parameters supplied. A saturated application (`ConstructorApply`), a first-order abstract
/// member, and every ground type are proper, so they yield `None`. A *type* position — the head
/// of an application, a `TYPE (Elem AS Wrap)` declaration, a module's type-constructor member —
/// takes a bare constructor legitimately and never consults this.
///
/// `position` is a noun phrase naming the *type slot* the constructor stands in — "the type of
/// parameter `x`", "the return type", "the element type of `LIST OF`" — so it reads as the
/// subject of "must be a proper type". It names the type, never the value or field whose type it
/// is: "the type of SIG value slot `boxed`", not "SIG value slot `boxed`", since a slot is not
/// itself a type. The constructor's parameter names follow, since supplying them is the fix.
pub fn unsaturated_constructor_message(
    kt: KType,
    position: impl std::fmt::Display,
    registries: &RunRegistries,
) -> Option<String> {
    use std::fmt::Write;
    let types = &registries.types;
    let param_names = constructor_param_names(kt, types)?;
    let plural = if param_names.len() == 1 { "" } else { "s" };
    // The message is its own buffer: the type name, every parameter name and the applied form are
    // written into it as they are read, so naming a constructor costs no rendering allocation.
    let mut message = String::new();
    let _ = write!(
        message,
        "`{}` is a type constructor taking {} type parameter{plural} (",
        kt.display_name(registries),
        param_names.len(),
    );
    for (index, param) in param_names.iter().enumerate() {
        if index > 0 {
            message.push_str(", ");
        }
        let _ = write!(message, "`{}`", display_label(param.symbol(), registries));
    }
    let _ = write!(
        message,
        "), but {position} must be a proper type — apply it, as `:({} {{",
        kt.display_name(registries),
    );
    for (index, param) in param_names.iter().enumerate() {
        if index > 0 {
            message.push_str(", ");
        }
        let _ = write!(
            message,
            "{} = <Type>",
            display_label(param.symbol(), registries)
        );
    }
    message.push_str("})`");
    Some(message)
}

/// Order-blind comparison of two constructor parameter lists: identity is the name set, and
/// declaration order is presentation. Symbol order is the canonical order — an arbitrary but
/// stable total order over the same names, which is all a set comparison needs.
pub(super) fn name_sets_equal(left: &[TypeSymbol], right: &[TypeSymbol]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left: Vec<TypeSymbol> = left.to_vec();
    let mut right: Vec<TypeSymbol> = right.to_vec();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

/// Rewrite `kt`, replacing references to `sig_id`'s abstract members with the caller's bindings
/// for them. Returns an interned handle used only for comparison.
///
/// One reference shape substitutes: a nonce-free `AbstractType { source: sig_id, name }` of either
/// order — a first-order slot type, or a higher-kinded member in the constructor position of a
/// `ConstructorApply`. Compound types recurse; every other shape is returned unchanged. A nonced
/// `AbstractType` is an opaque ascription's generative mint, not a reference to a declaration, so
/// it never substitutes even when it shares its binder's `source` and name.
///
/// A **nested `Signature`** recurses like any other compound: a signature standing in a slot type
/// (`VAL subs :(LIST OF (Inner WITH {Item = Elt}))`) carries references to the enclosing
/// signature's members in its own manifest members and value slots, and those rewrite. The nested
/// schema's *own* abstract members shadow the enclosing binder's **by name** — every projected SIG
/// canonicalizes to [`ScopeId::SENTINEL`], so `source` cannot tell an inner binder from an outer
/// one and the shadowed names are subtracted from the substitution before the descent.
pub fn substitute_sig_members(
    kt: KType,
    sig_id: ScopeId,
    members: &TypeMemberMap,
    types: &TypeRegistry,
) -> KType {
    types.with_node(kt, |node| match node {
        TypeNode::AbstractType {
            source,
            name,
            nonce: None,
            ..
        } if *source == sig_id => members.get(name).copied().unwrap_or(kt),
        TypeNode::List { element } => {
            let element = substitute_sig_members(*element, sig_id, members, types);
            types.list(element)
        }
        TypeNode::Dict { key, value } => {
            let key = substitute_sig_members(*key, sig_id, members, types);
            let value = substitute_sig_members(*value, sig_id, members, types);
            types.dict(key, value)
        }
        TypeNode::Record { fields } => {
            let fields = fields.map(|v| substitute_sig_members(*v, sig_id, members, types));
            types.record(fields)
        }
        TypeNode::KFunction { params, ret } => {
            let params = params.map(|v| substitute_sig_members(*v, sig_id, members, types));
            let ret = substitute_sig_members(*ret, sig_id, members, types);
            types.function_type(params, ret)
        }
        TypeNode::ExpressionShape {
            quantifiers,
            elements,
            ret,
        } => types.rebuild_shape(quantifiers, elements, *ret, |v| {
            substitute_sig_members(v, sig_id, members, types)
        }),
        TypeNode::Union { members: us } => {
            let substituted: Vec<KType> = us
                .iter()
                .map(|m| substitute_sig_members(*m, sig_id, members, types))
                .collect();
            types.union_of(&substituted)
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            let constructor = substitute_sig_members(*constructor, sig_id, members, types);
            let arguments = arguments.map(|a| substitute_sig_members(*a, sig_id, members, types));
            types.constructor_apply(constructor, arguments)
        }
        TypeNode::Signature { schema, .. } => {
            let effective = unshadowed(members, schema);
            if effective.is_empty() {
                return kt;
            }
            // The nested schema's own `abstract_members` are binders, not references, and its
            // `sig_id` names its own binder — neither is touched. `types.signature` is
            // content-addressed, so re-interning an unchanged schema returns `kt` itself.
            //
            // Owns: the manifest members and value slots are rewritten in place before
            // re-interning, which needs a mutable schema.
            let mut schema = schema.clone();
            for member in schema
                .manifest_members
                .values_mut()
                .chain(schema.value_slots.values_mut())
            {
                *member = substitute_sig_members(*member, sig_id, &effective, types);
            }
            schema.keyworded = canonical_overloads(
                schema
                    .keyworded
                    .iter()
                    .map(|shape| substitute_sig_members(*shape, sig_id, &effective, types))
                    .collect(),
            );
            types.signature(schema)
        }
        _ => kt,
    })
}

/// `members` with `nested`'s own abstract-member names removed — the substitution a walk carries
/// into a nested signature, since a nested binder shadows the enclosing one by name. Borrows when
/// nothing shadows, which is the common case (folding a `WITH` pin usually empties
/// `abstract_members` outright).
fn unshadowed<'m>(members: &'m TypeMemberMap, nested: &SigSchema) -> Cow<'m, TypeMemberMap> {
    if nested.abstract_members.is_empty() {
        return Cow::Borrowed(members);
    }
    Cow::Owned(
        members
            .iter()
            .filter(|(name, _)| !nested.abstract_members.contains_key(*name))
            .map(|(name, kt)| (*name, *kt))
            .collect(),
    )
}

/// Two bindings for one signature's abstract members: the pair a value read across an ascription
/// barrier is rewritten between. A value inhabiting
/// `substitute_sig_members(declared, sig_id, from)` coerces to
/// `substitute_sig_members(declared, sig_id, to)` for every SIG-declared slot type `declared`.
///
/// **Both tables ride as interned `Signature` handles** whose manifest members *are* the table —
/// a signature is already the canonical carrier of a name → type binding set, and interning one
/// makes the whole carrier `Copy` and lifetime-free. That is what lets a coercion plan sit inside
/// a [`Body`](crate::machine::core::Body) and inside a sealed return obligation, neither of which
/// may name a region.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MemberCoercion {
    sig_id: ScopeId,
    from: KType,
    to: KType,
}

impl MemberCoercion {
    /// Intern the two tables into their handles. `sig_id` is the binder whose abstract members the
    /// declared slot types reference — [`SigSchema::sig_id`], canonically [`ScopeId::SENTINEL`].
    pub fn new(
        sig_id: ScopeId,
        from: &TypeMemberMap,
        to: &TypeMemberMap,
        types: &TypeRegistry,
    ) -> MemberCoercion {
        MemberCoercion {
            sig_id,
            from: member_table_handle(from, types),
            to: member_table_handle(to, types),
        }
    }

    /// The same pair read in the other direction — a wrapper coerces its arguments inward under
    /// this and its result outward under [`Self::flipped`], off one stored plan.
    pub fn flipped(self) -> MemberCoercion {
        MemberCoercion {
            sig_id: self.sig_id,
            from: self.to,
            to: self.from,
        }
    }

    /// Materialize the plan for a walk: the two tables read back out of their handles, once.
    pub fn tables(self, types: &TypeRegistry) -> CoercionTables {
        CoercionTables {
            plan: self,
            from: member_table(self.from, types),
            to: member_table(self.to, types),
        }
    }
}

/// A [`MemberCoercion`] with its two tables materialized — what a coercion walk carries down its
/// recursion, so the handles are read once per coercion rather than once per position.
pub struct CoercionTables {
    plan: MemberCoercion,
    from: TypeMemberMap,
    to: TypeMemberMap,
}

impl CoercionTables {
    /// The `Copy` plan these tables were read out of — what a wrapper built mid-walk stores, so a
    /// per-call coercion re-materializes from the same handles rather than from a rebuilt table.
    pub fn coercion(&self) -> MemberCoercion {
        self.plan
    }

    /// The pair of substituted types a declared position rewrites between, or `None` when the two
    /// substitutions agree — the walk's fast path, which covers a concrete slot, a manifest-only
    /// slot, and every sub-position holding no reference to an abstract member.
    pub fn substitutions(&self, declared: KType, types: &TypeRegistry) -> Option<(KType, KType)> {
        let from = substitute_sig_members(declared, self.plan.sig_id, &self.from, types);
        let to = substitute_sig_members(declared, self.plan.sig_id, &self.to, types);
        (from != to).then_some((from, to))
    }

    /// Whether a value filling the slot typed `declared` has to be coerced at all — the replay's
    /// per-member plan question, and the same test the walk's own first step runs.
    pub fn coerces(&self, declared: KType, types: &TypeRegistry) -> bool {
        self.substitutions(declared, types).is_some()
    }

    /// The `to`-side substitution of a declared position — the identity a re-tag stamps and the
    /// type a rebuilt container is re-stamped to.
    pub fn substitute_to(&self, declared: KType, types: &TypeRegistry) -> KType {
        substitute_sig_members(declared, self.plan.sig_id, &self.to, types)
    }

    /// These tables with `nested`'s own abstract-member names removed from both sides — what the
    /// coercion walk carries when it descends into a nested signature, since a nested binder
    /// shadows the enclosing one by name exactly as [`substitute_sig_members`] has it.
    ///
    /// A fresh [`MemberCoercion`] rather than a filtered view, so the narrowed plan rides on into
    /// the nested view's own `ViewMembers` and into any `Body::CoercedDelegate` built beneath it.
    pub fn shadowed_by(&self, nested: &SigSchema, types: &TypeRegistry) -> CoercionTables {
        MemberCoercion::new(
            self.plan.sig_id,
            &unshadowed(&self.from, nested),
            &unshadowed(&self.to, nested),
            types,
        )
        .tables(types)
    }

    /// The `from`-side member table itself — what the shared selection function needs to read a
    /// declared type in the *source's* vocabulary without materializing the substituted type. The
    /// satisfaction check hands the same pair to the same door, which is what makes the view's
    /// pick and the check's pick one decision rather than two.
    pub fn source_table(&self) -> &TypeMemberMap {
        &self.from
    }

    /// The binder the declared types' abstract-member references are sourced at — [`source_table`]'s
    /// other half.
    ///
    /// [`source_table`]: Self::source_table
    pub fn sig_id(&self) -> ScopeId {
        self.plan.sig_id
    }

    /// The `from`-side substitution — the type the value being coerced currently inhabits, which a
    /// union arm tests a value against to pick its declared member.
    pub fn substitute_from(&self, declared: KType, types: &TypeRegistry) -> KType {
        substitute_sig_members(declared, self.plan.sig_id, &self.from, types)
    }
}

/// Intern a member table as the `Signature` handle carrying it — manifest members only, since a
/// binding table names concrete witnesses. `sig_id: None`: the table is a substitution, not an
/// interface, so it declares no abstract member for a slot type to reference.
fn member_table_handle(members: &TypeMemberMap, types: &TypeRegistry) -> KType {
    types.signature(SigSchema {
        sig_id: None,
        abstract_members: TypeMemberMap::default(),
        manifest_members: members.clone(),
        value_slots: HashMap::default(),
        // A substitution table names no keyworded member and no chaining record.
        keyworded: Vec::new(),
        operators: OperatorMembers::default(),
    })
}

/// Read a member table back out of the handle [`member_table_handle`] interned it into.
///
/// The one field is copied out, not the node: the table has to **own** it because
/// [`CoercionTables`] hands it to a caller and carries it across the whole coercion walk, outliving
/// any read.
fn member_table(handle: KType, types: &TypeRegistry) -> TypeMemberMap {
    types.with_node(handle, |node| match node {
        TypeNode::Signature { schema, .. } => schema.manifest_members.clone(),
        _ => unreachable!("a member table handle is interned as a `Signature` node"),
    })
}

/// The least upper bound of two schemas under the relation [`sig_subtype`] decides — the join of
/// the module lattice, and the `Signature` arm of [`TypeRegistry::join`].
///
/// **Width intersects.** A member only one operand names is dropped: the bound may promise only
/// what both operands supply.
///
/// **Depth reconciles per member.** Two equal manifest bindings survive manifest, since both
/// operands meet that exact requirement. Anything else at a matching kind — two differing
/// manifests, a manifest against an abstract, two abstracts — demotes to an abstract member at
/// that kind, the strongest requirement both bindings still satisfy. A kind disagreement (one
/// side first-order, or two constructors over different parameter names) has no common
/// requirement at all, so the member drops.
///
/// **Value slots join pointwise**, but through the demoted members first ([`sig_slot_join`]): a
/// slot typed by one operand's binding of `Carrier` and the other's rejoins as a reference to
/// `Carrier` itself rather than coarsening to `Any`.
///
/// The result carries the canonical [`ScopeId::SENTINEL`] binder every projected SIG carries and
/// mints nonce-free abstract members, so a joined schema is content-identical to the equivalent
/// written `SIG` declaration — and a join that keeps nothing lands on [`SigSchema::empty`], the
/// lattice top `:Module`, by digest.
pub fn join_schemas(a: &SigSchema, b: &SigSchema, types: &TypeRegistry) -> SigSchema {
    let binding_of = |schema: &SigSchema, name: &TypeSymbol| {
        schema
            .manifest_members
            .get(name)
            .or_else(|| schema.abstract_members.get(name))
            .copied()
    };
    // Sorted, so the demoted-member choice a binding-pair collision below settles on is the lowest
    // member symbol rather than a hash order.
    let mut names: Vec<TypeSymbol> = a
        .manifest_members
        .keys()
        .chain(a.abstract_members.keys())
        .copied()
        .collect();
    names.sort_unstable();

    let mut abstract_members = TypeMemberMap::default();
    let mut manifest_members = TypeMemberMap::default();
    // (`a`'s binding, `b`'s binding) → the member demoted over that pair. Two members demoting over
    // one pair of bindings are interchangeable in a slot position, so the first — the lowest
    // symbol — wins.
    let mut generalizations: HashMap<(KType, KType), KType> = HashMap::new();
    for name in names {
        let (Some(left), Some(right)) = (binding_of(a, &name), binding_of(b, &name)) else {
            continue;
        };
        let left_params = constructor_param_names(left, types);
        let param_names = match (&left_params, &constructor_param_names(right, types)) {
            (None, None) => Vec::new(),
            (Some(x), Some(y)) if name_sets_equal(x, y) => x.clone(),
            _ => continue,
        };
        let manifest_in_both =
            a.manifest_members.contains_key(&name) && b.manifest_members.contains_key(&name);
        if left == right && manifest_in_both {
            manifest_members.insert(name, left);
            continue;
        }
        let demoted = types.intern(TypeNode::AbstractType {
            source: ScopeId::SENTINEL,
            name,
            param_names,
            nonce: None,
        });
        abstract_members.insert(name, demoted);
        generalizations.entry((left, right)).or_insert(demoted);
    }

    let mut value_slots = HashMap::default();
    for (name, left) in &a.value_slots {
        if let Some(right) = b.value_slots.get(name) {
            value_slots.insert(*name, sig_slot_join(*left, *right, &generalizations, types));
        }
    }

    // Keyworded members join **pairwise under a shared key**: every left member is joined against
    // every right member keying the same bucket, positionally — slots meet, the return joins
    // ([`sig_slot_join`]'s shape arm) — and a pair whose slots met to `Never` is dropped as
    // vacuous, since nothing fills it. Each kept pair is an upper bound of both operands, so the
    // canonical set of them is the strongest interface both still satisfy.
    let mut keyworded: Vec<KType> = Vec::new();
    for left in &a.keyworded {
        for right in &b.keyworded {
            if !shape_keys_equal(*left, *right, types) {
                continue;
            }
            let joined = sig_slot_join(*left, *right, &generalizations, types);
            if shape_slots(joined, types).contains(&KType::NEVER) {
                continue;
            }
            keyworded.push(joined);
        }
    }
    let keyworded = canonical_overloads(keyworded);

    // Operator members intersect pairwise under an **equal** mode: a record either side declares
    // is a claim about how a run of its members reduces, and two modes make incompatible claims,
    // so only same-mode pairs have a common weakening. The weakening is the member intersection —
    // the operators both operands agree chain that way — and an empty one keeps nothing.
    let mut operators = OperatorMembers::new();
    for left in &a.operators {
        for right in &b.operators {
            if left.mode != right.mode {
                continue;
            }
            let members: Vec<KeywordSymbol> = left
                .members
                .iter()
                .filter(|member| right.members.contains(member))
                .copied()
                .collect();
            if !members.is_empty() {
                operators.push(DeclaredGroup {
                    members,
                    mode: left.mode,
                });
            }
        }
    }

    SigSchema {
        // A schema with no abstract member names nothing for a slot to substitute against, which
        // is what `None` records — the same `sig_id` a module self-sig and the empty schema carry.
        sig_id: (!abstract_members.is_empty()).then_some(ScopeId::SENTINEL),
        abstract_members,
        manifest_members,
        value_slots,
        keyworded,
        operators: canonical_groups(operators),
    }
}

/// Anti-unify two slot types against `generalizations`, then join what does not generalize — the
/// **covariant** half of the value-slot join, used at a slot's own position, a container's
/// element, and a function's return.
///
/// A pair the two operands bind one demoted member to *is* that member: a module satisfying the
/// join supplies some binding for it, and each operand's slot type is exactly its own binding, so
/// the reference is satisfied in either variance. Anything left over falls to
/// [`TypeRegistry::join`], which coarsens to `Any` where no structure relates the two.
fn sig_slot_join(
    x: KType,
    y: KType,
    generalizations: &HashMap<(KType, KType), KType>,
    types: &TypeRegistry,
) -> KType {
    if let Some(member) = generalizations.get(&(x, y)) {
        return *member;
    }
    if x == y {
        return x;
    }
    types.with_node(x, |nx| {
        types.with_node(y, |ny| match (nx, ny) {
            (TypeNode::List { element: ex }, TypeNode::List { element: ey }) => {
                let element = sig_slot_join(*ex, *ey, generalizations, types);
                types.list(element)
            }
            (TypeNode::Dict { key: kx, value: vx }, TypeNode::Dict { key: ky, value: vy }) => {
                let key = sig_slot_join(*kx, *ky, generalizations, types);
                let value = sig_slot_join(*vx, *vy, generalizations, types);
                types.dict(key, value)
            }
            (
                TypeNode::KFunction {
                    params: px,
                    ret: rx,
                },
                TypeNode::KFunction {
                    params: py,
                    ret: ry,
                },
            ) if px.len() == py.len() && px.keys().all(|k| py.get(k.symbol()).is_some()) => {
                let params = px
                    .iter()
                    .map(|(name, t)| {
                        let other = *py.get(name.symbol()).expect("the key sets were compared");
                        (name, sig_slot_meet(*t, other, generalizations, types))
                    })
                    .collect();
                let ret = sig_slot_join(*rx, *ry, generalizations, types);
                types.function_type(params, ret)
            }
            // A shape's argument positions are contravariant like a function's parameters, so
            // they take the dual; the return joins. Two shapes under different keys have no
            // common shape and fall to the coarsening below.
            (
                TypeNode::ExpressionShape {
                    quantifiers: qx,
                    elements: ex,
                    ret: rx,
                },
                TypeNode::ExpressionShape {
                    quantifiers: qy,
                    elements: ey,
                    ret: ry,
                },
            ) if qx.len() == qy.len() => {
                match zip_shape_slots(ex, ey, |a, b| sig_slot_meet(a, b, generalizations, types)) {
                    Some(elements) => {
                        let ret = sig_slot_join(*rx, *ry, generalizations, types);
                        types.shape_type(qx, &elements, ret)
                    }
                    None => types.join(x, y),
                }
            }
            _ => types.join(x, y),
        })
    })
}

/// The **contravariant** half of [`sig_slot_join`]: a function parameter position, where the bound
/// admits only what both operands admit. Generalization still comes first — a demoted member is
/// the common requirement in either variance — and what does not generalize meets, so two
/// unrelated parameter types bottom out at `Never` rather than falsely widening to `Any`.
fn sig_slot_meet(
    x: KType,
    y: KType,
    generalizations: &HashMap<(KType, KType), KType>,
    types: &TypeRegistry,
) -> KType {
    if let Some(member) = generalizations.get(&(x, y)) {
        return *member;
    }
    if x == y {
        return x;
    }
    types.with_node(x, |nx| {
        types.with_node(y, |ny| match (nx, ny) {
            (TypeNode::List { element: ex }, TypeNode::List { element: ey }) => {
                let element = sig_slot_meet(*ex, *ey, generalizations, types);
                types.list(element)
            }
            (TypeNode::Dict { key: kx, value: vx }, TypeNode::Dict { key: ky, value: vy }) => {
                let key = sig_slot_meet(*kx, *ky, generalizations, types);
                let value = sig_slot_meet(*vx, *vy, generalizations, types);
                types.dict(key, value)
            }
            (
                TypeNode::KFunction {
                    params: px,
                    ret: rx,
                },
                TypeNode::KFunction {
                    params: py,
                    ret: ry,
                },
            ) if px.len() == py.len() && px.keys().all(|k| py.get(k.symbol()).is_some()) => {
                let params = px
                    .iter()
                    .map(|(name, t)| {
                        let other = *py.get(name.symbol()).expect("the key sets were compared");
                        (name, sig_slot_join(*t, other, generalizations, types))
                    })
                    .collect();
                let ret = sig_slot_meet(*rx, *ry, generalizations, types);
                types.function_type(params, ret)
            }
            // Dual of the [`sig_slot_join`] arm: argument positions join, the return meets.
            (
                TypeNode::ExpressionShape {
                    quantifiers: qx,
                    elements: ex,
                    ret: rx,
                },
                TypeNode::ExpressionShape {
                    quantifiers: qy,
                    elements: ey,
                    ret: ry,
                },
            ) if qx.len() == qy.len() => {
                match zip_shape_slots(ex, ey, |a, b| sig_slot_join(a, b, generalizations, types)) {
                    Some(elements) => {
                        let ret = sig_slot_meet(*rx, *ry, generalizations, types);
                        types.shape_type(qx, &elements, ret)
                    }
                    None => types.meet(x, y),
                }
            }
            _ => types.meet(x, y),
        })
    })
}

/// Pair two shape element runs positionally, combining each argument position through `combine`.
/// `Some` iff the two runs key one bucket — equal length, and the same keyword in every keyword
/// position; `None` otherwise, which the caller coarsens through the plain lattice.
fn zip_shape_slots(
    a: &[DispatchTokenElement],
    b: &[DispatchTokenElement],
    mut combine: impl FnMut(KType, KType) -> KType,
) -> Option<smallvec::SmallVec<[DispatchTokenElement; 12]>> {
    if a.len() != b.len() {
        return None;
    }
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| match (x, y) {
            (DispatchTokenElement::Keyword(s), DispatchTokenElement::Keyword(t)) if s == t => {
                Some(*x)
            }
            (DispatchTokenElement::Slot(sx), DispatchTokenElement::Slot(sy)) => {
                Some(DispatchTokenElement::Slot(combine(*sx, *sy)))
            }
            _ => None,
        })
        .collect()
}

/// Rewrite every reference to `declared`'s own abstract members so it is sourced at
/// [`ScopeId::SENTINEL`] instead — the canonical binder every projected SIG schema shares.
///
/// Structurally this is [`substitute_sig_members`] with the substitution being a re-source rather
/// than a lookup: the same shapes recurse, and only a nonce-free `AbstractType` at `declared`
/// changes. Running it at projection is what makes two textually identical declarations one
/// interned type, since after it nothing in the schema records which scope declared it.
fn canonicalize_binder(kt: KType, declared: ScopeId, types: &TypeRegistry) -> KType {
    types.with_node(kt, |node| match node {
        TypeNode::AbstractType {
            source,
            name,
            param_names,
            nonce: None,
        } if *source == declared => types.intern(TypeNode::AbstractType {
            source: ScopeId::SENTINEL,
            name: *name,
            // Owns: re-interned as a fresh node's own field.
            param_names: param_names.clone(),
            nonce: None,
        }),
        TypeNode::List { element } => {
            let element = canonicalize_binder(*element, declared, types);
            types.list(element)
        }
        TypeNode::Dict { key, value } => {
            let key = canonicalize_binder(*key, declared, types);
            let value = canonicalize_binder(*value, declared, types);
            types.dict(key, value)
        }
        TypeNode::Record { fields } => {
            let fields = fields.map(|v| canonicalize_binder(*v, declared, types));
            types.record(fields)
        }
        TypeNode::KFunction { params, ret } => {
            let params = params.map(|v| canonicalize_binder(*v, declared, types));
            let ret = canonicalize_binder(*ret, declared, types);
            types.function_type(params, ret)
        }
        TypeNode::ExpressionShape {
            quantifiers,
            elements,
            ret,
        } => types.rebuild_shape(quantifiers, elements, *ret, |v| {
            canonicalize_binder(v, declared, types)
        }),
        TypeNode::Union { members } => {
            let canonical: Vec<KType> = members
                .iter()
                .map(|m| canonicalize_binder(*m, declared, types))
                .collect();
            types.union_of(&canonical)
        }
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            let constructor = canonicalize_binder(*constructor, declared, types);
            let arguments = arguments.map(|a| canonicalize_binder(*a, declared, types));
            types.constructor_apply(constructor, arguments)
        }
        TypeNode::Signature { schema, .. } => {
            // Keyed on the *real* declaring scope id, so no shadow subtraction: the nested
            // schema's own members were re-sourced to `SENTINEL` when the nested SIG was
            // projected, and `source == declared` can only match a genuine outer reference.
            //
            // Owns: the manifest members and value slots are rewritten in place before
            // re-interning, which needs a mutable schema.
            let mut schema = schema.clone();
            for member in schema
                .manifest_members
                .values_mut()
                .chain(schema.value_slots.values_mut())
            {
                *member = canonicalize_binder(*member, declared, types);
            }
            schema.keyworded = canonical_overloads(
                schema
                    .keyworded
                    .iter()
                    .map(|shape| canonicalize_binder(*shape, declared, types))
                    .collect(),
            );
            types.signature(schema)
        }
        _ => kt,
    })
}

/// Why a [`sig_subtype`] check failed — the per-member rule that rejected, carrying the offending
/// member name and the *rendered* types that disagreed. Rendering to `String` at the failure site
/// keeps this type free of any `KType` reference, so it travels as plain diagnostic data.
pub enum SigSubtypeFailure {
    MissingTypeMember {
        name: String,
    },
    ManifestMismatch {
        name: String,
        got: String,
        expected: String,
    },
    /// A type member's kind or parameter names disagreed. `expected_params` is `Some(names)` when
    /// the super signature declares a constructor over those parameters, `None` when it declares
    /// a first-order proper type; `got` is the rendered sub binding that failed to match. The
    /// parameter names are rendered at the failure site, like every other field here.
    KindMismatch {
        name: String,
        expected_params: Option<Vec<String>>,
        got: String,
    },
    MissingValueSlot {
        name: String,
    },
    ValueSlotMismatch {
        name: String,
        got: String,
        expected: String,
    },
    /// The sub schema declares no dispatch bucket under the declared member's key at all.
    MissingKeyworded {
        head: String,
    },
    /// The bucket exists but no overload in it satisfies the declared member. `got` is the rendered
    /// list of the overloads that were there, so the diagnostic shows what was rejected.
    KeywordedMismatch {
        head: String,
        got: Vec<String>,
    },
    /// Every overload under the key failed, and the first one failed at a position the declared
    /// member **quantifies** over: a quantified position is satisfied only by one at least as
    /// general (quantified, or `Any`), so a concrete position there says the module implements one
    /// instantiation where the signature declares an operation holding at every one.
    QuantifiedMismatch {
        head: String,
        parameter: String,
        got: String,
    },
    /// Two or more overloads satisfy the declared member and none is strictly the most specific —
    /// the keyworded reading of a dispatch ambiguity, raised where dispatch would raise it.
    AmbiguousKeyworded {
        head: String,
        candidates: Vec<String>,
    },
    /// No record in the module's operator registry covers the declared record's members. The
    /// module may well define every bucket — a bare `FN` head over an operator symbol does — but
    /// declaring no group over them, it claims nothing about how a run of them reduces.
    MissingOperatorGroup {
        members: String,
    },
    /// A record covering the declared members exists, but chains them a different way. The two
    /// modes are rendered at the failure site like every other field here.
    OperatorModeMismatch {
        members: String,
        expected: String,
        got: String,
    },
}

impl SigSubtypeFailure {
    /// Render the failure as the message fragment an ascription error embeds after
    /// `` module does not satisfy signature `{path}`: ``.
    pub fn render_fragment(&self) -> String {
        match self {
            SigSubtypeFailure::MissingTypeMember { name } => {
                format!("missing type member `{name}`")
            }
            SigSubtypeFailure::ManifestMismatch {
                name,
                got,
                expected,
            } => format!(
                "type member `{name}` is `{got}` but the signature fixes it to `{expected}`"
            ),
            SigSubtypeFailure::KindMismatch {
                name,
                expected_params: Some(params),
                got,
            } => {
                let mut sorted: Vec<&str> = params.iter().map(String::as_str).collect();
                sorted.sort_unstable();
                format!(
                    "type member `{name}` must be a type constructor with parameters {{{}}}, got `{got}`",
                    sorted.join(", ")
                )
            }
            SigSubtypeFailure::KindMismatch {
                name,
                expected_params: None,
                got,
            } => format!(
                "type member `{name}` must be a proper type, got the type constructor `{got}`"
            ),
            SigSubtypeFailure::MissingValueSlot { name } => format!("missing member `{name}`"),
            SigSubtypeFailure::ValueSlotMismatch {
                name,
                got,
                expected,
            } => {
                format!("member `{name}` has type `{got}` but the signature declares `{expected}`")
            }
            SigSubtypeFailure::MissingKeyworded { head } => {
                format!("missing keyworded member `{head}`")
            }
            SigSubtypeFailure::KeywordedMismatch { head, got } => format!(
                "no overload satisfies keyworded member `{head}` (found {})",
                got.iter()
                    .map(|one| format!("`{one}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            SigSubtypeFailure::QuantifiedMismatch {
                head,
                parameter,
                got,
            } => format!(
                "keyworded member `{head}` quantifies over `{parameter}`, but the overload fixes \
                 that position to `{got}` — one implementation must hold at every `{parameter}`"
            ),
            SigSubtypeFailure::AmbiguousKeyworded { head, candidates } => format!(
                "keyworded member `{head}` is satisfied by {} with no most specific one",
                candidates
                    .iter()
                    .map(|one| format!("`{one}`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
            SigSubtypeFailure::MissingOperatorGroup { members } => format!(
                "no chaining mode covers `{members}` (the module defines the buckets but declares \
                 no group over them)"
            ),
            SigSubtypeFailure::OperatorModeMismatch {
                members,
                expected,
                got,
            } => format!(
                "operators `{members}` chain {expected} in the signature but {got} in the module"
            ),
        }
    }
}

/// The canonical signature-subtyping relation: `sub <: sup`. Ok iff `sub` supplies every member
/// `sup` names (width — members `sup` does not name are ignored), with each manifest member
/// equal, each abstract member present at the matching kind and parameter names, and each value slot
/// covariantly compatible after substituting `sup`'s abstract members with `sub`'s bindings.
///
/// The failure is boxed: `SigSubtypeFailure` carries rendered member names and types, and is
/// large relative to the common `Ok` path.
pub fn sig_subtype(
    sub: &SigSchema,
    sup: &SigSchema,
    registries: &RunRegistries,
) -> Result<(), Box<SigSubtypeFailure>> {
    let types = &registries.types;
    // 1. Abstract members: present at the matching kind, and — for a constructor — over the same
    // parameter-name *set*. Parameter names are interface: a family declaring `{Item}` does not
    // supply a slot declared over `{Elem}`. The sub binding may be manifest or abstract.
    for (name, sup_repr) in &sup.abstract_members {
        let sub_binding = sub
            .manifest_members
            .get(name)
            .or_else(|| sub.abstract_members.get(name));
        let Some(sub_binding) = sub_binding else {
            return Err(Box::new(SigSubtypeFailure::MissingTypeMember {
                name: render_label(name.symbol(), registries),
            }));
        };
        let sup_params = constructor_param_names(*sup_repr, types);
        let sub_params = constructor_param_names(*sub_binding, types);
        let agrees = match (&sup_params, &sub_params) {
            (None, None) => true,
            (Some(expected), Some(got)) => name_sets_equal(expected, got),
            _ => false,
        };
        if !agrees {
            return Err(Box::new(SigSubtypeFailure::KindMismatch {
                name: render_label(name.symbol(), registries),
                expected_params: sup_params.map(|params| {
                    params
                        .iter()
                        .map(|p| render_label(p.symbol(), registries))
                        .collect()
                }),
                got: sub_binding.render(registries),
            }));
        }
    }

    // 2. Manifest members: present manifest in `sub` with an equal type.
    for (name, fixed) in &sup.manifest_members {
        match sub.manifest_members.get(name) {
            Some(got) if got == fixed => {}
            Some(got) => {
                return Err(Box::new(SigSubtypeFailure::ManifestMismatch {
                    name: render_label(name.symbol(), registries),
                    got: got.render(registries),
                    expected: fixed.render(registries),
                }));
            }
            None => {
                // An abstract `sub` member supplies no witness for a manifest requirement.
                if let Some(repr) = sub.abstract_members.get(name) {
                    return Err(Box::new(SigSubtypeFailure::ManifestMismatch {
                        name: render_label(name.symbol(), registries),
                        got: repr.render(registries),
                        expected: fixed.render(registries),
                    }));
                }
                return Err(Box::new(SigSubtypeFailure::MissingTypeMember {
                    name: render_label(name.symbol(), registries),
                }));
            }
        }
    }

    // 3. Value slots: present and covariantly compatible after abstract-member substitution.
    // The substitution binds every `sub` type-member name to its representation, so a `sup` slot
    // referencing one of `sup`'s abstract members reads through `sub`'s binding for it.
    // `slot_satisfied_by` computes the same verdict as
    // `substitute_sig_members(declared, id, sub_member_map).satisfied_by(sub_type)` by comparing
    // structurally and swapping in `sub`'s binding on reaching a self-abstract reference, so no
    // substituted type is ever built.
    let mut sub_member_map = TypeMemberMap::default();
    for (name, kt) in &sub.manifest_members {
        sub_member_map.insert(*name, *kt);
    }
    for (name, repr) in &sub.abstract_members {
        sub_member_map.insert(*name, *repr);
    }
    for (name, declared) in &sup.value_slots {
        let Some(sub_type) = sub.value_slots.get(name) else {
            return Err(Box::new(SigSubtypeFailure::MissingValueSlot {
                name: render_label(name.symbol(), registries),
            }));
        };
        let ok = match sup.sig_id {
            Some(id) => slot_satisfied_by(*declared, *sub_type, &sub_member_map, id, registries),
            // No `sig_id`: nothing to substitute, so the heterogeneous `satisfied_by` is exact.
            None => declared.satisfied_by(*sub_type, registries),
        };
        if !ok {
            return Err(Box::new(SigSubtypeFailure::ValueSlotMismatch {
                name: render_label(name.symbol(), registries),
                got: sub_type.render(registries),
                expected: declared.render(registries),
            }));
        }
    }

    // 4. Keyworded members: mirroring dispatch resolution. Each declared overload needs at least
    // one covariantly satisfying overload in `sub`'s bucket under the same key — the same
    // `KFunction` rule a value slot's function type is checked by — and among the satisfiers the
    // most specific is the one it selects. An incomparable tie is the keyworded reading of a
    // dispatch ambiguity, and rejects here rather than at the call.
    for declared in &sup.keyworded {
        let candidates: Vec<KType> = sub
            .keyworded
            .iter()
            .filter(|candidate| shape_keys_equal(*declared, **candidate, types))
            .copied()
            .collect();
        let head = || render_keyworded_head(*declared, &sup.operators, registries);
        let render_all = |indices: &[usize]| -> Vec<String> {
            indices
                .iter()
                .map(|i| render_keyworded_head(candidates[*i], &sub.operators, registries))
                .collect()
        };
        match select_keyworded_satisfier(
            *declared,
            &candidates,
            sup.sig_id.map(|id| (&sub_member_map, id)),
            registries,
        ) {
            Ok(_) => {}
            Err(satisfiers) if satisfiers.is_empty() && candidates.is_empty() => {
                return Err(Box::new(SigSubtypeFailure::MissingKeyworded {
                    head: head(),
                }));
            }
            Err(satisfiers) if satisfiers.is_empty() => {
                if let Some((parameter, got)) =
                    quantified_position_failure(*declared, candidates[0], registries)
                {
                    return Err(Box::new(SigSubtypeFailure::QuantifiedMismatch {
                        head: head(),
                        parameter,
                        got,
                    }));
                }
                return Err(Box::new(SigSubtypeFailure::KeywordedMismatch {
                    head: head(),
                    got: render_all(&(0..candidates.len()).collect::<Vec<_>>()),
                }));
            }
            Err(satisfiers) => {
                return Err(Box::new(SigSubtypeFailure::AmbiguousKeyworded {
                    head: head(),
                    candidates: render_all(&satisfiers),
                }));
            }
        }
    }

    // 5. Operator members: each declared record needs a `sub` record whose member set **includes**
    // it — width, as for every other channel — under an **equal** mode. At most one `sub` record
    // can cover a declared one: two records in a channel never share a member, so a set covering
    // the declared members is unique if it exists. Mode is matched exactly rather than covariantly
    // because a mode is not an approximation of another: a run folded right and the same run
    // folded left compute different things.
    for declared in &sup.operators {
        let covering = sub
            .operators
            .iter()
            .find(|record| declared.members.iter().all(|m| record.members.contains(m)));
        let members = || render_members(&declared.members, registries);
        let Some(covering) = covering else {
            return Err(Box::new(SigSubtypeFailure::MissingOperatorGroup {
                members: members(),
            }));
        };
        if covering.mode != declared.mode {
            return Err(Box::new(SigSubtypeFailure::OperatorModeMismatch {
                members: members(),
                expected: render_mode(declared.mode, registries),
                got: render_mode(covering.mode, registries),
            }));
        }
    }
    Ok(())
}

/// Why `candidate` failed the declared member, when the reason is a **quantified** position: the
/// first slot whose declared type reads a quantifier and whose candidate slot is not at least as
/// general, as the parameter's name and the type the candidate fixes it to. `None` when the
/// failure is an ordinary type disagreement, which the plain mismatch already renders.
fn quantified_position_failure(
    declared: KType,
    candidate: KType,
    registries: &RunRegistries,
) -> Option<(String, String)> {
    let types = &registries.types;
    let quantifiers = types.with_node(declared, |node| match node {
        TypeNode::ExpressionShape { quantifiers, .. } => quantifiers.clone(),
        _ => Vec::new(),
    });
    if quantifiers.is_empty() {
        return None;
    }
    let candidate_slots = shape_slots(candidate, types);
    for (position, declared_slot) in shape_slots(declared, types).iter().enumerate() {
        let candidate_slot = candidate_slots.get(position)?;
        // Contravariance: a candidate position fills a declared one by being equal or *more
        // general*. A position that pairs is not the failure, whatever it holds.
        if *declared_slot == *candidate_slot
            || declared_slot.is_more_specific_than(*candidate_slot, registries)
        {
            continue;
        }
        let named =
            (0..quantifiers.len()).find(|i| types.references_quantifier(*declared_slot, *i))?;
        return Some((
            render_label(quantifiers[named].symbol(), registries),
            candidate_slot.name(registries),
        ));
    }
    None
}

/// The overload a declared keyworded member selects out of `candidates` — the one resolution both
/// [`sig_subtype`] and an ascription's bucket replay run, so the member a signature check accepts
/// is the member the view installs.
///
/// Two steps, mirroring dispatch: keep the candidates that **satisfy** the declared overload (the
/// covariant rule value slots use), then rank the survivors by [`shape_specificity`] — the
/// positional pairing dispatch itself ranks by — and take the one strictly more specific than
/// every peer. A lone satisfier wins with no ranking.
///
/// `substitution` is how a declared type that references a binder's abstract members is read: the
/// subtyping check passes the binder and `sub`'s bindings for its members, so the comparison runs
/// against what the candidate side actually holds. A caller whose `declared` is already substituted
/// into the candidates' own world — the ascription replay, which substitutes through the coercion
/// plan's `from` side first — passes `None` and gets the plain structural rule.
///
/// `Err` carries the satisfier indices: empty means nothing satisfied, two or more means an
/// incomparable tie. Both are errors at the ascription, with the caller deciding the diagnostic.
pub fn select_keyworded_satisfier(
    declared: KType,
    candidates: &[KType],
    substitution: Option<(&TypeMemberMap, ScopeId)>,
    registries: &RunRegistries,
) -> Result<usize, Vec<usize>> {
    let satisfiers: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| match substitution {
            Some((members, id)) => {
                slot_satisfied_by(declared, **candidate, members, id, registries)
            }
            None => declared.satisfied_by(**candidate, registries),
        })
        .map(|(index, _)| index)
        .collect();
    if let [only] = satisfiers[..] {
        return Ok(only);
    }
    satisfiers
        .iter()
        .find(|i| {
            satisfiers.iter().all(|j| {
                *i == j
                    || matches!(
                        shape_specificity(candidates[**i], candidates[*j], registries),
                        Specificity::StrictlyMore
                    )
            })
        })
        .copied()
        .ok_or(satisfiers)
}

/// True iff `declared` contains a reference to one of `sig_id`'s abstract members that
/// [`substitute_sig_members`] would rewrite (an `AbstractType` sourced at `sig_id` whose name
/// `members` binds). When false, substitution is the identity and a plain compare on `declared`
/// is exact. A reference inside a nested signature counts, under the same by-name shadowing the
/// substitution applies — that is what routes such a slot off the structural fast path.
fn references_sig_member(
    declared: KType,
    sig_id: ScopeId,
    members: &TypeMemberMap,
    types: &TypeRegistry,
) -> bool {
    types.with_node(declared, |node| match node {
        TypeNode::AbstractType {
            source,
            name,
            nonce: None,
            ..
        } => *source == sig_id && members.contains_key(name),
        TypeNode::List { element } => references_sig_member(*element, sig_id, members, types),
        TypeNode::Dict { key, value } => {
            references_sig_member(*key, sig_id, members, types)
                || references_sig_member(*value, sig_id, members, types)
        }
        TypeNode::Record { fields } => fields
            .values()
            .any(|v| references_sig_member(*v, sig_id, members, types)),
        TypeNode::KFunction { params, ret } => {
            params
                .values()
                .any(|v| references_sig_member(*v, sig_id, members, types))
                || references_sig_member(*ret, sig_id, members, types)
        }
        TypeNode::ExpressionShape { elements, ret, .. } => {
            elements.iter().any(|element| match element {
                DispatchTokenElement::Slot(v) => references_sig_member(*v, sig_id, members, types),
                DispatchTokenElement::Keyword(_) => false,
            }) || references_sig_member(*ret, sig_id, members, types)
        }
        TypeNode::Union { members: us } => us
            .iter()
            .any(|m| references_sig_member(*m, sig_id, members, types)),
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            references_sig_member(*constructor, sig_id, members, types)
                || arguments
                    .values()
                    .any(|a| references_sig_member(*a, sig_id, members, types))
        }
        TypeNode::Signature { schema, .. } => {
            let effective = unshadowed(members, schema);
            !effective.is_empty()
                && schema
                    .manifest_members
                    .values()
                    .chain(schema.value_slots.values())
                    .chain(schema.keyworded.iter())
                    .any(|kt| references_sig_member(*kt, sig_id, &effective, types))
        }
        _ => false,
    })
}

/// The `sub`-side binding a substitution point in `declared` resolves to, if any — the type
/// `substitute_sig_members` would splice in for this node.
fn substitution_binding(
    declared: KType,
    sig_id: ScopeId,
    members: &TypeMemberMap,
    types: &TypeRegistry,
) -> Option<KType> {
    types.with_node(declared, |node| match node {
        TypeNode::AbstractType {
            source,
            name,
            nonce: None,
            ..
        } if *source == sig_id => members.get(name).copied(),
        _ => None,
    })
}

/// Verdict of `substitute_sig_members(declared, sig_id, members).satisfied_by(sub_type)` — does the
/// `sub` value slot fill the substituted `sup` slot? — computed without materializing the
/// substituted type. On reaching a self-abstract reference the walk switches to a direct
/// compare against `sub`'s binding; on a member-free node it falls to plain
/// `satisfied_by`; otherwise it descends the shared container structure with the same covariance
/// [`KType::satisfied_by`] applies (`Dict`/`Record`/`KFunction` component rules included).
///
/// A nested `Signature` and an `ExpressionShape` are the positions that do materialize: each has
/// a relation of its own — schema-recursive `sig_subtype`, positional shape subtyping — so the
/// walk substitutes once and hands the interned result to `satisfied_by` rather than re-deriving
/// that recursion.
fn slot_satisfied_by(
    declared: KType,
    sub_type: KType,
    members: &TypeMemberMap,
    sig_id: ScopeId,
    registries: &RunRegistries,
) -> bool {
    let types = &registries.types;
    if let Some(binding) = substitution_binding(declared, sig_id, members, types) {
        return binding.satisfied_by(sub_type, registries);
    }
    if !references_sig_member(declared, sig_id, members, types) {
        return declared.satisfied_by(sub_type, registries);
    }
    types.with_node(declared, |dn| {
        types.with_node(sub_type, |sn| match (dn, sn) {
            // The two positions the no-materialize walks materialize at. The relation between two
            // signatures is `sig_subtype`, which is already schema-recursive, so substituting once
            // and handing the interned result to it beats re-deriving that recursion here; a shape
            // materializes for the same economy — its own relation pairs positionally over an
            // element run, and rebuilding that walk substituting-as-it-goes would buy nothing.
            // Both are rare, and one intern is cheaper than a fourth structural walk.
            (TypeNode::Signature { .. } | TypeNode::ExpressionShape { .. }, _) => {
                substitute_sig_members(declared, sig_id, members, types)
                    .satisfied_by(sub_type, registries)
            }
            (TypeNode::List { element: ed }, TypeNode::List { element: es }) => {
                slot_satisfied_by(*ed, *es, members, sig_id, registries)
            }
            (TypeNode::Dict { key: kd, value: vd }, TypeNode::Dict { key: ks, value: vs }) => {
                slot_satisfied_by(*kd, *ks, members, sig_id, registries)
                    && slot_satisfied_by(*vd, *vs, members, sig_id, registries)
            }
            (TypeNode::Record { fields: fd }, TypeNode::Record { fields: fs }) => {
                // Record-value covariance: every slot field present in the value, covariantly.
                fd.iter().all(|(name, dt)| {
                    fs.get(name.symbol())
                        .is_some_and(|st| slot_satisfied_by(*dt, *st, members, sig_id, registries))
                })
            }
            (
                TypeNode::ConstructorApply {
                    constructor: cd,
                    arguments: ad,
                },
                TypeNode::ConstructorApply {
                    constructor: cs,
                    arguments: as_,
                },
            ) => {
                ad.len() == as_.len()
                    && slot_types_equal(*cd, *cs, members, sig_id, types)
                    && ad.iter().all(|(name, d)| {
                        as_.get(name.symbol())
                            .is_some_and(|s| slot_satisfied_by(*d, *s, members, sig_id, registries))
                    })
            }
            (
                TypeNode::KFunction {
                    params: pd,
                    ret: rd,
                },
                TypeNode::KFunction {
                    params: ps,
                    ret: rs,
                },
            ) => {
                // Contravariant params (width-drop): every value param names a slot param the
                // substituted slot fixes equal-or-more-specific. Covariant return.
                ps.keys().all(|k| pd.get(k.symbol()).is_some())
                    && ps.iter().all(|(name, sp)| {
                        pd.get(name.symbol()).is_some_and(|dp| {
                            slot_more_specific_or_equal(*dp, *sp, members, sig_id, registries)
                        })
                    })
                    && slot_satisfied_by(*rd, *rs, members, sig_id, registries)
            }
            (TypeNode::Union { members: ud }, sub_node) => {
                // A value satisfies a substituted union slot iff it (each of its members, if it is
                // itself a union) refines some slot member — the union-membership rule of
                // `satisfied_by`.
                let refines_a_member = |y: KType| {
                    ud.iter()
                        .any(|md| slot_satisfied_by(*md, y, members, sig_id, registries))
                };
                match sub_node {
                    TypeNode::Union { members: us } => us.iter().all(|y| refines_a_member(*y)),
                    _ => refines_a_member(sub_type),
                }
            }
            _ => false,
        })
    })
}

/// Verdict of `substitute_sig_members(declared, ...) == target
/// || substitute_sig_members(declared, ...).is_more_specific_than(target)` — the contravariant
/// direction [`slot_satisfied_by`] needs for a function parameter, computed without building the
/// substituted type except at a nested `Signature` or an `ExpressionShape`, which materialize for
/// the same reason.
fn slot_more_specific_or_equal(
    declared: KType,
    target: KType,
    members: &TypeMemberMap,
    sig_id: ScopeId,
    registries: &RunRegistries,
) -> bool {
    let types = &registries.types;
    if let Some(binding) = substitution_binding(declared, sig_id, members, types) {
        return binding == target || binding.is_more_specific_than(target, registries);
    }
    if !references_sig_member(declared, sig_id, members, types) {
        return declared == target || declared.is_more_specific_than(target, registries);
    }
    // The substituted slot outranks `Any` / an unconstrained name, and refines a union it has a
    // member in — the top guards of `more_specific_walk`, mirrored here.
    if target == KType::ANY
        || target == KType::IDENTIFIER
        || target == KType::of_kind(KKind::ProperType)
        || target == KType::NAME_TOKEN
        || target == KType::TYPE_NAME_TOKEN
    {
        return true;
    }
    types.with_node(target, |target_node| {
        if let TypeNode::Union { members: ts } = target_node {
            return ts
                .iter()
                .any(|t| slot_more_specific_or_equal(declared, *t, members, sig_id, registries));
        }
        types.with_node(declared, |declared_node| {
            match (declared_node, target_node) {
                // Materialize at a nested signature or a shape, as [`slot_satisfied_by`] does —
                // the `Signature`-vs-`Signature` relation rides `is_more_specific_than`'s own
                // strict `sig_subtype` arm, and a shape's rides its positional arm.
                (TypeNode::Signature { .. } | TypeNode::ExpressionShape { .. }, _) => {
                    let substituted = substitute_sig_members(declared, sig_id, members, types);
                    substituted == target || substituted.is_more_specific_than(target, registries)
                }
                (TypeNode::List { element: ed }, TypeNode::List { element: et }) => {
                    slot_more_specific_or_equal(*ed, *et, members, sig_id, registries)
                }
                (TypeNode::Dict { key: kd, value: vd }, TypeNode::Dict { key: kt, value: vt }) => {
                    slot_more_specific_or_equal(*kd, *kt, members, sig_id, registries)
                        && slot_more_specific_or_equal(*vd, *vt, members, sig_id, registries)
                }
                (TypeNode::Record { fields: fd }, TypeNode::Record { fields: ft }) => {
                    // Record-value covariance with width-superset: the more-specific record has every
                    // field of `target`, each covariantly refined.
                    ft.keys().all(|k| fd.get(k.symbol()).is_some())
                        && ft.iter().all(|(name, tt)| {
                            fd.get(name.symbol()).is_some_and(|dt| {
                                slot_more_specific_or_equal(*dt, *tt, members, sig_id, registries)
                            })
                        })
                }
                (
                    TypeNode::ConstructorApply {
                        constructor: cd,
                        arguments: ad,
                    },
                    TypeNode::ConstructorApply {
                        constructor: ct,
                        arguments: at,
                    },
                ) => {
                    ad.len() == at.len()
                        && slot_types_equal(*cd, *ct, members, sig_id, types)
                        && ad.iter().all(|(name, d)| {
                            at.get(name.symbol()).is_some_and(|t| {
                                slot_more_specific_or_equal(*d, *t, members, sig_id, registries)
                            })
                        })
                }
                (
                    TypeNode::KFunction {
                        params: pd,
                        ret: rd,
                    },
                    TypeNode::KFunction {
                        params: pt,
                        ret: rt,
                    },
                ) => {
                    // Contravariant params, covariant return — the dual of the `slot_satisfied_by` case.
                    pd.keys().all(|k| pt.get(k.symbol()).is_some())
                        && pd.iter().all(|(name, dp)| {
                            pt.get(name.symbol()).is_some_and(|tp| {
                                slot_satisfied_by(*dp, *tp, members, sig_id, registries)
                            })
                        })
                        && slot_more_specific_or_equal(*rd, *rt, members, sig_id, registries)
                }
                _ => false,
            }
        })
    })
}

/// Verdict of `substitute_sig_members(declared, ...) == other` — structural equality with `sub`'s
/// bindings spliced in. Only the constructor identity of a `ConstructorApply` needs this (a
/// constructor is a leaf member reference, so the recursion bottoms out immediately in practice).
/// A nested `Signature` materializes and compares handles — content addressing makes that exact.
fn slot_types_equal(
    declared: KType,
    other: KType,
    members: &TypeMemberMap,
    sig_id: ScopeId,
    types: &TypeRegistry,
) -> bool {
    if let Some(binding) = substitution_binding(declared, sig_id, members, types) {
        return binding == other;
    }
    if !references_sig_member(declared, sig_id, members, types) {
        return declared == other;
    }
    types.with_node(declared, |dn| {
        types.with_node(other, |on| match (dn, on) {
            // Materialize at a nested signature or a shape: every handle is content-addressed, so
            // handle equality against the substituted type *is* the structural comparison.
            (TypeNode::Signature { .. } | TypeNode::ExpressionShape { .. }, _) => {
                substitute_sig_members(declared, sig_id, members, types) == other
            }
            (TypeNode::List { element: ed }, TypeNode::List { element: eo }) => {
                slot_types_equal(*ed, *eo, members, sig_id, types)
            }
            (TypeNode::Dict { key: kd, value: vd }, TypeNode::Dict { key: ko, value: vo }) => {
                slot_types_equal(*kd, *ko, members, sig_id, types)
                    && slot_types_equal(*vd, *vo, members, sig_id, types)
            }
            (TypeNode::Record { fields: fd }, TypeNode::Record { fields: fo }) => {
                fd.len() == fo.len()
                    && fd.iter().all(|(name, dt)| {
                        fo.get(name.symbol())
                            .is_some_and(|ot| slot_types_equal(*dt, *ot, members, sig_id, types))
                    })
            }
            (
                TypeNode::ConstructorApply {
                    constructor: cd,
                    arguments: ad,
                },
                TypeNode::ConstructorApply {
                    constructor: co,
                    arguments: ao,
                },
            ) => {
                ad.len() == ao.len()
                    && slot_types_equal(*cd, *co, members, sig_id, types)
                    && ad.iter().all(|(name, d)| {
                        ao.get(name.symbol())
                            .is_some_and(|o| slot_types_equal(*d, *o, members, sig_id, types))
                    })
            }
            (
                TypeNode::KFunction {
                    params: pd,
                    ret: rd,
                },
                TypeNode::KFunction {
                    params: po,
                    ret: ro,
                },
            ) => {
                pd.len() == po.len()
                    && pd.iter().all(|(name, dt)| {
                        po.get(name.symbol())
                            .is_some_and(|ot| slot_types_equal(*dt, *ot, members, sig_id, types))
                    })
                    && slot_types_equal(*rd, *ro, members, sig_id, types)
            }
            (TypeNode::Union { members: ud }, TypeNode::Union { members: uo }) => {
                ud.len() == uo.len()
                    && ud.iter().all(|dm| {
                        uo.iter()
                            .any(|om| slot_types_equal(*dm, *om, members, sig_id, types))
                    })
            }
            _ => false,
        })
    })
}

/// Classify a SIG type-table entry by its *representation*: an abstract member carries no
/// concrete witness, which is exactly an `AbstractType` node — the first-order `TYPE Elt` slot
/// and the higher-kinded `TYPE (Elem AS Wrap)` slot alike, both sourced at the SIG decl scope.
/// Everything else — a manifest `LET Tag = Number` binding a concrete type, a minted constructor
/// family — is manifest.
pub(crate) fn is_abstract_sig_member(kt: KType, types: &TypeRegistry) -> bool {
    types.with_node(kt, |node| matches!(node, TypeNode::AbstractType { .. }))
}

#[cfg(test)]
mod tests;
