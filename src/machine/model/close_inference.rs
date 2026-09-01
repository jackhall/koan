//! Free-identifier inference for `CLOSE (<block>)`: which names a block would resolve outward for,
//! read structurally off raw AST before anything evaluates.
//!
//! `CLOSE OVER` names its captures; `CLOSE` derives the same list from the block. Deriving it means
//! answering, for every identifier the block spells, whether the block itself binds it — which is a
//! scoping question, so the walk sources every binding fact it can from the reader the interpreter
//! uses rather than restating it:
//!
//! - which slots hold raw code — [`KExpression::lazy_kinds_at`], the seal-time
//!   [`LAZY_SLOT_SPECS`](crate::machine::model::lazy_slots::LAZY_SLOT_SPECS) stamp;
//! - what a statement declares — [`KExpression::statement_binder_plan`];
//! - which position of a declaration form is the declared name — [`KExpression::binder_name_slot`];
//! - which surfaces are nominal declarations — [`announced_type_declaration`];
//! - what a module body announces — [`announce_type_members`];
//! - where a signature's binders sit — [`SignatureScan`];
//! - what a `MATCH` arm or an `OP` body binds — [`MACHINE_BINDERS`].
//!
//! What is left is stated here and pinned by tests: the positional visibility rule, and the label
//! positions no table answers. [`FORM_SPECS`] holds the second — one entry per builtin form whose
//! slots the generic walk would misread — keyed by full untyped bucket key, sound for the same
//! reason [`BINDER_SPECS`](crate::machine::model::binder::BINDER_SPECS) is: builtin buckets are
//! unshadowable, so a node whose key matches an entry can only ever resolve to that builtin's
//! overloads.
//!
//! The walk is exact rather than conservative, because laziness is static: a bare `(…)` outside a
//! builtin's lazy slot evaluates in the block's own chain, so its identifiers are genuine uses. See
//! [design/lazy-closures.md](../../../design/lazy-closures.md).

use crate::machine::core::body_statement_refs;
use crate::machine::model::binder::{
    TypeDeclarationSurface, announced_type_declaration, union_schema,
};
use crate::machine::model::key_spec::{KEYWORDS, KeyElementSpec, key_matches_parts};
use crate::machine::model::labels::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::machine::model::lazy_slots::LazyKinds;
use crate::machine::model::{
    ExpressionPart, KExpression, MACHINE_BINDERS, RunRegistries, SignaturePosition, SignatureScan,
    announce_type_members, pair_list_names,
};
use crate::source::{FileId, Span};
use crate::witnessed::{BumpAllocator, BumpVec};

/// What the block's free identifiers came to. Every buffer is the caller's arena — the step scratch
/// on the runtime path — so the whole analysis dies with the drain pop that produced it.
pub(crate) struct CloseInference<'s> {
    /// Free value-channel names, in first-occurrence order.
    pub(crate) values: BumpVec<'s, ValueSymbol>,
    /// Free type-channel names, in first-occurrence order.
    pub(crate) types: BumpVec<'s, TypeSymbol>,
    /// The first form inside the inference domain that resolves names dynamically, if any. A block
    /// holding one has no inferable capture list at all, so this outranks the two name runs.
    pub(crate) conflict: Option<InferenceConflict>,
}

/// A form the inference domain forbids, and where it sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct InferenceConflict {
    pub(crate) form: DynamicNameForm,
    pub(crate) span: Option<Span>,
    pub(crate) file: Option<FileId>,
}

/// The two forms that resolve names against something the walk cannot read statically.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DynamicNameForm {
    /// `$(…)`, and its `EVAL <expr>` spelling — the name run is a value, known only at evaluation.
    Eval,
    /// `USING <module> SCOPE (<body>)` — the surfaced member names are the module's, not the text's.
    Using,
}

impl DynamicNameForm {
    /// The surface this form is written as, for the diagnostic that reports the conflict.
    pub fn surface(self) -> &'static str {
        match self {
            DynamicNameForm::Eval => "$(...)",
            DynamicNameForm::Using => "USING ... SCOPE",
        }
    }

    /// What this form does that the walk cannot read, phrased to complete "`<surface>` at <site>
    /// …".
    pub fn reason(self) -> &'static str {
        match self {
            DynamicNameForm::Eval => "resolves names dynamically",
            DynamicNameForm::Using => "surfaces module members dynamically",
        }
    }
}

