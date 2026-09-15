//! Shape plans: what a program's shapes are to be, generated as data and rendered to koan source for
//! the builder to shape back.
//!
//! A plan is valid by construction. Names are unique across the program. Each scope partitions its
//! binders into components first, then plans the reads between them: a cycle through every member
//! of a component, a component's internal reads and every read of a later binding deferred, and
//! reads between components following one order of them. A nested scope reads an enclosing scope's
//! binding only where that scope planned the read, and reads an enclosing parameter or a builtin —
//! which can close no cycle — where it likes. So a plan's components, classes and resolutions are
//! what the builder must find, and nothing about them is re-derived from the rendered source.
//!
//! A perturbation of a valid plan injects exactly one refusal; the shadowing perturbation re-declares
//! an enclosing name inside a nested scope.

use std::collections::BTreeSet;

use proptest::prelude::*;

use crate::parse::{BinderSymbol, LabelInterner};

use super::{type_name, value_name};

/// A plan's generated choices. An exhausted stream always chooses `0`, the simplest option, so
/// shrinking the stream shrinks the plan.
pub(super) fn choices() -> impl Strategy<Value = Vec<u32>> {
    proptest::collection::vec(any::<u32>(), 0..512)
}

/// The deepest a scope nests below the program.
const MAX_DEPTH: usize = 3;

/// How many nested scopes one plan opens, give or take the last statement's.
const MAX_SCOPES: usize = 10;

// ---------- names ----------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Name {
    /// A value binder, spelled `aa`, `ab`, …
    Value(u32),
    /// A type binder, spelled `Taa`, `Tab`, …
    Type(u32),
    /// An arm's matched value.
    It,
    /// A builtin, or a name nothing declares.
    Fixed(&'static str),
}

impl Name {
    pub fn text(self) -> String {
        let letters = |index: u32| {
            assert!(
                index < 8 * 26,
                "a plan spells at most 208 names of a channel"
            );
            let first = char::from(b'a' + (index / 26) as u8);
            let second = char::from(b'a' + (index % 26) as u8);
            format!("{first}{second}")
        };
        match self {
            Name::Value(index) => letters(index),
            Name::Type(index) => format!("T{}", letters(index)),
            Name::It => "it".to_owned(),
            Name::Fixed(text) => text.to_owned(),
        }
    }

    pub fn is_type(self) -> bool {
        match self {
            Name::Value(_) | Name::It => false,
            Name::Type(_) => true,
            Name::Fixed(text) => text.starts_with(|first: char| first.is_ascii_uppercase()),
        }
    }

    pub fn symbol(self, labels: &LabelInterner) -> BinderSymbol {
        let text = self.text();
        if self.is_type() {
            BinderSymbol::Type(type_name(&text, labels))
        } else {
            BinderSymbol::Value(value_name(&text, labels))
        }
    }
}

const ORIGIN: Name = Name::Fixed("origin");
const NUMBER: Name = Name::Fixed("Number");
const BUILTIN_TYPES: &[Name] = &[NUMBER, Name::Fixed("Null"), Name::Fixed("Str")];

// ---------- the plan ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Class {
    Eager,
    Deferred,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Program,
    Callable,
    Arm,
    Eval,
}

/// Where a read resolves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Lands {
    Builtin,
    /// A binder of the scope `up` scopes out from the reader's.
    Binder {
        up: usize,
    },
    /// Nowhere: the one read a refusal injects.
    Nowhere,
}

/// One mention: its name, its class in the scope it sits in, and where it lands.
#[derive(Clone, Copy, Debug)]
pub(super) struct Read {
    pub name: Name,
    pub class: Class,
    pub lands: Lands,
    /// An eager value read wrapped in `EVAL`.
    pub eval: bool,
}

impl Read {
    fn builtin(name: Name, class: Class) -> Read {
        Read {
            name,
            class,
            lands: Lands::Builtin,
            eval: false,
        }
    }

