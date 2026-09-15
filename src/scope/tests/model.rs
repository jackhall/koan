//! The oracle the property laws check shapes against: a program generated as data, rendered to
//! koan source, and analysed on its own terms.
//!
//! The analysis resolves each name by walking the model's own scopes outward — each scope at the
//! position the path into it reads at — and classifies each mention by the context rule, with no
//! notion of captures, coordinates or slots. Its components are mutual-reachability classes found by
//! brute force. What the builder computes is compared against this, never restated.

use std::collections::BTreeSet;

use proptest::prelude::*;

/// The names a binder may take: few, so repeats, shadows and forward references are common.
pub(super) const BINDERS: &[&str] = &["aa", "bb", "cc", "dd", "ee", "ff"];

/// The names a read may take: every binder name, an arm's parameter, and a builtin.
pub(super) const READS: &[&str] = &["aa", "bb", "cc", "dd", "ee", "ff", "it", "origin"];

#[derive(Clone, Debug)]
pub(super) enum Statement {
    Let(usize, Expr),
    Function {
        name: usize,
        parameters: Vec<usize>,
        body: Vec<Statement>,
    },
    Bare(Expr),
}

#[derive(Clone, Debug)]
pub(super) enum Expr {
    Name(usize),
    Number,
    Quote(usize),
    List(Vec<Expr>),
    Call(Vec<Expr>),
    Lambda {
        parameters: Vec<usize>,
        body: Vec<Statement>,
    },
    Arm {
        scrutinee: Box<Expr>,
        body: Vec<Statement>,
    },
    Eval(Box<Expr>),
}

pub(super) fn expr() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        8 => (0..BINDERS.len()).prop_map(Expr::Name),
        1 => (BINDERS.len()..READS.len()).prop_map(Expr::Name),
        1 => Just(Expr::Number),
        1 => (0..BINDERS.len()).prop_map(Expr::Quote),
    ];
    leaf.prop_recursive(3, 24, 3, |inner| {
        let body = proptest::collection::vec(
            prop_oneof![
                2 => (0..BINDERS.len(), inner.clone()).prop_map(|(name, rhs)| Statement::Let(name, rhs)),
                1 => inner.clone().prop_map(Statement::Bare),
            ],
            1..3,
        );
        prop_oneof![
            2 => proptest::collection::vec(inner.clone(), 0..3).prop_map(Expr::List),
            2 => proptest::collection::vec(inner.clone(), 1..3).prop_map(Expr::Call),
            2 => (proptest::collection::vec(0..BINDERS.len(), 0..2), body.clone())
                .prop_map(|(parameters, body)| Expr::Lambda { parameters, body }),
            2 => (inner.clone(), body).prop_map(|(scrutinee, body)| Expr::Arm {
                scrutinee: Box::new(scrutinee),
                body,
            }),
            1 => inner.prop_map(|inner| Expr::Eval(Box::new(inner))),
        ]
    })
}

pub(super) fn statement() -> impl Strategy<Value = Statement> {
    let simple = || {
        prop_oneof![
            3 => (0..BINDERS.len(), expr()).prop_map(|(name, rhs)| Statement::Let(name, rhs)),
            1 => expr().prop_map(Statement::Bare),
        ]
    };
    prop_oneof![
        4 => simple(),
        1 => (
            0..BINDERS.len(),
            proptest::collection::vec(0..BINDERS.len(), 0..2),
            proptest::collection::vec(simple(), 1..3),
        )
            .prop_map(|(name, parameters, body)| Statement::Function {
                name,
                parameters,
                body,
            }),
    ]
}

/// A function reading a later binding that reads the function back — eagerly through a call, or
/// deferred through a list — so both kinds of cycle turn up among the random statements.
fn knot() -> impl Strategy<Value = Vec<Statement>> {
    (0..BINDERS.len(), 1..BINDERS.len(), any::<bool>()).prop_map(|(function, offset, eager)| {
        let reader = (function + offset) % BINDERS.len();
        let reads_function = if eager {
            Expr::Call(vec![Expr::Name(function)])
        } else {
            Expr::List(vec![Expr::Name(function)])
        };
        vec![
            Statement::Function {
                name: function,
                parameters: Vec::new(),
                body: vec![Statement::Bare(Expr::Name(reader))],
            },
            Statement::Let(reader, reads_function),
        ]
    })
}