/// The names `block` would resolve outward for, derived exactly as an explicit `CLOSE OVER` list
/// would have named them.
///
/// `block` is the raw body slot, walked before any of it dispatches. A nested `CLOSE` contributes
/// its own free set and not its conflicts — the inner form raises those when it evaluates.
pub(crate) fn infer_close_captures<'s>(
    block: &KExpression<'_>,
    scratch: BumpAllocator<'s>,
    registries: &RunRegistries,
) -> CloseInference<'s> {
    let mut walk = Walk {
        scratch,
        registries,
        scopes: BumpVec::new_in(scratch),
        bindings: BumpVec::new_in(scratch),
        values: BumpVec::new_in(scratch),
        types: BumpVec::new_in(scratch),
        conflict: None,
    };
    walk.walk_block(block, &[]);
    CloseInference {
        values: walk.values,
        types: walk.types,
        conflict: walk.conflict,
    }
}

// ---------- the recognized forms ----------

use KeyElementSpec::{Keyword as Kw, Slot};

/// What the walk does with the slots of a form its full untyped bucket key names. Every entry
/// states only what the sourced readers do not: which slot is a label rather than a use, which is
/// severed rather than walked, and which child scope is seeded with what.
enum FormRule {
    /// A signature-bearing declaration: the binders the signature slot declares seed the body
    /// slot's scope, while the signature's own annotations are uses in the enclosing one. `body` is
    /// `None` for the type-language `FN <signature> -> <return>` declarator, which has none.
    Signature {
        signature: usize,
        body: Option<usize>,
    },
    /// An operator declaration: the body slot's scope is seeded with the operand names the surface
    /// fixes rather than spells ([`MACHINE_BINDERS`]).
    Operator { unary: bool, body: usize },
    /// The `OVER`-less `MATCH`: the slot holds `<head> -> <body>` triples whose heads are type
    /// tests — genuine type-channel uses — and each body's scope is seeded with the arm binder.
    Arms { arms: usize },
    /// `MATCH … OVER` / `TRY`: the same triples, except each head names a **member** of the form's
    /// own operand — a union member, an error kind, the `_` wildcard — and a member never binds in
    /// the enclosing scope. So the heads are skipped and only the bodies are walked, seeded with
    /// the arm binder.
    MemberArms { arms: usize },
    /// `MODULE` / `GROUP`: the body is a statement block whose announced type declarations are
    /// visible throughout it regardless of statement order.
    ModuleBody { body: usize },
    /// `ATTR <record> <field>` — `m.x`. The field slot is a member label the record resolves, not
    /// a name the scope does; the lhs is an ordinary use.
    Attribute { field: usize },
    /// `(<fields>) FROM <record>`: the field list is labels only.
    Projection { fields: usize },
    /// `CLOSE OVER <captures> <body>`: the capture list is a run of uses against this chain, and
    /// the body is severed — its own names resolve against those captures, never outward.
    ExplicitClose { captures: usize, body: usize },
    /// `CLOSE <body>` nested inside the domain: the inner block's free set is this form's use set.
    InferredClose { body: usize },
    /// A form the inference domain forbids.
    Dynamic(DynamicNameForm),
}

/// A builtin form the generic walk would misread, and the rule that reads it.
struct FormSpec {
    /// Full untyped bucket key — ALL keywords in position, never just the lead keyword.
    key: &'static [KeyElementSpec],
    rule: FormRule,
}