    fn local(name: Name, class: Class) -> Read {
        Read {
            name,
            class,
            lands: Lands::Binder { up: 0 },
            eval: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Scope {
    pub kind: Kind,
    pub parameters: Vec<Name>,
    pub statements: Vec<Statement>,
    /// The components the parameters and binders fall into, each parameter alone.
    pub components: Vec<BTreeSet<Name>>,
}

#[derive(Clone, Debug)]
pub(super) struct Statement {
    pub binder: Option<Name>,
    pub form: Form,
}

#[derive(Clone, Debug)]
pub(super) enum Form {
    /// `LET x = <carriers>`.
    Let(Vec<Carrier>),
    /// `LET x = FN EXPR (ZZ <signature>) -> <returns> = <body>`.
    Function(Callable),
    /// `UNION Tx = (<tag> :<read> …)`, every read deferred.
    Union(Vec<Read>),
    /// `(<carriers>)`.
    Bare(Vec<Carrier>),
}

/// A callable: its signature's parameter types and return type, read eagerly where it sits, and its
/// body, whose parameters are the signature's names.
#[derive(Clone, Debug)]
pub(super) struct Callable {
    pub types: Vec<Read>,
    pub returns: Read,
    pub body: Scope,
}

/// What a statement's right-hand side carries.
#[derive(Clone, Debug)]
pub(super) enum Carrier {
    /// A read made here: a list element when deferred, a call argument when eager.
    Read(Read),
    /// A `FN` value: in a list when deferred, called or passed when eager.
    Lambda(Class, Callable),
    /// A `MATCH` over the scrutinee (or `1`) with its type and each arm's head read eagerly.
    Arm {
        scrutinee: Option<Read>,
        ty: Read,
        arms: Vec<(Read, Scope)>,
    },
}

impl Scope {
    /// Every parameter at `0` and every statement's binder at the statement's position.
    pub fn binders(&self) -> Vec<(Name, u32)> {
        let parameters = self.parameters.iter().map(|name| (*name, 0));
        let statements = self
            .statements
            .iter()
            .enumerate()
            .filter_map(|(index, statement)| Some((statement.binder?, index as u32 + 1)));
        parameters.chain(statements).collect()
    }

    /// The nested scopes, in source order.
    pub fn children(&self) -> Vec<&Scope> {
        let mut out = Vec::new();
        for statement in &self.statements {
            match &statement.form {
                Form::Function(callable) => out.push(&callable.body),
                Form::Let(carriers) | Form::Bare(carriers) => {
                    for carrier in carriers {
                        match carrier {
                            Carrier::Read(_) => {}
                            Carrier::Lambda(_, callable) => out.push(&callable.body),
                            Carrier::Arm { arms, .. } => {
                                out.extend(arms.iter().map(|(_, body)| body))
                            }
                        }
                    }
                }
                Form::Union(_) => {}
            }
        }
        out
    }

    fn children_mut(&mut self) -> Vec<&mut Scope> {
        let mut out = Vec::new();
        for statement in &mut self.statements {
            match &mut statement.form {
                Form::Function(callable) => out.push(&mut callable.body),
                Form::Let(carriers) | Form::Bare(carriers) => {
                    for carrier in carriers {
                        match carrier {
                            Carrier::Read(_) => {}
                            Carrier::Lambda(_, callable) => out.push(&mut callable.body),
                            Carrier::Arm { arms, .. } => {
                                out.extend(arms.iter_mut().map(|(_, body)| body))
                            }
                        }
                    }
                }
                Form::Union(_) => {}
            }
        }
        out
    }

    /// Every read this scope's own statements make.
    fn reads_mut(&mut self) -> Vec<&mut Read> {
        let mut out = Vec::new();
        for statement in &mut self.statements {
            match &mut statement.form {
                Form::Function(callable) => {
                    out.extend(callable.types.iter_mut());
                    out.push(&mut callable.returns);
                }
                Form::Union(reads) => out.extend(reads.iter_mut()),
                Form::Let(carriers) | Form::Bare(carriers) => {
                    for carrier in carriers {
                        match carrier {
                            Carrier::Read(read) => out.push(read),
                            Carrier::Lambda(_, callable) => {
                                out.extend(callable.types.iter_mut());
                                out.push(&mut callable.returns);
                            }
                            Carrier::Arm {
                                scrutinee,
                                ty,
                                arms,
                            } => {
                                out.extend(scrutinee.iter_mut());
                                out.push(ty);
                                out.extend(arms.iter_mut().map(|(head, _)| head));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Whether an `EVAL` sits in this scope or any scope nested in it.
    pub fn keeps_defining_scope(&self) -> bool {
        let here = self
            .statements
            .iter()
            .any(|statement| match &statement.form {
                Form::Let(carriers) | Form::Bare(carriers) => carriers
                    .iter()
                    .any(|carrier| matches!(carrier, Carrier::Read(read) if read.eval)),
                Form::Function(_) | Form::Union(_) => false,
            });
        here || self
            .children()
            .iter()
            .any(|child| child.keeps_defining_scope())
    }

    /// The scope at `path` of child indices below this one.
    fn at_path(&mut self, path: &[usize]) -> &mut Scope {
        match path.split_first() {
            None => self,
            Some((first, rest)) => self
                .children_mut()
                .into_iter()
                .nth(*first)
                .expect("a path names a child")
                .at_path(rest),
        }
    }

    /// Every scope's path below this one, with its depth, outermost first.
    fn paths(&self, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        out.push(path.clone());
        for (index, child) in self.children().into_iter().enumerate() {
            path.push(index);
            child.paths(path, out);
            path.pop();
        }
    }

    /// Rename every read of `from` in this scope and every scope nested in it.
    fn rename_reads(&mut self, from: Name, to: Name) {
        for read in self.reads_mut() {
            if read.name == from {
                read.name = to;
            }
        }
        for child in self.children_mut() {
            child.rename_reads(from, to);
        }
    }

    /// Whether this scope or a scope nested in it reads `name`.
    fn reads_name(&mut self, name: Name) -> bool {
        self.reads_mut().iter().any(|read| read.name == name)
            || self
                .children_mut()
                .into_iter()
                .any(|child| child.reads_name(name))
    }

    /// Rename the binder or parameter `from` wherever this scope declares it.
    fn rename_binder(&mut self, from: Name, to: Name) {
        for parameter in &mut self.parameters {
            if *parameter == from {
                *parameter = to;
            }
        }
        for statement in &mut self.statements {
            if statement.binder == Some(from) {
                statement.binder = Some(to);
            }
        }
        for component in &mut self.components {
            if component.remove(&from) {
                component.insert(to);
            }
        }
    }
}

// ---------- generation ----------

/// What a statement is while its reads are planned.
#[derive(Clone, PartialEq, Eq)]
enum Draft {
    Let,
    Function(Vec<Name>),
    Union,
    Bare,
}

pub(super) struct Generator<'c> {
    stream: &'c [u32],
    at: usize,
    values: u32,
    types: u32,
    scopes: usize,
}

impl<'c> Generator<'c> {
    pub fn new(stream: &'c [u32]) -> Self {
        Generator {
            stream,
            at: 0,
            values: 0,
            types: 0,
            scopes: 0,
        }
    }

    /// A choice below `count`.
    pub fn pick(&mut self, count: usize) -> usize {
        let choice = self.stream.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        if count == 0 {
            0
        } else {
            choice as usize % count
        }
    }

    /// One time in `one_in`, never once the stream runs out.
    fn chance(&mut self, one_in: usize) -> bool {
        self.pick(one_in) == one_in - 1
    }

    fn fresh(&mut self, is_type: bool) -> Name {
        if is_type {
            self.types += 1;
            Name::Type(self.types - 1)
        } else {
            self.values += 1;
            Name::Value(self.values - 1)
        }
    }

    fn nestable(&self, depth: usize) -> bool {
        depth < MAX_DEPTH && self.scopes < MAX_SCOPES
    }

    pub fn program(&mut self) -> Scope {
        self.scope(Kind::Program, 0, Vec::new(), &[], Vec::new())
    }

    /// An `EVAL` body read at depth `depth`, where `visible` are the names the site sees, each with
    /// the depth of the scope declaring it.
    pub fn eval_body(&mut self, depth: usize, visible: &[(Name, usize)]) -> Scope {
        let count = if visible.is_empty() { 0 } else { self.pick(4) };
        let obligations = (0..count)
            .map(|_| {
                let (name, declared) = visible[self.pick(visible.len())];
                (name, Some(declared))
            })
            .collect();
        self.scope(Kind::Eval, depth, Vec::new(), visible, obligations)
    }

    /// One scope at `depth`. `visible` are the enclosing names it may read wherever it likes (each
    /// with its declaring depth), and `obligations` the names it must read somewhere — an enclosing
    /// binder with its declaring depth because the scope declaring it planned that read, or a
    /// builtin.
    fn scope(
        &mut self,
        kind: Kind,
        depth: usize,
        parameters: Vec<Name>,
        visible: &[(Name, usize)],
        obligations: Vec<(Name, Option<usize>)>,
    ) -> Scope {
        let count = 1 + self.pick(if kind == Kind::Program { 6 } else { 3 });
        let mut drafts = Vec::with_capacity(count);
        for _ in 0..count {
            drafts.push(match self.pick(8) {
                3 | 4 => Draft::Bare,
                5 if self.nestable(depth) => {
                    self.scopes += 1;
                    let parameters = (0..self.pick(3)).map(|_| self.fresh(false)).collect();
                    Draft::Function(parameters)
                }
                6 => Draft::Union,
                7 if kind == Kind::Program => Draft::Union,
                _ => Draft::Let,
            });
        }
        let needs_value = obligations.iter().any(|(name, _)| !name.is_type());
        if needs_value && drafts.iter().all(|draft| *draft == Draft::Union) {
            drafts[count - 1] = Draft::Let;
        }
        let binders: Vec<Option<Name>> = drafts
            .iter()
            .map(|draft| match draft {
                Draft::Let | Draft::Function(_) => Some(self.fresh(false)),
                Draft::Union => Some(self.fresh(true)),
                Draft::Bare => None,
            })
            .collect();

        // The components, and one order of them.
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut group_of = vec![usize::MAX; count];
        for (index, binder) in binders.iter().enumerate() {
            let Some(binder) = binder else { continue };
            let same: Vec<usize> = (0..groups.len())
                .filter(|group| {
                    binders[groups[*group][0]]
                        .is_some_and(|other| other.is_type() == binder.is_type())
                })
                .collect();
            let join = if binder.is_type() { 2 } else { 3 };
            let group = if !same.is_empty() && self.chance(join) {
                same[self.pick(same.len())]
            } else {
                groups.push(Vec::new());
                groups.len() - 1
            };
            groups[group].push(index);
            group_of[index] = group;
        }
        let order: Vec<(usize, usize)> = (0..groups.len())
            .map(|group| (self.pick(4), group))
            .collect();

        let mut reads: Vec<Vec<Read>> = vec![Vec::new(); count];
        let binder = |index: usize| binders[index].expect("a member binds");

        // Within a component: a cycle through every member, a self-read, an extra read, all deferred.
        for group in &groups {
            if group.len() > 1 {
                for (place, &reader) in group.iter().enumerate() {
                    let bound = group[(place + 1) % group.len()];
                    reads[reader].push(Read::local(binder(bound), Class::Deferred));
                }
                if self.chance(3) {
                    let reader = group[self.pick(group.len())];
                    let bound = group[self.pick(group.len())];
                    reads[reader].push(Read::local(binder(bound), Class::Deferred));
                }
            } else if self.chance(3) {
                reads[group[0]].push(Read::local(binder(group[0]), Class::Deferred));
            }
        }
        // Between components: toward a later one in the order, deferred when the bound is not yet
        // declared.
        for reader in 0..count {
            if binders[reader].is_none() {
                continue;
            }
            for _ in 0..self.pick(3) {
                let candidates: Vec<usize> = (0..count)
                    .filter(|bound| {
                        binders[*bound].is_some()
                            && order[group_of[*bound]] > order[group_of[reader]]
                            && may_read(&drafts[reader], binder(*bound))
                    })
                    .collect();
                if candidates.is_empty() {
                    break;
                }
                let bound = candidates[self.pick(candidates.len())];
                let class = self.class(&drafts[reader], binder(bound), bound < reader);
                reads[reader].push(Read::local(binder(bound), class));
            }
        }
        // A bare statement reads any binder of its scope.
        for reader in 0..count {
            if drafts[reader] != Draft::Bare {
                continue;
            }
            for _ in 0..self.pick(3) {
                let bound = self.pick(count);
                if let Some(name) = binders[bound] {
                    let class = self.class(&drafts[reader], name, bound < reader);
                    reads[reader].push(Read::local(name, class));
                }
            }
        }
        // Parameters, enclosing parameters and builtins, which close no cycle.
        for reader in 0..count {
            if !parameters.is_empty() && self.chance(4) {
                let name = parameters[self.pick(parameters.len())];
                if may_read(&drafts[reader], name) {
                    let class = self.class(&drafts[reader], name, true);
                    reads[reader].push(Read::local(name, class));
                }
            }
            if !visible.is_empty() && self.chance(4) {
                let (name, declared) = visible[self.pick(visible.len())];
                if may_read(&drafts[reader], name) {
                    let class = self.class(&drafts[reader], name, true);
                    reads[reader].push(Read {
                        name,
                        class,
                        lands: Lands::Binder {
                            up: depth - declared,
                        },
                        eval: false,
                    });
                }
            }
            if self.chance(4) || (drafts[reader] == Draft::Union && reads[reader].is_empty()) {
                let name = if drafts[reader] == Draft::Union || self.chance(2) {
                    BUILTIN_TYPES[self.pick(BUILTIN_TYPES.len())]
                } else {
                    ORIGIN
                };
                let class = self.class(&drafts[reader], name, true);
                reads[reader].push(Read::builtin(name, class));
            }
        }
        // The names this scope was handed.
        for (name, declared) in obligations {
            let hosts: Vec<usize> = (0..count)
                .filter(|reader| may_read(&drafts[*reader], name))
                .collect();
            let reader = hosts[self.pick(hosts.len())];
            let class = self.class(&drafts[reader], name, true);
            let lands = match declared {
                Some(declared) => Lands::Binder {
                    up: depth - declared,
                },
                None => Lands::Builtin,
            };
            reads[reader].push(Read {
                name,
                class,
                lands,
                eval: false,
            });
        }
        for (reader, own) in reads.iter_mut().enumerate() {
            if matches!(drafts[reader], Draft::Let | Draft::Bare) {
                for read in own.iter_mut() {
                    read.eval =
                        read.class == Class::Eager && !read.name.is_type() && self.chance(6);
                }
            }
        }

        // What the scopes nested here may read wherever they like.
        let enclosing: Vec<(Name, usize)> = visible
            .iter()
            .copied()
            .chain(parameters.iter().map(|name| (*name, depth)))
            .collect();
        let mut statements = Vec::with_capacity(count);
        for (index, draft) in drafts.into_iter().enumerate() {
            let own = std::mem::take(&mut reads[index]);
            let form = match draft {
                Draft::Union => Form::Union(own),
                Draft::Function(parameters) => {
                    let (signature, routed) =
                        own.into_iter().partition(|read| read.class == Class::Eager);
                    Form::Function(self.callable(depth, parameters, signature, routed, &enclosing))
                }
                Draft::Let => Form::Let(self.carriers(depth, own, &enclosing)),
                Draft::Bare => Form::Bare(self.carriers(depth, own, &enclosing)),
            };
            statements.push(Statement {
                binder: binders[index],
                form,
            });
        }
        let components = parameters
            .iter()
            .map(|name| BTreeSet::from([*name]))
            .chain(
                groups
                    .iter()
                    .map(|group| group.iter().map(|member| binder(*member)).collect()),
            )
            .collect();
        Scope {
            kind,
            parameters,
            statements,
            components,
        }
    }

    /// The class of a read of `name` by a statement drafted as `draft`: eager at random where the
    /// form can read it eagerly and `declared_before` the reader, deferred otherwise.
    fn class(&mut self, draft: &Draft, name: Name, declared_before: bool) -> Class {
        let eager = declared_before
            && match draft {
                Draft::Union => false,
                Draft::Function(_) => name.is_type(),
                Draft::Let | Draft::Bare => true,
            };
        if eager && self.chance(2) {
            Class::Eager
        } else {
            Class::Deferred
        }
    }

    /// A callable whose signature holds the eager type reads and whose body the routed reads.
    fn callable(
        &mut self,
        depth: usize,
        mut parameters: Vec<Name>,
        mut signature: Vec<Read>,
        routed: Vec<Read>,
        enclosing: &[(Name, usize)],
    ) -> Callable {
        while parameters.len() + 1 < signature.len() {
            parameters.push(self.fresh(false));
        }
        let number = Read::builtin(NUMBER, Class::Eager);
        let types = parameters
            .iter()
            .map(|_| signature.pop().unwrap_or(number))
            .collect();
        let returns = signature.pop().unwrap_or(number);
        let obligations = obligations(depth, &routed);
        let body = self.scope(
            Kind::Callable,
            depth + 1,
            parameters,
            enclosing,
            obligations,
        );
        Callable {
            types,
            returns,
            body,
        }
    }

    /// A `LET` or bare statement's carriers: each read made here, or routed into a nested scope
    /// whose boundary gives it the same class here.
    fn carriers(
        &mut self,
        depth: usize,
        own: Vec<Read>,
        enclosing: &[(Name, usize)],
    ) -> Vec<Carrier> {
        let mut direct = Vec::new();
        let mut deferred = Vec::new();
        let mut eager = Vec::new();
        for read in own {
            if !read.eval && self.nestable(depth) && self.chance(3) {
                match read.class {
                    Class::Deferred => deferred.push(read),
                    Class::Eager => eager.push(read),
                }
            } else {
                direct.push(read);
            }
        }
        let mut nested = Vec::new();
        if !deferred.is_empty() || (self.nestable(depth) && self.chance(6)) {
            self.scopes += 1;
            let signature = self.take_types(&mut direct);
            let callable = self.callable(depth, Vec::new(), signature, deferred, enclosing);
            nested.push(Carrier::Lambda(Class::Deferred, callable));
        }
        // A nested arm declares its own `it`, so a read of an enclosing `it` routes through a lambda.
        let (called, matched) = if self.chance(2) {
            (eager, Vec::new())
        } else {
            eager.into_iter().partition(|read| read.name == Name::It)
        };
        if !called.is_empty() || (self.nestable(depth) && self.chance(6)) {
            self.scopes += 1;
            let signature = self.take_types(&mut direct);
            let callable = self.callable(depth, Vec::new(), signature, called, enclosing);
            nested.push(Carrier::Lambda(Class::Eager, callable));
        }
        if !matched.is_empty() || (self.nestable(depth) && self.chance(6)) {
            self.scopes += 1;
            nested.push(self.arm(depth, matched, &mut direct, enclosing));
        }
        direct
            .into_iter()
            .map(Carrier::Read)
            .chain(nested)
            .collect()
    }

    /// Up to two of `direct`'s eager type reads, for a signature.
    fn take_types(&mut self, direct: &mut Vec<Read>) -> Vec<Read> {
        let mut taken = Vec::new();
        for _ in 0..2 {
            let found = direct
                .iter()
                .position(|read| read.class == Class::Eager && read.name.is_type());
            match found {
                Some(index) if self.chance(2) => taken.push(direct.remove(index)),
                _ => break,
            }
        }
        taken
    }

    fn arm(
        &mut self,
        depth: usize,
        routed: Vec<Read>,
        direct: &mut Vec<Read>,
        enclosing: &[(Name, usize)],
    ) -> Carrier {
        let mut take = |generator: &mut Self, is_type: bool| {
            let found = direct.iter().position(|read| {
                read.class == Class::Eager && !read.eval && read.name.is_type() == is_type
            });
            match found {
                Some(index) if generator.chance(2) => Some(direct.remove(index)),
                _ => None,
            }
        };
        let scrutinee = take(self, false);
        let number = Read::builtin(NUMBER, Class::Eager);
        let ty = take(self, true).unwrap_or(number);
        let count = 1 + self.pick(2);
        let mut bodies: Vec<Vec<Read>> = vec![Vec::new(); count];
        for read in routed {
            bodies[self.pick(count)].push(read);
        }
        let inner: Vec<(Name, usize)> = enclosing
            .iter()
            .copied()
            .filter(|(name, _)| *name != Name::It)
            .collect();
        let arms = bodies
            .into_iter()
            .map(|routed| {
                let head = take(self, true).unwrap_or(number);
                let body = self.scope(
                    Kind::Arm,
                    depth + 1,
                    vec![Name::It],
                    &inner,
                    obligations(depth, &routed),
                );
                (head, body)
            })
            .collect();
        Carrier::Arm {
            scrutinee,
            ty,
            arms,
        }
    }
}

/// Whether a statement drafted as `draft` can read `name` at all.
fn may_read(draft: &Draft, name: Name) -> bool {
    *draft != Draft::Union || name.is_type()
}

/// What reads routed out of a scope at `depth` hand the nested scope: each name, with the depth
/// declaring it unless it is a builtin.
fn obligations(depth: usize, routed: &[Read]) -> Vec<(Name, Option<usize>)> {
    routed
        .iter()
        .map(|read| match read.lands {
            Lands::Binder { up } => (read.name, Some(depth - up)),
            Lands::Builtin => (read.name, None),
            Lands::Nowhere => unreachable!("a valid plan routes no refused read"),
        })
        .collect()
}

// ---------- perturbations ----------

/// The refusal a perturbed plan must be refused with.
#[derive(Clone, Debug)]
pub(super) enum Refusal {
    Rebind {
        name: Name,
        first: u32,
        second: u32,
    },
    ShadowsBuiltin {
        name: Name,
        at: u32,
    },
    /// The read landing [`Lands::Nowhere`], in this statement of its scope.
    Unbound {
        name: Name,
        statement: u32,
    },
    EagerCycle(BTreeSet<Name>),
}

impl Generator<'_> {
    /// Inject refusal `which` (modulo the five kinds) into `program`.
    pub fn refuse(&mut self, program: &mut Scope, which: usize) -> Refusal {
        let mut paths = Vec::new();
        program.paths(&mut Vec::new(), &mut paths);
        match which % 5 {
            0 => self.eager_cycle(program, &paths),
            1 => self.read_ahead(program, &paths),
            2 => self.undeclared(program, &paths),
            3 => self.rebind(program, &paths),
            _ => self.shadow_builtin(program, &paths),
        }
    }

    /// Make a deferred read closing a component's cycle eager.
    fn eager_cycle(&mut self, program: &mut Scope, paths: &[Vec<usize>]) -> Refusal {
        let mut candidates = Vec::new();
        for path in paths {
            let scope = program.at_path(path);
            let binders = scope.binders();
            let declared = |name: Name| {
                binders
                    .iter()
                    .find(|(bound, _)| *bound == name)
                    .map(|(_, at)| *at)
            };
            for (index, statement) in scope.statements.iter().enumerate() {
                let (Some(reader), Form::Let(carriers)) = (statement.binder, &statement.form)
                else {
                    continue;
                };
                let component = scope
                    .components
                    .iter()
                    .find(|component| component.contains(&reader))
                    .expect("a binder sits in a component");
                for (place, carrier) in carriers.iter().enumerate() {
                    if let Carrier::Read(read) = carrier
                        && read.lands == (Lands::Binder { up: 0 })
                        && component.len() > 1
                        && component.contains(&read.name)
                        && declared(read.name).is_some_and(|at| at <= index as u32)
                    {
                        candidates.push((path.clone(), index, place, component.clone()));
                    }
                }
            }
        }
        if candidates.is_empty() {
            let (first, second) = (self.fresh(false), self.fresh(false));
            program.statements.push(Statement {
                binder: Some(first),
                form: Form::Let(vec![Carrier::Read(Read::local(second, Class::Deferred))]),
            });
            program.statements.push(Statement {
                binder: Some(second),
                form: Form::Let(vec![Carrier::Read(Read::local(first, Class::Eager))]),
            });
            let members = BTreeSet::from([first, second]);
            program.components.push(members.clone());
            return Refusal::EagerCycle(members);
        }
        let (path, index, place, members) = candidates.swap_remove(self.pick(candidates.len()));
        let Form::Let(carriers) = &mut program.at_path(&path).statements[index].form else {
            unreachable!("the candidate is a LET");
        };
        let Carrier::Read(read) = &mut carriers[place] else {
            unreachable!("the candidate is a read");
        };
        read.class = Class::Eager;
        read.eval = false;
        Refusal::EagerCycle(members)
    }

    /// The statements of every scope a read can be added to: a `LET` or a bare one.
    fn hosts(program: &mut Scope, paths: &[Vec<usize>]) -> Vec<(Vec<usize>, usize)> {
        let mut hosts = Vec::new();
        for path in paths {
            for (index, statement) in program.at_path(path).statements.iter().enumerate() {
                if matches!(statement.form, Form::Let(_) | Form::Bare(_)) {
                    hosts.push((path.clone(), index));
                }
            }
        }
        hosts
    }

    fn inject(program: &mut Scope, path: &[usize], index: usize, read: Read) {
        match &mut program.at_path(path).statements[index].form {
            Form::Let(carriers) | Form::Bare(carriers) => carriers.push(Carrier::Read(read)),
            Form::Function(_) | Form::Union(_) => unreachable!("reads are injected into a host"),
        }
    }

    /// Read a binder eagerly at or before its own statement.
    fn read_ahead(&mut self, program: &mut Scope, paths: &[Vec<usize>]) -> Refusal {
        let mut candidates = Vec::new();
        for (path, index) in Self::hosts(program, paths) {
            let scope = program.at_path(&path);
            for (later, statement) in scope.statements.iter().enumerate().skip(index) {
                if let Some(binder) = statement.binder {
                    candidates.push((path.clone(), index, later, binder));
                }
            }
        }
        if candidates.is_empty() {
            return self.undeclared(program, paths);
        }
        let (path, index, _, name) = candidates.swap_remove(self.pick(candidates.len()));
        let read = Read {
            name,
            class: Class::Eager,
            lands: Lands::Nowhere,
            eval: false,
        };
        Self::inject(program, &path, index, read);
        Refusal::Unbound {
            name,
            statement: index as u32,
        }
    }

    /// Read a name nothing declares.
    fn undeclared(&mut self, program: &mut Scope, paths: &[Vec<usize>]) -> Refusal {
        let name = if self.chance(2) {
            Name::Fixed("Tzz")
        } else {
            Name::Fixed("zz")
        };
        let class = if self.chance(2) {
            Class::Eager
        } else {
            Class::Deferred
        };
        let read = Read {
            name,
            class,
            lands: Lands::Nowhere,
            eval: false,
        };
        let hosts = Self::hosts(program, paths);
        let (path, index) = if hosts.is_empty() {
            program.statements.push(Statement {
                binder: None,
                form: Form::Bare(Vec::new()),
            });
            (Vec::new(), program.statements.len() - 1)
        } else {
            hosts[self.pick(hosts.len())].clone()
        };
        Self::inject(program, &path, index, read);
        Refusal::Unbound {
            name,
            statement: index as u32,
        }
    }

    /// Declare a later binder of a scope with an earlier one's name.
    fn rebind(&mut self, program: &mut Scope, paths: &[Vec<usize>]) -> Refusal {
        let mut candidates = Vec::new();
        for path in paths {
            let binders = program.at_path(path).binders();
            for (place, (first, at)) in binders.iter().enumerate() {
                for (second, again) in &binders[place + 1..] {
                    let parameters = *at == 0 && *again == 0;
                    let named = *first != Name::It && *second != Name::It;
                    if first.is_type() == second.is_type() && !parameters && named {
                        candidates.push((path.clone(), *first, *at, *second, *again));
                    }
                }
            }
        }
        if candidates.is_empty() {
            let name = self.fresh(false);
            for _ in 0..2 {
                program.statements.push(Statement {
                    binder: Some(name),
                    form: Form::Let(Vec::new()),
                });
            }
            let at = program.statements.len() as u32;
            return Refusal::Rebind {
                name,
                first: at - 1,
                second: at,
            };
        }
        let (path, name, first, renamed, second) =
            candidates.swap_remove(self.pick(candidates.len()));
        program.at_path(&path).rename_binder(renamed, name);
        Refusal::Rebind {
            name,
            first,
            second,
        }
    }

    /// Declare a binder with a builtin's name.
    fn shadow_builtin(&mut self, program: &mut Scope, paths: &[Vec<usize>]) -> Refusal {
        let mut candidates = Vec::new();
        for path in paths {
            for (name, at) in program.at_path(path).binders() {
                if name != Name::It {
                    candidates.push((path.clone(), name, at));
                }
            }
        }
        if candidates.is_empty() {
            program.statements.push(Statement {
                binder: Some(ORIGIN),
                form: Form::Let(Vec::new()),
            });
            return Refusal::ShadowsBuiltin {
                name: ORIGIN,
                at: program.statements.len() as u32,
            };
        }
        let (path, renamed, at) = candidates.swap_remove(self.pick(candidates.len()));
        let name = if renamed.is_type() { NUMBER } else { ORIGIN };
        program.at_path(&path).rename_binder(renamed, name);
        Refusal::ShadowsBuiltin { name, at }
    }

    /// Re-declare an enclosing name inside a nested scope: rename one of the nested scope's binders
    /// to it, and read it by that name both inside the nested scope and in the scope declaring the
    /// enclosing binder. Each read lands on the binder nearest it.
    pub fn shadow(&mut self, program: &mut Scope) {
        let mut candidates = Self::shadow_candidates(program);
        if candidates.is_empty() {
            let outer = self.fresh(false);
            let inner = self.fresh(false);
            program.statements.push(Statement {
                binder: Some(outer),
                form: Form::Let(Vec::new()),
            });
            program.components.push(BTreeSet::from([outer]));
            let body = Scope {
                kind: Kind::Callable,
                parameters: vec![inner],
                statements: vec![Statement {
                    binder: None,
                    form: Form::Bare(Vec::new()),
                }],
                components: vec![BTreeSet::from([inner])],
            };
            let number = Read::builtin(NUMBER, Class::Eager);
            let callable = Callable {
                types: vec![number],
                returns: number,
                body,
            };
            program.statements.push(Statement {
                binder: None,
                form: Form::Bare(vec![Carrier::Lambda(Class::Deferred, callable)]),
            });
            candidates = Self::shadow_candidates(program);
        }
        let (path, declaring, renamed, name) = candidates.swap_remove(self.pick(candidates.len()));
        let nested = program.at_path(&path);
        nested.rename_reads(renamed, name);
        nested.rename_binder(renamed, name);
        nested.statements.push(Statement {
            binder: None,
            form: Form::Bare(vec![Carrier::Read(Read::local(name, Class::Deferred))]),
        });
        program
            .at_path(&path[..declaring])
            .statements
            .push(Statement {
                binder: None,
                form: Form::Bare(vec![Carrier::Read(Read::local(name, Class::Deferred))]),
            });
    }

    /// `(nested path, declaring depth, nested binder, enclosing name)` for every re-declaration that
    /// leaves every existing read landing where it did.
    fn shadow_candidates(program: &mut Scope) -> Vec<(Vec<usize>, usize, Name, Name)> {
        let mut paths = Vec::new();
        program.paths(&mut Vec::new(), &mut paths);
        let mut candidates = Vec::new();
        for path in paths.iter().filter(|path| !path.is_empty()) {
            let mut enclosing = Vec::new();
            for depth in 0..path.len() {
                for (name, _) in program.at_path(&path[..depth]).binders() {
                    if name != Name::It {
                        enclosing.push((depth, name));
                    }
                }
            }
            let nested = program.at_path(path);
            for (renamed, _) in nested.binders() {
                if renamed == Name::It {
                    continue;
                }
                for (depth, name) in &enclosing {
                    if name.is_type() == renamed.is_type() && !nested.reads_name(*name) {
                        candidates.push((path.clone(), *depth, renamed, *name));
                    }
                }
            }
        }
        candidates
    }
}

// ---------- rendering ----------

/// What the renderer wrote at each name or nested body, in source order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Token {
    /// The read at this index of [`Rendering::reads`].
    Mention(usize),
    /// A name that is no mention: a binder, a parameter, a tag.
    Other,
    /// The body of the scope at this index of [`Rendering::scopes`] opens.
    Open(usize),
    Close,
}

pub(super) struct Placed<'p> {
    pub read: &'p Read,
    pub scope: usize,
    pub statement: u32,
}

pub(super) struct Rendered<'p> {
    pub scope: &'p Scope,
    pub parent: Option<usize>,
    /// The statement of the parent this scope sits in.
    pub statement: u32,
    /// The scope's depth in the chain it is checked in.
    pub level: usize,
}

pub(super) struct Rendering<'p> {
    pub source: String,
    pub tokens: Vec<Token>,
    pub reads: Vec<Placed<'p>>,
    pub scopes: Vec<Rendered<'p>>,
}

struct Renderer<'p> {
    out: Rendering<'p>,
    scope: usize,
    statement: u32,
}

/// A program's source: one line per statement.
pub(super) fn render_program(program: &Scope) -> Rendering<'_> {
    let mut renderer = Renderer::new(program, 0);
    for (index, statement) in program.statements.iter().enumerate() {
        if index > 0 {
            renderer.text("\n");
        }
        renderer.statement(index, statement);
    }
    renderer.out
}

/// An `EVAL` body's quoted source, its scope checked at `level`.
pub(super) fn render_eval(body: &Scope, level: usize) -> Rendering<'_> {
    let mut renderer = Renderer::new(body, level);
    renderer.text("#");
    renderer.statements(body);
    renderer.out
}

impl<'p> Renderer<'p> {
    fn new(root: &'p Scope, level: usize) -> Self {
        Renderer {
            out: Rendering {
                source: String::new(),
                tokens: Vec::new(),
                reads: Vec::new(),
                scopes: vec![Rendered {
                    scope: root,
                    parent: None,
                    statement: 0,
                    level,
                }],
            },
            scope: 0,
            statement: 0,
        }
    }