pub(super) fn program() -> impl Strategy<Value = Vec<Statement>> {
    let statements = |count| proptest::collection::vec(statement(), count);
    prop_oneof![
        3 => statements(1..6),
        1 => (statements(0..3), knot(), statements(0..3)).prop_map(|(mut program, knot, after)| {
            program.extend(knot);
            program.extend(after);
            program
        }),
    ]
}

/// A program the model shapes, for the laws that run what a shape describes.
pub(super) fn shaped_program() -> impl Strategy<Value = Vec<Statement>> {
    program().prop_filter("the model shapes it", |program| {
        analyze_program(program).is_ok()
    })
}

// ---------- rendering ----------

pub(super) fn render_program(statements: &[Statement]) -> String {
    statements
        .iter()
        .map(render_statement)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_statement(statement: &Statement) -> String {
    match statement {
        Statement::Let(name, rhs) => format!("LET {} = {}", BINDERS[*name], render_expr(rhs)),
        Statement::Function {
            name,
            parameters,
            body,
        } => {
            let parameters: Vec<_> = parameters
                .iter()
                .map(|parameter| format!(" {} :Number", BINDERS[*parameter]))
                .collect();
            format!(
                "LET {} = FN EXPR (ZZ{}) -> Number = {}",
                BINDERS[*name],
                parameters.concat(),
                render_body(body)
            )
        }
        Statement::Bare(expr) => format!("({})", render_expr(expr)),
    }
}

pub(super) fn render_body(statements: &[Statement]) -> String {
    match statements {
        [only] => format!("({})", render_statement(only)),
        many => {
            let wrapped: Vec<_> = many
                .iter()
                .map(|statement| format!("({})", render_statement(statement)))
                .collect();
            format!("({})", wrapped.join(" "))
        }
    }
}

fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Name(name) => READS[*name].to_owned(),
        Expr::Number => "1".to_owned(),
        Expr::Quote(name) => format!("#({})", BINDERS[*name]),
        Expr::List(items) => {
            let items: Vec<_> = items.iter().map(render_expr).collect();
            format!("[{}]", items.join(" "))
        }
        Expr::Call(arguments) => {
            let arguments: Vec<_> = arguments.iter().map(render_expr).collect();
            format!("(ZZ {})", arguments.join(" "))
        }
        Expr::Lambda { parameters, body } => {
            let parameters: Vec<_> = parameters
                .iter()
                .map(|parameter| format!("{} :Number", BINDERS[*parameter]))
                .collect();
            format!(
                "(FN :{{{}}} -> Number = {})",
                parameters.join(", "),
                render_body(body)
            )
        }
        Expr::Arm { scrutinee, body } => format!(
            "(MATCH {} -> :Number WITH (Number -> {}))",
            render_expr(scrutinee),
            render_body(body)
        ),
        Expr::Eval(inner) => format!("(EVAL {})", render_expr(inner)),
    }
}

// ---------- analysis ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Program,
    Callable,
    Block,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Class {
    Eager,
    Deferred,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Root,
    Deferred,
    Eager,
}

impl State {
    fn constructor(self) -> State {
        match self {
            State::Eager => State::Eager,
            State::Root | State::Deferred => State::Deferred,
        }
    }

    fn class(self) -> Class {
        match self {
            State::Deferred => Class::Deferred,
            State::Root | State::Eager => Class::Eager,
        }
    }
}

/// Where a model mention's name is bound.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Resolved {
    Builtin,
    /// The binder `name` of the scope `up` scopes out from the reader's.
    Binder {
        up: usize,
        name: &'static str,
    },
}

#[derive(Clone, Debug)]
pub(super) struct ModelMention {
    pub name: &'static str,
    pub statement: u32,
    pub class: Class,
    pub resolved: Resolved,
}