/// The forms the walk recognizes structurally. Pinned against the live registration table by the
/// consistency tests, so an entry whose builtin was renamed, re-shaped, or dropped fails the suite.
///
/// The nominal declarations (`NEWTYPE <name> = <repr>`, `UNION <name> = <schema>`) are absent on
/// purpose: [`announced_type_declaration`] already recognizes them off `BINDER_SPECS`, so the walk
/// asks that rather than restating their keys.
static FORM_SPECS: &[FormSpec] = &[
    // FN <signature> -> <return type> — the type-language declarator, no body.
    FormSpec {
        key: &[Kw(&KEYWORDS.fn_), Slot, Kw(&KEYWORDS.arrow), Slot],
        rule: FormRule::Signature {
            signature: 1,
            body: None,
        },
    },
    // FN <signature> -> <return type> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Signature {
            signature: 1,
            body: Some(5),
        },
    },
    // LET <name> = FN <signature> -> <return type> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.fn_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Signature {
            signature: 4,
            body: Some(8),
        },
    },
    // OP <symbol> OVER <operand> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Operator {
            unary: false,
            body: 5,
        },
    },
    // OP <symbol> OVER <operand> -> <return type> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Operator {
            unary: false,
            body: 7,
        },
    },
    // UNARY OP <symbol> OVER <operand> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Operator {
            unary: true,
            body: 6,
        },
    },
    // UNARY OP <symbol> OVER <operand> -> <return type> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Operator {
            unary: true,
            body: 8,
        },
    },
    // LET <name> = OP <symbol> OVER <operand> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Operator {
            unary: false,
            body: 8,
        },
    },
    // LET <name> = OP <symbol> OVER <operand> -> <return type> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Operator {
            unary: false,
            body: 10,
        },
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Operator {
            unary: true,
            body: 9,
        },
    },
    // LET <name> = UNARY OP <symbol> OVER <operand> -> <return type> = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.let_),
            Slot,
            Kw(&KEYWORDS.equals),
            Kw(&KEYWORDS.unary),
            Kw(&KEYWORDS.op),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::Operator {
            unary: true,
            body: 11,
        },
    },
    // MATCH <scrutinee> -> <result type> WITH <branches>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.match_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.with),
            Slot,
        ],
        rule: FormRule::Arms { arms: 5 },
    },
    // TRY <body> -> <result type> WITH <branches>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.try_),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.with),
            Slot,
        ],
        rule: FormRule::MemberArms { arms: 5 },
    },
    // MATCH <scrutinee> OVER <union> -> <result type> WITH <branches>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.match_),
            Slot,
            Kw(&KEYWORDS.over),
            Slot,
            Kw(&KEYWORDS.arrow),
            Slot,
            Kw(&KEYWORDS.with),
            Slot,
        ],
        rule: FormRule::MemberArms { arms: 7 },
    },
    // MODULE <name> = <body>
    FormSpec {
        key: &[Kw(&KEYWORDS.module), Slot, Kw(&KEYWORDS.equals), Slot],
        rule: FormRule::ModuleBody { body: 3 },
    },
    // GROUP <name> FOLD LEFT|RIGHT = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::ModuleBody { body: 5 },
    },
    FormSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.fold),
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::ModuleBody { body: 5 },
    },
    // GROUP <name> PAIRWISE FOLD <combiner> LEFT|RIGHT = <body>
    FormSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.left),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::ModuleBody { body: 7 },
    },
    FormSpec {
        key: &[
            Kw(&KEYWORDS.group),
            Slot,
            Kw(&KEYWORDS.pairwise),
            Kw(&KEYWORDS.fold),
            Slot,
            Kw(&KEYWORDS.right),
            Kw(&KEYWORDS.equals),
            Slot,
        ],
        rule: FormRule::ModuleBody { body: 7 },
    },
    // ATTR <record> <field> — the parse of `m.x`.
    FormSpec {
        key: &[Kw(&KEYWORDS.attr), Slot, Slot],
        rule: FormRule::Attribute { field: 2 },
    },
    // <field list> FROM <record>
    FormSpec {
        key: &[Slot, Kw(&KEYWORDS.from), Slot],
        rule: FormRule::Projection { fields: 0 },
    },
    // CLOSE OVER <captures> <body>
    FormSpec {
        key: &[Kw(&KEYWORDS.close), Kw(&KEYWORDS.over), Slot, Slot],
        rule: FormRule::ExplicitClose {
            captures: 2,
            body: 3,
        },
    },
    // CLOSE <body>
    FormSpec {
        key: &[Kw(&KEYWORDS.close), Slot],
        rule: FormRule::InferredClose { body: 1 },
    },
    // USING <module> SCOPE <body>
    FormSpec {
        key: &[Kw(&KEYWORDS.using), Slot, Kw(&KEYWORDS.scope), Slot],
        rule: FormRule::Dynamic(DynamicNameForm::Using),
    },
    // EVAL <expr> — the parse of `$(expr)`.
    FormSpec {
        key: &[Kw(&KEYWORDS.eval), Slot],
        rule: FormRule::Dynamic(DynamicNameForm::Eval),
    },
];