    fn text(&mut self, text: &str) {
        self.out.source.push_str(text);
    }

    fn other(&mut self, name: Name) {
        self.text(&name.text());
        self.out.tokens.push(Token::Other);
    }

    fn read(&mut self, read: &'p Read) {
        self.out.tokens.push(Token::Mention(self.out.reads.len()));
        self.out.reads.push(Placed {
            read,
            scope: self.scope,
            statement: self.statement,
        });
        self.text(&read.name.text());
    }

    fn body(&mut self, scope: &'p Scope) {
        let index = self.out.scopes.len();
        self.out.scopes.push(Rendered {
            scope,
            parent: Some(self.scope),
            statement: self.statement,
            level: self.out.scopes[self.scope].level + 1,
        });
        self.out.tokens.push(Token::Open(index));
        let (scope_was, statement_was) = (self.scope, self.statement);
        self.scope = index;
        self.statements(scope);
        (self.scope, self.statement) = (scope_was, statement_was);
        self.out.tokens.push(Token::Close);
    }

    /// A body's statements: `(<statement>)` for one, `((<statement>) …)` for more.
    fn statements(&mut self, scope: &'p Scope) {
        self.text("(");
        match scope.statements.as_slice() {
            [only] => self.statement(0, only),
            many => {
                for (index, statement) in many.iter().enumerate() {
                    self.text(if index == 0 { "(" } else { " (" });
                    self.statement(index, statement);
                    self.text(")");
                }
            }
        }
        self.text(")");
    }