#[derive(Clone, Debug)]
pub(super) struct ModelScope {
    pub kind: Kind,
    /// Every binder and the position it writes at: parameters at `0`, statement `i` at `i + 1`.
    pub binders: Vec<(&'static str, u32)>,
    pub statements: u32,
    pub statement_binder: Vec<Option<&'static str>>,
    /// The statement of the enclosing scope this one sits in.
    pub parent_statement: u32,
    /// The value-name mentions this scope's own statements make, in walk order.
    pub mentions: Vec<ModelMention>,
    /// The nested scopes, in walk order.
    pub children: Vec<ModelScope>,
    /// `(binder, bound, class)` for every mention resolving to one of this scope's binders.
    pub edges: Vec<(&'static str, &'static str, Class)>,
    pub keeps_defining_scope: bool,
    current: (u32, Class),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum ModelError {
    Rebind(&'static str),
    Unbound(&'static str, u32),
    EagerCycle(BTreeSet<&'static str>),
}

impl ModelScope {
    /// Where the path into the nested scope being walked reads in this one.
    fn boundary(&self) -> u32 {
        match self.current.1 {
            Class::Deferred => self.statements + 1,
            Class::Eager => self.current.0 + 1,
        }
    }

    /// The mutual-reachability classes of the binders, each with whether its internal edges are
    /// all deferred and whether it has any internal edge at all.
    pub fn components(&self) -> Vec<(BTreeSet<&'static str>, bool, bool)> {
        let names: Vec<_> = self.binders.iter().map(|(name, _)| *name).collect();
        let reaches = |from: &str, to: &str| {
            let mut seen = BTreeSet::from([from]);
            let mut stack = vec![from];
            while let Some(at) = stack.pop() {
                for (binder, bound, _) in &self.edges {
                    if *binder == at && seen.insert(bound) {
                        stack.push(bound);
                    }
                }
            }
            seen.contains(to)
        };
        let mut out: Vec<(BTreeSet<&'static str>, bool, bool)> = Vec::new();
        for name in &names {
            if out.iter().any(|(members, _, _)| members.contains(name)) {
                continue;
            }
            let members: BTreeSet<_> = names
                .iter()
                .copied()
                .filter(|other| reaches(name, other) && reaches(other, name))
                .collect();
            let internal: Vec<_> = self
                .edges
                .iter()
                .filter(|(binder, bound, _)| members.contains(binder) && members.contains(bound))
                .collect();
            let deferred_only = internal
                .iter()
                .all(|(_, _, class)| *class == Class::Deferred);
            let cyclic = !internal.is_empty();
            out.push((members, deferred_only, cyclic));
        }
        out
    }

    pub fn component_of(&self, name: &str) -> BTreeSet<&'static str> {
        self.components()
            .into_iter()
            .find(|(members, _, _)| members.contains(name))
            .expect("every binder is in a component")
            .0
    }
}

/// The analysis of a whole program.
pub(super) fn analyze_program(statements: &[Statement]) -> Result<ModelScope, ModelError> {
    let mut stack = Vec::new();
    analyze(&mut stack, Kind::Program, &[], statements)
}

/// The analysis of an `EVAL` body read in the innermost scope of `site`, outermost first: each scope
/// beside the statement the path into the next one reads at, every such path eager.
pub(super) fn analyze_eval(
    site: &[(&ModelScope, u32)],
    body: &[Statement],
) -> Result<ModelScope, ModelError> {
    let mut stack = site
        .iter()
        .map(|(scope, statement)| ModelScope {
            mentions: Vec::new(),
            children: Vec::new(),
            edges: Vec::new(),
            current: (*statement, Class::Eager),
            ..(*scope).clone()
        })
        .collect();
    analyze(&mut stack, Kind::Block, &[], body)
}

fn analyze(
    stack: &mut Vec<ModelScope>,
    kind: Kind,
    parameters: &[&'static str],
    statements: &[Statement],
) -> Result<ModelScope, ModelError> {
    let mut binders: Vec<(&'static str, u32)> = Vec::new();
    let statement_binder: Vec<_> = statements
        .iter()
        .map(|statement| match statement {
            Statement::Let(name, _) | Statement::Function { name, .. } => Some(BINDERS[*name]),
            Statement::Bare(_) => None,
        })
        .collect();
    let declared = parameters.iter().map(|name| (Some(*name), 0)).chain(
        statement_binder
            .iter()
            .enumerate()
            .map(|(index, name)| (*name, index as u32 + 1)),
    );
    for (name, position) in declared {
        let Some(name) = name else { continue };
        if binders.iter().any(|(bound, _)| *bound == name) {
            return Err(ModelError::Rebind(name));
        }
        binders.push((name, position));
    }
    let parent_statement = stack.last().map_or(u32::MAX, |parent| parent.current.0);
    stack.push(ModelScope {
        kind,
        binders,
        statements: statements.len() as u32,
        statement_binder,
        parent_statement,
        mentions: Vec::new(),
        children: Vec::new(),
        edges: Vec::new(),
        keeps_defining_scope: false,
        current: (0, Class::Eager),
    });
    let walked = statements
        .iter()
        .enumerate()
        .try_for_each(|(index, statement)| walk_statement(stack, index as u32, statement));
    let scope = stack.pop().expect("pushed above");
    walked?;
    let refused = scope
        .components()
        .into_iter()
        .filter(|(_, deferred_only, cyclic)| *cyclic && !deferred_only)
        .min_by_key(|(members, _, _)| {
            members
                .iter()
                .map(|member| {
                    scope
                        .binders
                        .iter()
                        .find(|(name, _)| name == member)
                        .unwrap()
                        .1
                })
                .min()
        });
    if let Some((members, _, _)) = refused {
        return Err(ModelError::EagerCycle(members));
    }
    Ok(scope)
}

fn walk_statement(
    stack: &mut Vec<ModelScope>,
    index: u32,
    statement: &Statement,
) -> Result<(), ModelError> {
    match statement {
        Statement::Let(_, rhs) | Statement::Bare(rhs) => walk_expr(stack, index, rhs, State::Root),
        Statement::Function {
            parameters, body, ..
        } => enter(
            stack,
            index,
            Kind::Callable,
            Class::Deferred,
            parameters,
            body,
        ),
    }
}

fn walk_expr(
    stack: &mut Vec<ModelScope>,
    index: u32,
    expr: &Expr,
    state: State,
) -> Result<(), ModelError> {
    match expr {
        Expr::Name(name) => mention(stack, index, READS[*name], state),
        Expr::Number | Expr::Quote(_) => Ok(()),
        Expr::List(items) => items
            .iter()
            .try_for_each(|item| walk_expr(stack, index, item, state.constructor())),
        Expr::Call(arguments) => arguments
            .iter()
            .try_for_each(|argument| walk_expr(stack, index, argument, State::Eager)),
        Expr::Lambda { parameters, body } => {
            let class = match state {
                State::Eager => Class::Eager,
                State::Root | State::Deferred => Class::Deferred,
            };
            enter(stack, index, Kind::Callable, class, parameters, body)
        }
        Expr::Arm { scrutinee, body } => {
            walk_expr(stack, index, scrutinee, State::Eager)?;
            let top = stack.len() - 1;
            stack[top].current = (index, Class::Eager);
            let child = analyze(stack, Kind::Block, &["it"], body)?;
            stack[top].children.push(child);
            Ok(())
        }
        Expr::Eval(inner) => {
            for scope in stack.iter_mut() {
                scope.keeps_defining_scope = true;
            }
            walk_expr(stack, index, inner, State::Eager)
        }
    }
}

fn enter(
    stack: &mut Vec<ModelScope>,
    index: u32,
    kind: Kind,
    class: Class,
    parameters: &[usize],
    body: &[Statement],
) -> Result<(), ModelError> {
    let top = stack.len() - 1;
    stack[top].current = (index, class);
    let parameters: Vec<_> = parameters.iter().map(|name| BINDERS[*name]).collect();
    let child = analyze(stack, kind, &parameters, body)?;
    stack[top].children.push(child);
    Ok(())
}

fn mention(
    stack: &mut [ModelScope],
    index: u32,
    name: &'static str,
    state: State,
) -> Result<(), ModelError> {
    let class = state.class();
    let top = stack.len() - 1;
    let resolved = if name == "origin" {
        Resolved::Builtin
    } else {
        let at = match class {
            Class::Eager => index + 1,
            Class::Deferred => stack[top].statements + 1,
        };
        let found = (0..stack.len()).find(|up| {
            let scope = &stack[top - up];
            let position = if *up == 0 { at } else { scope.boundary() };
            scope
                .binders
                .iter()
                .any(|(bound, declared)| *bound == name && *declared < position)
        });
        let Some(up) = found else {
            return Err(ModelError::Unbound(name, index));
        };
        let scope = &mut stack[top - up];
        let (statement, path) = if up == 0 {
            (index, class)
        } else {
            scope.current
        };
        if let Some(binder) = scope.statement_binder[statement as usize] {
            scope.edges.push((binder, name, path));
        }
        Resolved::Binder { up, name }
    };
    stack[top].mentions.push(ModelMention {
        name,
        statement: index,
        class,
        resolved,
    });
    Ok(())
}