/// The [`FORM_SPECS`] entry `expression`'s bucket key matches, or `None` when the generic walk
/// reads every slot correctly. The one table probe.
fn form_rule_for(expression: &KExpression<'_>) -> Option<&'static FormRule> {
    FORM_SPECS
        .iter()
        .find(|spec| key_matches_parts(spec.key, expression.parts))
        .map(|spec| &spec.rule)
}

// ---------- the scope stack ----------

/// A name one of the walk's open scopes binds. `at` is the statement index that installs it;
/// `None` is block-wide — visible to every statement of the scope regardless of order.
struct Binding {
    name: BinderSymbol,
    at: Option<usize>,
}

/// One open scope: where its bindings start in the flat stack, and the statement index the walk
/// currently sits at inside it.
struct OpenScope {
    start: usize,
    at: usize,
}

struct Walk<'s, 'r> {
    scratch: BumpAllocator<'s>,
    registries: &'r RunRegistries,
    scopes: BumpVec<'s, OpenScope>,
    /// Every open scope's bindings, innermost scope last. Flat rather than per-scope so a scope
    /// costs one `OpenScope` and its names push onto one run.
    bindings: BumpVec<'s, Binding>,
    values: BumpVec<'s, ValueSymbol>,
    types: BumpVec<'s, TypeSymbol>,
    conflict: Option<InferenceConflict>,
}

impl<'s> Walk<'s, '_> {
    /// The positional visibility rule, stated once: a binding at statement index `j` is visible to a
    /// reader at statement `c` of the same scope iff `j < c`, so a binder never binds its own
    /// statement's subtree. Block-wide bindings — a scope's seeded names, a module body's
    /// announcements, a nominal declaration's own names — ignore the gate.
    ///
    /// This is the walk's own copy of the interpreter's cutoff (`src/machine/core/bindings.rs`
    /// states it over runtime binding entries, which do not exist yet here). The two are held to
    /// each other by the behavioral tests, not by the type system.
    fn binds(&self, name: BinderSymbol) -> bool {
        let mut end = self.bindings.len();
        for scope in self.scopes.iter().rev() {
            let visible = self.bindings[scope.start..end]
                .iter()
                .any(|binding| binding.name == name && binding.at.is_none_or(|at| at < scope.at));
            if visible {
                return true;
            }
            end = scope.start;
        }
        false
    }

    fn push_scope(&mut self, seeded: &[BinderSymbol]) {
        self.scopes.push(OpenScope {
            start: self.bindings.len(),
            at: 0,
        });
        for name in seeded {
            self.bindings.push(Binding {
                name: *name,
                at: None,
            });
        }
    }

    fn pop_scope(&mut self) {
        let scope = self.scopes.pop().expect("every push is matched by one pop");
        self.bindings.truncate(scope.start);
    }

    fn use_value(&mut self, name: ValueSymbol) {
        if !self.binds(BinderSymbol::Value(name)) && !self.values.contains(&name) {
            self.values.push(name);
        }
    }

    fn use_type(&mut self, name: TypeSymbol) {
        if !self.binds(BinderSymbol::Type(name)) && !self.types.contains(&name) {
            self.types.push(name);
        }
    }

    // ---------- walking ----------

    /// Walk `body` as one scope. A statement block pre-scans its statements' declared names, so the
    /// namespace is known before the first statement is read; anything else is a single statement.
    fn walk_block(&mut self, body: &KExpression<'_>, seeded: &[BinderSymbol]) {
        self.push_scope(seeded);
        if body.is_statement_block() {
            let statements = body_statement_refs(body);
            for (index, statement) in statements.iter().enumerate() {
                if let Some(plan) = statement.statement_binder_plan()
                    && let Some(name) = plan.name
                {
                    self.bindings.push(Binding {
                        name,
                        at: Some(index),
                    });
                }
            }
            for (index, statement) in statements.iter().enumerate() {
                if let Some(scope) = self.scopes.last_mut() {
                    scope.at = index;
                }
                self.walk_node(statement);
            }
        } else {
            self.walk_node(body);
        }
        self.pop_scope();
    }

    fn walk_node(&mut self, expr: &KExpression<'_>) {
        if expr.is_statement_block() {
            return self.walk_block(expr, &[]);
        }
        if let Some(rule) = form_rule_for(expr) {
            return self.walk_form(expr, rule);
        }
        if let Some(surface) = announced_type_declaration(expr) {
            return self.walk_nominal_declaration(expr, surface);
        }
        self.walk_unclaimed(expr, &[]);
    }

    /// Walk every part a rule did not claim. The declared-name position of a binder form is claimed
    /// implicitly — it is the label the form installs, never a name it reads.
    fn walk_unclaimed(&mut self, expr: &KExpression<'_>, claimed: &[usize]) {
        for index in 0..expr.parts.len() {
            if claimed.contains(&index) || expr.binder_name_slot() == Some(index) {
                continue;
            }
            self.walk_part(expr, index);
        }
    }

    /// One part read at its slot position in `owner`, which is what decides whether a quote there is
    /// code or data.
    fn walk_part(&mut self, owner: &KExpression<'_>, index: usize) {
        let Some(part) = owner.parts.get(index) else {
            return;
        };
        // A quote filling a builtin's lazy CODE slot is that builtin's body spelled with a quote —
        // `stays_raw` admits both spellings — so it is walked as code. Anywhere else a quote is a
        // literal, and its body never resolves a name.
        if let ExpressionPart::QuotedExpression(node) = part.value {
            if owner.lazy_kinds_at(index).contains(LazyKinds::CODE) {
                self.walk_node(node.reference());
            }
            return;
        }
        self.walk_loose_part(&part.value);
    }

    /// One part read outside any slot position — a list item, a dict entry, a record-literal value.
    /// A quote here is always data.
    fn walk_loose_part(&mut self, part: &ExpressionPart<'_>) {
        match *part {
            ExpressionPart::Keyword(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::QuotedExpression(_) => {}
            ExpressionPart::Identifier(name) => self.use_value(name),
            ExpressionPart::Type(name) => self.use_type(name),
            ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node) => {
                self.walk_node(node.reference());
            }
            // A record type's inner run is the same `<name> :<Type>` shape a signature is: the
            // names are field labels, the annotations are type uses.
            ExpressionPart::RecordType(node) => {
                self.walk_pair_run(node.reference());
            }
            ExpressionPart::ListLiteral(items) => {
                for item in items {
                    self.walk_loose_part(item);
                }
            }
            ExpressionPart::DictLiteral(pairs) => {
                for (key, value) in pairs {
                    self.walk_loose_part(key);
                    self.walk_loose_part(value);
                }
            }
            // A record literal's keys are field labels the record itself holds, not names.
            ExpressionPart::RecordLiteral(fields) => {
                for (_, value) in fields {
                    self.walk_loose_part(value);
                }
            }
        }
    }

    /// The binders a `<name> :<Type>` run declares, walking every annotation as a use in the
    /// current scope. One reader for all three spellings of the shape — an `FN` signature group, an
    /// anonymous `FN`'s record schema, and a record type's field list — driven off the same
    /// [`SignatureScan`] stride `parse_fn_param_list` reads a signature with.
    fn walk_pair_run(&mut self, run: &KExpression<'_>) -> BumpVec<'s, BinderSymbol> {
        let mut declared = BumpVec::new_in(self.scratch);
        for position in SignatureScan::new(run.parts) {
            match position {
                SignaturePosition::Keyword(_) => {}
                SignaturePosition::Annotated { name, annotation } => {
                    declared.push(name);
                    self.walk_part(run, annotation);
                }
                SignaturePosition::Bare(name) => declared.push(name),
                SignaturePosition::Foreign(index) => self.walk_part(run, index),
            }
        }
        declared
    }

    fn walk_form(&mut self, expr: &KExpression<'_>, rule: &FormRule) {
        match *rule {
            FormRule::Signature { signature, body } => {
                let declared = self.signature_binders(expr, signature);
                match body {
                    Some(body) => {
                        self.walk_unclaimed(expr, &[signature, body]);
                        self.walk_body_slot(expr, body, &declared);
                    }
                    None => self.walk_unclaimed(expr, &[signature]),
                }
            }
            FormRule::Operator { unary, body } => {
                let mut seeded = BumpVec::new_in(self.scratch);
                if unary {
                    seeded.push(BinderSymbol::Value(MACHINE_BINDERS.operands.symbol()));
                } else {
                    seeded.push(BinderSymbol::Value(MACHINE_BINDERS.operand_left.symbol()));
                    seeded.push(BinderSymbol::Value(MACHINE_BINDERS.operand_right.symbol()));
                }
                self.walk_unclaimed(expr, &[body]);
                self.walk_body_slot(expr, body, &seeded);
            }
            FormRule::Arms { arms } => {
                self.walk_unclaimed(expr, &[arms]);
                match expr.parts.get(arms).map(|part| part.value) {
                    Some(ExpressionPart::Expression(node)) => {
                        self.walk_arms(node.reference(), true)
                    }
                    _ => self.walk_part(expr, arms),
                }
            }
            FormRule::MemberArms { arms } => {
                self.walk_unclaimed(expr, &[arms]);
                match expr.parts.get(arms).map(|part| part.value) {
                    Some(ExpressionPart::Expression(node)) => {
                        self.walk_arms(node.reference(), false)
                    }
                    _ => self.walk_part(expr, arms),
                }
            }
            FormRule::ModuleBody { body } => {
                self.walk_unclaimed(expr, &[body]);
                let announced = self.announced_binders(expr, body);
                self.walk_body_slot(expr, body, &announced);
            }
            FormRule::Attribute { field } => self.walk_unclaimed(expr, &[field]),
            FormRule::Projection { fields } => self.walk_unclaimed(expr, &[fields]),
            FormRule::ExplicitClose { captures, body } => {
                self.walk_unclaimed(expr, &[captures, body]);
                self.walk_capture_list(expr, captures);
            }
            FormRule::InferredClose { body } => {
                self.walk_unclaimed(expr, &[body]);
                if let Some(ExpressionPart::Expression(node)) =
                    expr.parts.get(body).map(|part| part.value)
                {
                    // The inner form's own conflicts stay its own: it raises them when it evaluates.
                    let inner =
                        infer_close_captures(node.reference(), self.scratch, self.registries);
                    for name in inner.values.iter() {
                        self.use_value(*name);
                    }
                    for name in inner.types.iter() {
                        self.use_type(*name);
                    }
                }
            }
            FormRule::Dynamic(form) => {
                self.conflict.get_or_insert(InferenceConflict {
                    form,
                    span: expr.span,
                    file: expr.file,
                });
            }
        }
    }

    /// The binders a declaration's signature slot declares. A parenthesized group is a keyworded
    /// signature; a record schema is the anonymous `FN :{…}` form, whose fields are its parameters.
    fn signature_binders(
        &mut self,
        expr: &KExpression<'_>,
        signature: usize,
    ) -> BumpVec<'s, BinderSymbol> {
        match expr.parts.get(signature).map(|part| part.value) {
            Some(ExpressionPart::Expression(node) | ExpressionPart::RecordType(node)) => {
                self.walk_pair_run(node.reference())
            }
            _ => {
                self.walk_part(expr, signature);
                BumpVec::new_in(self.scratch)
            }
        }
    }

    /// The type names a `MODULE` / `GROUP` body announces, which are visible throughout it whatever
    /// order its statements sit in. Read through [`announce_type_members`], the same scan the
    /// declaration's own dispatch pre-scans its window with.
    ///
    /// A body that announces nothing, one whose scan the declaration will reject, and a
    /// declaration named with a Type token — a hard error either way — all announce nothing here.
    fn announced_binders(
        &mut self,
        expr: &KExpression<'_>,
        body: usize,
    ) -> BumpVec<'s, BinderSymbol> {
        let mut names = BumpVec::new_in(self.scratch);
        let Some(ExpressionPart::Expression(node)) = expr.parts.get(body).map(|part| part.value)
        else {
            return names;
        };
        let Some(BinderSymbol::Value(module)) = expr.binder_plan().and_then(|plan| plan.name)
        else {
            return names;
        };
        let Ok(Some(announced)) = announce_type_members(node.reference(), module, self.registries)
        else {
            return names;
        };
        for (member, _) in &announced.members {
            names.push(BinderSymbol::Type(*member));
        }
        for (binder, _) in &announced.binders {
            names.push(BinderSymbol::Type(*binder));
        }
        names
    }

    /// A nominal declaration's own names are visible inside its representation — `NEWTYPE Tree =
    /// :{left :Tree}` and a `UNION`'s variant tags both refer to what the statement is declaring.
    fn walk_nominal_declaration(
        &mut self,
        expr: &KExpression<'_>,
        surface: TypeDeclarationSurface,
    ) {
        let representation = expr.parts.len() - 1;
        self.walk_unclaimed(expr, &[representation]);

        let mut declared = BumpVec::new_in(self.scratch);
        if let Some(name) = expr.binder_name_from_type_part() {
            declared.push(BinderSymbol::Type(name));
        }
        if surface == TypeDeclarationSurface::Union
            && let Some(schema) = union_schema(expr)
            && let Ok(tags) = pair_list_names(&schema, "UNION schema", self.registries)
        {
            for tag in tags {
                declared.push(BinderSymbol::Type(tag));
            }
        }
        self.push_scope(&declared);
        self.walk_part(expr, representation);
        self.pop_scope();
    }

    /// A `<head> -> <body>` arm run: heads name types, each body runs in a scope holding the arm
    /// binder. A run the branch walker itself would reject falls back to the generic walk, which is
    /// the conservative reading — its own dispatch surfaces the real diagnostic.
    fn walk_arms(&mut self, run: &KExpression<'_>, heads_are_uses: bool) {
        if !run.parts.len().is_multiple_of(3) {
            return self.walk_unclaimed(run, &[]);
        }
        let arm = [BinderSymbol::Value(MACHINE_BINDERS.arm.symbol())];
        for head in (0..run.parts.len()).step_by(3) {
            if heads_are_uses {
                self.walk_part(run, head);
            }
            self.walk_body_slot(run, head + 2, &arm);
        }
    }

    /// A `CLOSE OVER` capture list read as uses against this chain. A parenthesized entry is a
    /// signature-shaped pattern naming a bucket key, which contributes no name.
    fn walk_capture_list(&mut self, expr: &KExpression<'_>, captures: usize) {
        let Some(ExpressionPart::Expression(node)) =
            expr.parts.get(captures).map(|part| part.value)
        else {
            return;
        };
        for part in node.reference().parts.iter() {
            match part.value {
                ExpressionPart::Identifier(name) => self.use_value(name),
                ExpressionPart::Type(name) => self.use_type(name),
                _ => {}
            }
        }
    }

    /// Walk the body slot at `index` as a scope seeded with `seeded`. A body is a group or the quote
    /// spelling of one; anything else is read as an ordinary part, which seeds nothing because there
    /// is no block to seed.
    fn walk_body_slot(&mut self, owner: &KExpression<'_>, index: usize, seeded: &[BinderSymbol]) {
        match owner.parts.get(index).map(|part| part.value) {
            Some(ExpressionPart::Expression(node)) => self.walk_block(node.reference(), seeded),
            Some(ExpressionPart::QuotedExpression(node))
                if owner.lazy_kinds_at(index).contains(LazyKinds::CODE) =>
            {
                self.walk_block(node.reference(), seeded);
            }
            _ => self.walk_part(owner, index),
        }
    }
}

#[cfg(test)]
mod tests;