    fn statement(&mut self, index: usize, statement: &'p Statement) {
        self.statement = index as u32;
        match &statement.form {
            Form::Let(carriers) => {
                self.text("LET ");
                self.other(statement.binder.expect("a LET binds"));
                self.text(" = ");
                self.carriers(carriers);
            }
            Form::Function(callable) => {
                self.text("LET ");
                self.other(statement.binder.expect("a function binds"));
                self.text(" = FN EXPR (ZZ");
                for (parameter, ty) in callable.body.parameters.iter().zip(&callable.types) {
                    self.text(" ");
                    self.other(*parameter);
                    self.text(" :");
                    self.read(ty);
                }
                self.text(") -> ");
                self.read(&callable.returns);
                self.text(" = ");
                self.body(&callable.body);
            }
            Form::Union(reads) => {
                self.text("UNION ");
                self.other(statement.binder.expect("a UNION binds"));
                self.text(" = (");
                for (tag, read) in reads.iter().enumerate() {
                    let tag = char::from(b'a' + tag as u8);
                    self.text(&format!("{}K{tag} :", if tag == 'a' { "" } else { " " }));
                    self.out.tokens.push(Token::Other);
                    self.read(read);
                }
                self.text(")");
            }
            Form::Bare(carriers) => {
                self.text("(");
                self.carriers(carriers);
                self.text(")");
            }
        }
    }

    /// A right-hand side: the deferred carriers in a list, the eager ones as a call's arguments
    /// inside it — or a lone carrier where it stands.
    fn carriers(&mut self, carriers: &'p [Carrier]) {
        let (deferred, eager): (Vec<&Carrier>, Vec<&Carrier>) =
            carriers.iter().partition(|carrier| match carrier {
                Carrier::Read(read) => read.class == Class::Deferred,
                Carrier::Lambda(class, _) => *class == Class::Deferred,
                Carrier::Arm { .. } => false,
            });
        match (deferred.len(), eager.len()) {
            (0, 0) => self.text("1"),
            (0, 1) => match eager[0] {
                Carrier::Read(read) if !read.eval && !read.name.is_type() => self.read(read),
                Carrier::Read(read) if read.name.is_type() => self.call(&eager),
                Carrier::Lambda(_, callable) => {
                    self.text("(");
                    self.lambda(callable);
                    self.text(" 1)");
                }
                other => self.eager(other),
            },
            (0, _) => self.call(&eager),
            (1, 0) if matches!(deferred[0], Carrier::Lambda(..)) => {
                let Carrier::Lambda(_, callable) = deferred[0] else {
                    unreachable!("matched a lambda");
                };
                self.lambda(callable);
            }
            (_, _) => {
                self.text("[");
                for (index, carrier) in deferred.iter().copied().enumerate() {
                    if index > 0 {
                        self.text(" ");
                    }
                    match carrier {
                        Carrier::Read(read) => self.read(read),
                        Carrier::Lambda(_, callable) => self.lambda(callable),
                        Carrier::Arm { .. } => unreachable!("an arm is eager"),
                    }
                }
                if !eager.is_empty() {
                    self.text(" ");
                    self.call(&eager);
                }
                self.text("]");
            }
        }
    }

    fn call(&mut self, arguments: &[&'p Carrier]) {
        self.text("(ZZ");
        for &argument in arguments {
            self.text(" ");
            self.eager(argument);
        }
        self.text(")");
    }

    fn eager(&mut self, carrier: &'p Carrier) {
        match carrier {
            Carrier::Read(read) if read.eval => {
                self.text("(EVAL ");
                self.read(read);
                self.text(")");
            }
            Carrier::Read(read) => self.read(read),
            Carrier::Lambda(_, callable) => self.lambda(callable),
            Carrier::Arm {
                scrutinee,
                ty,
                arms,
            } => {
                self.text("(MATCH ");
                match scrutinee {
                    Some(read) => self.read(read),
                    None => self.text("1"),
                }
                self.text(" -> :");
                self.read(ty);
                self.text(" WITH (");
                for (index, (head, body)) in arms.iter().enumerate() {
                    if index > 0 {
                        self.text(" ");
                    }
                    self.read(head);
                    self.text(" -> ");
                    self.body(body);
                }
                self.text("))");
            }
        }
    }

    fn lambda(&mut self, callable: &'p Callable) {
        self.text("(FN :{");
        for (index, (parameter, ty)) in callable
            .body
            .parameters
            .iter()
            .zip(&callable.types)
            .enumerate()
        {
            if index > 0 {
                self.text(", ");
            }
            self.other(*parameter);
            self.text(" :");
            self.read(ty);
        }
        self.text("} -> ");
        self.read(&callable.returns);
        self.text(" = ");
        self.body(&callable.body);
        self.text(")");
    }
}
