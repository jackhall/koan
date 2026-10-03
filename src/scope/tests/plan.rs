//! Body-shape plans: what a program's shapes are to be, generated as data and rendered to koan source for
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
//! A plan also writes quote values. A quote's code is a scope its quote's reads are routed into as
//! a lambda's are, each read there through `$`; inside it, a plan adds holes, `\` names offered or
//! landing at a binder of the code, registrations, and keyworded uses at the four planned keys or
//! `ZZ`, each with the candidates its mark says it lists. The program scope registers too, so a
//! `$(…)` lists a program registration that sees its quote.
//!
//! A refused plan is generated with exactly one refusal injected into one of its scopes, where the
//! refused read may be routed into a nested scope like any other but never into a quote's code; the
//! shadowing perturbation re-declares an enclosing name inside a nested scope, a quote's code
//! included, where a `$` read in that code still lands outside it.

use std::collections::BTreeSet;

use proptest::prelude::*;

use crate::symbols::{BinderSymbol, SymbolInterner};

use super::{type_name, value_name};

/// A plan's generated choices. An exhausted stream always chooses `0`, the simplest option, so
/// shrinking the stream shrinks the plan.
pub(crate) fn choices() -> impl Strategy<Value = Vec<u32>> {
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

    pub fn symbol(self, symbols: &SymbolInterner) -> BinderSymbol {
        let text = self.text();
        if self.is_type() {
            BinderSymbol::Type(type_name(&text, symbols))
        } else {
            BinderSymbol::Value(value_name(&text, symbols))
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
    /// A quote value's code.
    Code,
}

/// How a name or keyworded use in a quote's code is marked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Marking {
    Unmarked,
    /// `$`: resolved where the quote is written.
    Written,
    /// `\`: resolved where the code is built.
    Built,
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
    /// A hole of the quote's code holding the read.
    Hole,
    /// A name the quote's code holding the read is offered where it is built.
    Offered,
}

/// One mention: its name, its class in the scope it sits in, how it is marked, and where it lands.
#[derive(Clone, Copy, Debug)]
pub(super) struct Read {
    pub name: Name,
    pub class: Class,
    pub mark: Marking,
    pub lands: Lands,
}

impl Read {
    fn builtin(name: Name, class: Class) -> Read {
        Read {
            name,
            class,
            mark: Marking::Unmarked,
            lands: Lands::Builtin,
        }
    }

    fn local(name: Name, class: Class) -> Read {
        Read {
            name,
            class,
            mark: Marking::Unmarked,
            lands: Lands::Binder { up: 0 },
        }
    }
}

/// One of the four keys a plan registers at, spelled `KAA _` … `KAD _`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Key(u8);

impl Key {
    fn text(self) -> String {
        format!("KA{}", char::from(b'A' + self.0))
    }
}

/// The key a keyworded use is written at: a planned one, or the suites' builtin `ZZ _`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Use {
    Key(Key),
    Builtin,
}

/// One candidate a keyworded use must list.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) enum Listed {
    Builtin,
    /// The registration at `statement` of the scope `up` scopes out from the use's.
    Registration {
        up: usize,
        statement: u32,
    },
    Hole,
    Offered,
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
    pub form: BuiltinShape,
}

impl Statement {
    /// The scopes nested in this statement, in source order.
    fn children(&self) -> Vec<&Scope> {
        let mut out = Vec::new();
        match &self.form {
            BuiltinShape::Function(callable) | BuiltinShape::Expr { body: callable, .. } => {
                out.push(&callable.body)
            }
            BuiltinShape::Let(carriers) | BuiltinShape::Bare(carriers) => {
                for carrier in carriers {
                    match carrier {
                        Carrier::Read(_) | Carrier::Use { .. } => {}
                        Carrier::Lambda(_, callable) => out.push(&callable.body),
                        Carrier::Quote(_, code) => out.push(code),
                        Carrier::Arm { arms, .. } => out.extend(arms.iter().map(|(_, body)| body)),
                    }
                }
            }
            BuiltinShape::Union(_) => {}
        }
        out
    }

    fn children_mut(&mut self) -> Vec<&mut Scope> {
        let mut out = Vec::new();
        match &mut self.form {
            BuiltinShape::Function(callable) | BuiltinShape::Expr { body: callable, .. } => {
                out.push(&mut callable.body)
            }
            BuiltinShape::Let(carriers) | BuiltinShape::Bare(carriers) => {
                for carrier in carriers {
                    match carrier {
                        Carrier::Read(_) | Carrier::Use { .. } => {}
                        Carrier::Lambda(_, callable) => out.push(&mut callable.body),
                        Carrier::Quote(_, code) => out.push(code),
                        Carrier::Arm { arms, .. } => {
                            out.extend(arms.iter_mut().map(|(_, body)| body))
                        }
                    }
                }
            }
            BuiltinShape::Union(_) => {}
        }
        out
    }
}

#[derive(Clone, Debug)]
pub(super) enum BuiltinShape {
    /// `LET x = <carriers>`.
    Let(Vec<Carrier>),
    /// `LET x = (FN :{<signature>} -> <returns> = #<body>)`.
    Function(Callable),
    /// `UNION Tx = #{<tag>: <read> …}`, every read deferred.
    Union(Vec<Read>),
    /// `(<carriers>)`.
    Bare(Vec<Carrier>),
    /// `EXPR #(<key> p :Number) -> Number = #(p)`: a registration, binding no name.
    Expr { key: Key, body: Callable },
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
    /// A quote value of the code scope: in a list when deferred, a call argument when eager. Its
    /// `$` reads take its class here.
    Quote(Class, Scope),
    /// A keyworded use of one argument, `(KAA 1)`, `$(KAA 1)` or `\(KAA 1)`, and the candidates it
    /// must list.
    Use {
        key: Use,
        mark: Marking,
        expected: Vec<Listed>,
    },
}

impl Scope {
    /// Every parameter at `0` and every statement's binder at the statement's position.
    pub fn binders(&self) -> Vec<(Name, u32)> {
        let binders: Vec<Option<Name>> = self
            .statements
            .iter()
            .map(|statement| statement.binder)
            .collect();
        declared(&binders, &self.parameters)
    }

    /// The nested scopes, in source order.
    pub fn children(&self) -> Vec<&Scope> {
        self.statements
            .iter()
            .flat_map(Statement::children)
            .collect()
    }

    fn children_mut(&mut self) -> Vec<&mut Scope> {
        self.statements
            .iter_mut()
            .flat_map(Statement::children_mut)
            .collect()
    }

    /// Every read this scope's own statements make.
    fn reads_mut(&mut self) -> Vec<&mut Read> {
        let mut out = Vec::new();
        for statement in &mut self.statements {
            match &mut statement.form {
                BuiltinShape::Function(callable) | BuiltinShape::Expr { body: callable, .. } => {
                    out.extend(callable.types.iter_mut());
                    out.push(&mut callable.returns);
                }
                BuiltinShape::Union(reads) => out.extend(reads.iter_mut()),
                BuiltinShape::Let(carriers) | BuiltinShape::Bare(carriers) => {
                    for carrier in carriers {
                        match carrier {
                            Carrier::Read(read) => out.push(read),
                            Carrier::Quote(..) | Carrier::Use { .. } => {}
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

    /// Whether this scope or a scope nested in it reads `name`. Where this scope is in a quote's
    /// code, `in_code`, a `$` read does not count, as it skips every binder of the code.
    fn reads_name(&mut self, name: Name, in_code: bool) -> bool {
        let counts = |read: &Read| read.name == name && !(in_code && read.mark == Marking::Written);
        self.reads_mut().into_iter().any(|read| counts(read))
            || self
                .children_mut()
                .into_iter()
                .any(|child| child.reads_name(name, in_code))
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
    Expr(Key),
}

pub(super) struct Generator<'c> {
    stream: &'c [u32],
    at: usize,
    values: u32,
    types: u32,
    scopes: usize,
    /// How many scopes generation has entered.
    entered: usize,
    /// The refusal to inject, and the ordinal of the scope it goes in.
    injection: Option<(Injection, usize)>,
    refusal: Option<Refusal>,
    /// The depth of the quote's code being generated, if any.
    code: Option<usize>,
    /// The program's registrations, by key and statement.
    registered: Vec<(Key, usize)>,
    /// The program statement being generated.
    statement: usize,
    /// The program registrations a `$(…)` in the code being generated lists, by key and statement.
    written: Vec<(Key, usize)>,
    /// Whether the plan writes quote values.
    quotes: bool,
}

impl<'c> Generator<'c> {
    pub fn new(stream: &'c [u32]) -> Self {
        Generator {
            stream,
            at: 0,
            values: 0,
            types: 0,
            scopes: 0,
            entered: 0,
            injection: None,
            refusal: None,
            code: None,
            registered: Vec::new(),
            statement: 0,
            written: Vec::new(),
            quotes: true,
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

    /// One scope at `depth`. `visible` are the enclosing names it may read wherever it likes (each
    /// with its declaring depth), and `obligations` the names it must read somewhere, each with
    /// where the read lands.
    fn scope(
        &mut self,
        kind: Kind,
        depth: usize,
        mut parameters: Vec<Name>,
        visible: &[(Name, usize)],
        obligations: Vec<(Name, Owed, Marking)>,
    ) -> Scope {
        let injection = self.claim();
        let mut count = 1 + self.pick(if kind == Kind::Program { 6 } else { 3 });
        if matches!(injection, Some(Injection::EagerCycle | Injection::Rebind)) {
            count = count.max(2);
        }
        let mut drafts = Vec::with_capacity(count);
        let registers = matches!(kind, Kind::Program | Kind::Code);
        for _ in 0..count {
            drafts.push(match self.pick(9) {
                3 | 4 => Draft::Bare,
                5 if self.nestable(depth) => {
                    self.scopes += 1;
                    let parameters = (0..self.pick(3)).map(|_| self.fresh(false)).collect();
                    Draft::Function(parameters)
                }
                6 => Draft::Union,
                7 if kind == Kind::Program => Draft::Union,
                8 if registers && self.nestable(depth) => {
                    self.scopes += 1;
                    Draft::Expr(Key(self.pick(4) as u8))
                }
                _ => Draft::Let,
            });
        }
        // A registration ahead of the statements, so a keyworded use often sees one.
        if registers && self.nestable(depth) && self.chance(2) {
            self.scopes += 1;
            drafts.insert(0, Draft::Expr(Key(self.pick(4) as u8)));
            count += 1;
        }
        let needs_value = obligations.iter().any(|(name, ..)| !name.is_type());
        let hosts = |drafts: &[Draft], value: bool| {
            drafts
                .iter()
                .any(|draft| !matches!(draft, Draft::Expr(_)) && (!value || *draft != Draft::Union))
        };
        if !obligations.is_empty() && !hosts(&drafts, needs_value) {
            drafts[count - 1] = Draft::Let;
        }
        if let Some(injection) = injection {
            make_room(injection, &mut drafts, &parameters);
        }
        let mut binders: Vec<Option<Name>> = drafts
            .iter()
            .map(|draft| declares(draft).map(|is_type| self.fresh(is_type)))
            .collect();
        match injection {
            Some(Injection::Rebind) => self.inject_rebind(&mut binders, &parameters),
            Some(Injection::ShadowsBuiltin) => {
                self.inject_shadow_builtin(&mut binders, &mut parameters)
            }
            _ => {}
        }

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
        if injection == Some(Injection::EagerCycle) && cyclable(&groups, &drafts).is_empty() {
            // Move a `LET` into the component of an earlier value binder.
            let mut pairs = Vec::new();
            for joining in 0..count {
                for host in 0..joining {
                    if drafts[joining] == Draft::Let && declares(&drafts[host]) == Some(false) {
                        pairs.push((joining, host));
                    }
                }
            }
            let (joining, host) = pairs[self.pick(pairs.len())];
            groups[group_of[joining]].retain(|member| *member != joining);
            let into = &mut groups[group_of[host]];
            let at = into.partition_point(|member| *member < joining);
            into.insert(at, joining);
            groups.retain(|members| !members.is_empty());
            for (group, members) in groups.iter().enumerate() {
                for member in members {
                    group_of[*member] = group;
                }
            }
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
        // The injected eager cycle: a `LET` of a component reading an earlier member eagerly.
        if injection == Some(Injection::EagerCycle) {
            let candidates = cyclable(&groups, &drafts);
            let group = &groups[candidates[self.pick(candidates.len())]];
            let readers: Vec<usize> = group[1..]
                .iter()
                .copied()
                .filter(|member| drafts[*member] == Draft::Let)
                .collect();
            let reader = readers[self.pick(readers.len())];
            let earlier: Vec<usize> = group.iter().copied().filter(|m| *m < reader).collect();
            let bound = earlier[self.pick(earlier.len())];
            let at = self.pick(reads[reader].len() + 1);
            reads[reader].insert(at, Read::local(binder(bound), Class::Eager));
            let members = group.iter().map(|member| binder(*member)).collect();
            self.refusal = Some(Refusal::EagerCycle(members));
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
            if matches!(drafts[reader], Draft::Expr(_)) {
                continue;
            }
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
                    // A name declared outside the quote's code reaches it only through `$`.
                    let outside = self.code.is_some_and(|code| declared < code);
                    reads[reader].push(Read {
                        name,
                        class,
                        mark: if outside {
                            Marking::Written
                        } else {
                            Marking::Unmarked
                        },
                        lands: Lands::Binder {
                            up: depth - declared,
                        },
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
        // Inside a quote's code: a hole, and a name the build is offered.
        if let Some(code) = self.code {
            let outside: Vec<Name> = (visible.iter())
                .filter(|(_, declared)| *declared < code)
                .map(|(name, _)| *name)
                .chain([Name::Fixed("zz"), Name::Fixed("Tzz")])
                .collect();
            for reader in 0..count {
                for (mark, lands) in [
                    (Marking::Unmarked, Lands::Hole),
                    (Marking::Built, Lands::Offered),
                ] {
                    let name = outside[self.pick(outside.len())];
                    if self.chance(5) && may_read(&drafts[reader], name) {
                        let class = self.class(&drafts[reader], name, true);
                        reads[reader].push(Read {
                            name,
                            class,
                            mark,
                            lands,
                        });
                    }
                }
            }
        }
        // The names this scope was handed.
        for (name, owed, mark) in obligations {
            let hosts: Vec<usize> = (0..count)
                .filter(|reader| may_read(&drafts[*reader], name))
                .collect();
            let reader = hosts[self.pick(hosts.len())];
            let class = self.class(&drafts[reader], name, true);
            let lands = match owed {
                Owed::Builtin => Lands::Builtin,
                Owed::Declared(declared) => Lands::Binder {
                    up: depth - declared,
                },
                Owed::Nowhere => Lands::Nowhere,
                Owed::Hole => Lands::Hole,
                Owed::Offered => Lands::Offered,
            };
            reads[reader].push(Read {
                name,
                class,
                mark,
                lands,
            });
        }
        // The injected unbound read: eager at or before its binder's statement, or of a name
        // nothing declares.
        let unbound = match injection {
            Some(Injection::ReadAhead) => {
                let mut pairs = Vec::new();
                for (reader, draft) in drafts.iter().enumerate() {
                    for name in binders[reader..].iter().flatten() {
                        if reads_eagerly(draft, name.is_type()) {
                            pairs.push((reader, *name, Class::Eager));
                        }
                    }
                }
                Some(pairs[self.pick(pairs.len())])
            }
            Some(Injection::Undeclared) => {
                let readers: Vec<usize> = (0..count)
                    .filter(|reader| !matches!(drafts[*reader], Draft::Expr(_)))
                    .collect();
                let reader = readers[self.pick(readers.len())];
                let name = if drafts[reader] == Draft::Union || self.chance(2) {
                    Name::Fixed("Tzz")
                } else {
                    Name::Fixed("zz")
                };
                let eager = reads_eagerly(&drafts[reader], name.is_type()) && self.chance(2);
                let class = if eager { Class::Eager } else { Class::Deferred };
                Some((reader, name, class))
            }
            _ => None,
        };
        if let Some((reader, name, class)) = unbound {
            let at = self.pick(reads[reader].len() + 1);
            let read = Read {
                name,
                class,
                mark: Marking::Unmarked,
                lands: Lands::Nowhere,
            };
            reads[reader].insert(at, read);
            self.refusal = Some(Refusal::Unbound { name });
        }

        // What the scopes nested here may read wherever they like.
        let enclosing: Vec<(Name, usize)> = visible
            .iter()
            .copied()
            .chain(parameters.iter().map(|name| (*name, depth)))
            .collect();
        // Inside a quote's code, a `\` name lands at the binder of the code an unmarked one would.
        if let Some(code) = self.code {
            for read in reads.iter_mut().flatten() {
                let inside = matches!(read.lands, Lands::Binder { up } if depth - up >= code);
                if inside && read.mark == Marking::Unmarked && self.chance(4) {
                    read.mark = Marking::Built;
                }
            }
        }
        let registrations: Vec<(Key, usize)> = (drafts.iter().enumerate())
            .filter_map(|(index, draft)| match draft {
                Draft::Expr(key) => Some((*key, index)),
                _ => None,
            })
            .collect();
        if kind == Kind::Program {
            self.registered = registrations.clone();
        }
        // Only the quote's code holds no injection, so a refused read is never routed into it.
        let quotable = self.quotes && self.code.is_none() && injection.is_none();
        let mut statements = Vec::with_capacity(count);
        for (index, draft) in drafts.into_iter().enumerate() {
            if kind == Kind::Program {
                self.statement = index;
            }
            let own = std::mem::take(&mut reads[index]);
            let form = match draft {
                Draft::Union => BuiltinShape::Union(own),
                Draft::Function(parameters) => {
                    let (signature, routed) =
                        own.into_iter().partition(|read| read.class == Class::Eager);
                    BuiltinShape::Function(
                        self.callable(depth, parameters, signature, routed, &enclosing),
                    )
                }
                Draft::Expr(key) => {
                    assert!(own.is_empty(), "a registration reads nothing");
                    BuiltinShape::Expr {
                        key,
                        body: self.registration(),
                    }
                }
                Draft::Let | Draft::Bare => {
                    let mut carriers = self.carriers(depth, own, &enclosing, quotable);
                    if kind == Kind::Code {
                        for _ in 0..self.pick(3) {
                            carriers.push(self.keyworded(index, &registrations));
                        }
                    }
                    match draft {
                        Draft::Let => BuiltinShape::Let(carriers),
                        _ => BuiltinShape::Bare(carriers),
                    }
                }
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
        let eager = declared_before && reads_eagerly(draft, name.is_type());
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
    /// whose boundary gives it the same class here — a lambda, an arm, or where `quotable` a quote's
    /// code, read there through `$`.
    fn carriers(
        &mut self,
        depth: usize,
        own: Vec<Read>,
        enclosing: &[(Name, usize)],
        quotable: bool,
    ) -> Vec<Carrier> {
        let mut direct = Vec::new();
        let mut deferred = Vec::new();
        let mut eager = Vec::new();
        for read in own {
            if self.nestable(depth) && self.chance(3) {
                match read.class {
                    Class::Deferred => deferred.push(read),
                    Class::Eager => eager.push(read),
                }
            } else {
                direct.push(read);
            }
        }
        let mut nested = Vec::new();
        let quoted = |generator: &mut Self, routed: &[Read]| {
            let refused = routed.iter().any(|read| read.lands == Lands::Nowhere);
            quotable && !refused && generator.chance(3)
        };
        if !deferred.is_empty() || (self.nestable(depth) && self.chance(6)) {
            self.scopes += 1;
            if quoted(self, &deferred) {
                let code = self.code_scope(depth, Class::Deferred, &deferred, enclosing);
                nested.push(Carrier::Quote(Class::Deferred, code));
            } else {
                let signature = self.take_types(&mut direct);
                let callable = self.callable(depth, Vec::new(), signature, deferred, enclosing);
                nested.push(Carrier::Lambda(Class::Deferred, callable));
            }
        }
        // A nested arm declares its own `it`, so a read of an enclosing `it` routes through a lambda.
        let (called, matched) = if self.chance(2) {
            (eager, Vec::new())
        } else {
            eager.into_iter().partition(|read| read.name == Name::It)
        };
        if !called.is_empty() || (self.nestable(depth) && self.chance(6)) {
            self.scopes += 1;
            if quoted(self, &called) {
                let code = self.code_scope(depth, Class::Eager, &called, enclosing);
                nested.push(Carrier::Quote(Class::Eager, code));
            } else {
                let signature = self.take_types(&mut direct);
                let callable = self.callable(depth, Vec::new(), signature, called, enclosing);
                nested.push(Carrier::Lambda(Class::Eager, callable));
            }
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

    /// A quote's code holding `routed`, the reads a quote of class `class` written at `depth` makes
    /// there: each through `$` where it lands at a binder, unmarked where at a builtin.
    fn code_scope(
        &mut self,
        depth: usize,
        class: Class,
        routed: &[Read],
        enclosing: &[(Name, usize)],
    ) -> Scope {
        let written = match depth {
            0 => (self.registered.iter().copied())
                .filter(|(_, statement)| class == Class::Deferred || *statement < self.statement)
                .collect(),
            _ => Vec::new(),
        };
        let obligations = obligations(depth, routed)
            .into_iter()
            .map(|(name, owed, _)| match owed {
                Owed::Declared(_) => (name, owed, Marking::Written),
                _ => (name, owed, Marking::Unmarked),
            })
            .collect();
        let outer = (
            self.code.replace(depth + 1),
            std::mem::replace(&mut self.written, written),
        );
        let code = self.scope(Kind::Code, depth + 1, Vec::new(), enclosing, obligations);
        (self.code, self.written) = outer;
        code
    }

    /// The callable a registration registers: one parameter, read by its one statement.
    fn registration(&mut self) -> Callable {
        let parameter = self.fresh(false);
        let number = Read::builtin(NUMBER, Class::Eager);
        let body = Scope {
            kind: Kind::Callable,
            parameters: vec![parameter],
            statements: vec![Statement {
                binder: None,
                form: BuiltinShape::Bare(vec![Carrier::Read(Read::local(parameter, Class::Eager))]),
            }],
            components: vec![BTreeSet::from([parameter])],
        };
        Callable {
            types: vec![number],
            returns: number,
            body,
        }
    }

    /// A keyworded use in statement `statement` of a quote's code, whose own registrations are
    /// `registrations`, and the candidates it must list.
    fn keyworded(&mut self, statement: usize, registrations: &[(Key, usize)]) -> Carrier {
        let mark = match self.pick(4) {
            3 if !self.written.is_empty() => Marking::Written,
            choice => [Marking::Unmarked, Marking::Written, Marking::Built][choice % 3],
        };
        // Mostly a key a registration the use sees holds.
        let seen: Vec<Key> = match mark {
            Marking::Written => self.written.iter().map(|(key, _)| *key).collect(),
            _ => (registrations.iter())
                .filter(|(_, at)| *at < statement)
                .map(|(key, _)| *key)
                .collect(),
        };
        let key = match self.pick(5) {
            _ if !seen.is_empty() && self.pick(3) > 0 => Use::Key(seen[self.pick(seen.len())]),
            4 => Use::Builtin,
            index => Use::Key(Key(index as u8)),
        };
        // A `$(…)` of a planned key needs a registration of the program to see the quote.
        let key = match key {
            Use::Key(key) if mark == Marking::Written && !seen.contains(&key) => Use::Builtin,
            key => key,
        };
        let own = |key: Key| {
            (registrations.iter())
                .filter(move |(registered, at)| *registered == key && *at < statement)
                .map(|(_, at)| Listed::Registration {
                    up: 0,
                    statement: *at as u32,
                })
        };
        let mut expected: Vec<Listed> = match (mark, key) {
            (Marking::Unmarked, Use::Builtin) => vec![Listed::Builtin, Listed::Hole],
            (Marking::Unmarked, Use::Key(key)) => own(key).chain([Listed::Hole]).collect(),
            (Marking::Written, Use::Builtin) => vec![Listed::Builtin],
            (Marking::Written, Use::Key(key)) => (self.written.iter())
                .filter(|(registered, _)| *registered == key)
                .map(|(_, at)| Listed::Registration {
                    up: 1,
                    statement: *at as u32,
                })
                .collect(),
            (Marking::Built, Use::Builtin) => vec![Listed::Offered],
            (Marking::Built, Use::Key(key)) => own(key).chain([Listed::Offered]).collect(),
        };
        expected.sort();
        Carrier::Use {
            key,
            mark,
            expected,
        }
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
            let found = direct
                .iter()
                .position(|read| read.class == Class::Eager && read.name.is_type() == is_type);
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
    match draft {
        Draft::Union => name.is_type(),
        Draft::Expr(_) => false,
        _ => true,
    }
}

/// Whether a statement drafted as `draft` can read a name of the channel `is_type` eagerly.
fn reads_eagerly(draft: &Draft, is_type: bool) -> bool {
    match draft {
        Draft::Union | Draft::Expr(_) => false,
        Draft::Function(_) => is_type,
        Draft::Let | Draft::Bare => true,
    }
}

/// The channel of the binder a statement drafted as `draft` declares, if it declares one.
fn declares(draft: &Draft) -> Option<bool> {
    match draft {
        Draft::Let | Draft::Function(_) => Some(false),
        Draft::Union => Some(true),
        Draft::Bare | Draft::Expr(_) => None,
    }
}

/// Every parameter at `0` and every statement's binder at the statement's position.
fn declared(binders: &[Option<Name>], parameters: &[Name]) -> Vec<(Name, u32)> {
    let statements = binders
        .iter()
        .enumerate()
        .filter_map(|(index, binder)| Some(((*binder)?, index as u32 + 1)));
    parameters
        .iter()
        .map(|name| (*name, 0))
        .chain(statements)
        .collect()
}

/// The components an eager cycle can be closed in: a value component with a `LET` after its first
/// member.
fn cyclable(groups: &[Vec<usize>], drafts: &[Draft]) -> Vec<usize> {
    (0..groups.len())
        .filter(|group| {
            let members = &groups[*group];
            declares(&drafts[members[0]]) == Some(false)
                && members[1..]
                    .iter()
                    .any(|member| drafts[*member] == Draft::Let)
        })
        .collect()
}

/// Redraft a scope `injection` is claimed for until it can hold the refusal: forcing the last
/// statement a `LET`, and the first one too where nothing else can pair with it.
fn make_room(injection: Injection, drafts: &mut [Draft], parameters: &[Name]) {
    let last = drafts.len() - 1;
    let values_before = |drafts: &[Draft], index: usize| {
        drafts[..index]
            .iter()
            .any(|draft| declares(draft) == Some(false))
    };
    match injection {
        Injection::EagerCycle => {
            let room =
                (0..=last).any(|index| drafts[index] == Draft::Let && values_before(drafts, index));
            if !room {
                drafts[last] = Draft::Let;
                if !values_before(drafts, last) {
                    drafts[0] = Draft::Let;
                }
            }
        }
        Injection::Rebind => {
            let room = (0..=last).any(|later| {
                declares(&drafts[later]).is_some_and(|is_type| {
                    (!is_type && !parameters.is_empty())
                        || drafts[..later]
                            .iter()
                            .any(|draft| declares(draft) == Some(is_type))
                })
            });
            if !room {
                drafts[last] = Draft::Let;
                if parameters.is_empty() && !values_before(drafts, last) {
                    drafts[0] = Draft::Let;
                }
            }
        }
        Injection::ShadowsBuiltin => {
            let named = parameters.iter().any(|name| *name != Name::It);
            if !named && drafts.iter().all(|draft| declares(draft).is_none()) {
                drafts[last] = Draft::Let;
            }
        }
        Injection::ReadAhead => {
            let room = (0..=last).any(|bound| {
                declares(&drafts[bound]).is_some_and(|is_type| {
                    drafts[..=bound]
                        .iter()
                        .any(|reader| reads_eagerly(reader, is_type))
                })
            });
            if !room {
                drafts[last] = Draft::Let;
            }
        }
        Injection::Undeclared => {
            if drafts.iter().all(|draft| matches!(draft, Draft::Expr(_))) {
                drafts[last] = Draft::Let;
            }
        }
    }
}

/// Where a name a nested scope is handed resolves.
#[derive(Clone, Copy, Debug)]
enum Owed {
    Builtin,
    /// A binder of the scope at this depth.
    Declared(usize),
    /// Nowhere: the refused read, routed in.
    Nowhere,
    Hole,
    Offered,
}

/// What reads routed out of a scope at `depth` hand the nested scope: each name, where it
/// resolves, and how it is marked.
fn obligations(depth: usize, routed: &[Read]) -> Vec<(Name, Owed, Marking)> {
    routed
        .iter()
        .map(|read| {
            let owed = match read.lands {
                Lands::Binder { up } => Owed::Declared(depth - up),
                Lands::Builtin => Owed::Builtin,
                Lands::Nowhere => Owed::Nowhere,
                Lands::Hole => Owed::Hole,
                Lands::Offered => Owed::Offered,
            };
            (read.name, owed, read.mark)
        })
        .collect()
}

// ---------- refusals ----------

/// The kind of refusal a refused plan injects into one of its scopes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Injection {
    /// A `LET` reads a fellow member of its component eagerly, directly or through a called lambda
    /// or an arm.
    EagerCycle,
    /// A read eager where it sits, directly or through a called lambda or an arm, of a binder at or
    /// after its statement.
    ReadAhead,
    /// A read of a name nothing declares.
    Undeclared,
    /// A later binder named after an earlier binder or parameter of its channel.
    Rebind,
    /// A binder or parameter named after a builtin.
    ShadowsBuiltin,
}

const INJECTIONS: [Injection; 5] = [
    Injection::EagerCycle,
    Injection::ReadAhead,
    Injection::Undeclared,
    Injection::Rebind,
    Injection::ShadowsBuiltin,
];

/// The refusal a refused plan must be refused with. Where each is found is read off the rendered
/// source's declared offsets.
#[derive(Clone, Debug)]
pub(super) enum Refusal {
    Rebind {
        name: Name,
    },
    ShadowsBuiltin {
        name: Name,
    },
    /// The read landing [`Lands::Nowhere`].
    Unbound {
        name: Name,
    },
    EagerCycle(BTreeSet<Name>),
}

/// A program generated from `stream` with refusal `which` (modulo the five kinds) injected into
/// the scope entered `scope`-th (modulo the scopes the program enters), in source order.
pub(super) fn refused(stream: &[u32], which: usize, scope: usize) -> (Scope, Refusal) {
    let mut valid = Generator::new(stream);
    valid.program();
    // The scopes entered before the target are generated alike with or without the injection, so
    // the target is entered.
    let mut generator = Generator::new(stream);
    generator.injection = Some((INJECTIONS[which % 5], scope % valid.entered));
    let program = generator.program();
    let refusal = generator
        .refusal
        .expect("the target scope injects its refusal");
    (program, refusal)
}

impl Generator<'_> {
    /// Enter a scope: the injection, if this is the scope it targets. A quote's code holds none.
    fn claim(&mut self) -> Option<Injection> {
        // A quote's code is never a target, so it is not counted.
        if self.code.is_some() {
            return None;
        }
        self.entered += 1;
        let (injection, target) = self.injection?;
        (target == self.entered - 1).then_some(injection)
    }

    /// Name a later binder of a scope after an earlier binder or parameter of its channel.
    fn inject_rebind(&mut self, binders: &mut [Option<Name>], parameters: &[Name]) {
        let declared = declared(binders, parameters);
        let mut pairs = Vec::new();
        for (place, (first, _)) in declared.iter().enumerate() {
            for (later, (second, at)) in declared.iter().enumerate().skip(place + 1) {
                if *at > 0 && first.is_type() == second.is_type() {
                    pairs.push((place, later));
                }
            }
        }
        let (place, later) = pairs[self.pick(pairs.len())];
        let name = declared[place].0;
        let second = declared[later].1;
        binders[second as usize - 1] = Some(name);
        self.refusal = Some(Refusal::Rebind { name });
    }

    /// Name a binder or parameter of a scope after a builtin of its channel.
    fn inject_shadow_builtin(&mut self, binders: &mut [Option<Name>], parameters: &mut [Name]) {
        let declared: Vec<(Name, u32)> = declared(binders, parameters)
            .into_iter()
            .filter(|(name, _)| *name != Name::It)
            .collect();
        let (renamed, at) = declared[self.pick(declared.len())];
        let name = if renamed.is_type() {
            BUILTIN_TYPES[self.pick(BUILTIN_TYPES.len())]
        } else {
            ORIGIN
        };
        match at {
            0 => {
                for parameter in parameters.iter_mut() {
                    if *parameter == renamed {
                        *parameter = name;
                    }
                }
            }
            _ => binders[at as usize - 1] = Some(name),
        }
        self.refusal = Some(Refusal::ShadowsBuiltin { name });
    }
}

// ---------- shadowing ----------

impl Generator<'_> {
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
                form: BuiltinShape::Let(Vec::new()),
            });
            program.components.push(BTreeSet::from([outer]));
            let body = Scope {
                kind: Kind::Callable,
                parameters: vec![inner],
                statements: vec![Statement {
                    binder: None,
                    form: BuiltinShape::Bare(Vec::new()),
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
                form: BuiltinShape::Bare(vec![Carrier::Lambda(Class::Deferred, callable)]),
            });
            candidates = Self::shadow_candidates(program);
        }
        let (path, declaring, renamed, name) = candidates.swap_remove(self.pick(candidates.len()));
        let nested = program.at_path(&path);
        nested.rename_reads(renamed, name);
        nested.rename_binder(renamed, name);
        nested.statements.push(Statement {
            binder: None,
            form: BuiltinShape::Bare(vec![Carrier::Read(Read::local(name, Class::Deferred))]),
        });
        program
            .at_path(&path[..declaring])
            .statements
            .push(Statement {
                binder: None,
                form: BuiltinShape::Bare(vec![Carrier::Read(Read::local(name, Class::Deferred))]),
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
            let mut in_code = false;
            for depth in 0..path.len() {
                let scope = program.at_path(&path[..depth]);
                in_code |= scope.kind == Kind::Code;
                for (name, _) in scope.binders() {
                    if name != Name::It {
                        enclosing.push((depth, name));
                    }
                }
            }
            let nested = program.at_path(path);
            in_code |= nested.kind == Kind::Code;
            for (renamed, _) in nested.binders() {
                if renamed == Name::It {
                    continue;
                }
                for (depth, name) in &enclosing {
                    if name.is_type() == renamed.is_type() && !nested.reads_name(*name, in_code) {
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
    /// A keyworded use: the one at this index of [`Rendering::uses`], or a `ZZ` call.
    Use(Option<usize>),
}

/// A planned keyworded use, in the scope it is written in.
pub(super) struct PlacedUse<'p> {
    pub expected: &'p [Listed],
    pub scope: usize,
}

pub(super) struct Placed<'p> {
    pub read: &'p Read,
    pub scope: usize,
    pub statement: u32,
    /// Where in the source the read's name is written.
    pub offset: u32,
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
    pub uses: Vec<PlacedUse<'p>>,
    pub scopes: Vec<Rendered<'p>>,
    /// Every binder and parameter name, beside where in the source it is written.
    pub declared: Vec<(Name, u32)>,
}

struct Renderer<'p> {
    out: Rendering<'p>,
    scope: usize,
    statement: u32,
}

/// The source of the program `choices` plans, writing no quote value — what the knot suite runs a
/// plan as, which does not read how a quote ties.
pub(crate) fn program_source(choices: &[u32]) -> String {
    let mut generator = Generator::new(choices);
    generator.quotes = false;
    render_program(&generator.program()).source
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

impl<'p> Renderer<'p> {
    fn new(root: &'p Scope, level: usize) -> Self {
        Renderer {
            out: Rendering {
                source: String::new(),
                tokens: Vec::new(),
                reads: Vec::new(),
                uses: Vec::new(),
                scopes: vec![Rendered {
                    scope: root,
                    parent: None,
                    statement: 0,
                    level,
                }],
                declared: Vec::new(),
            },
            scope: 0,
            statement: 0,
        }
    }

    fn text(&mut self, text: &str) {
        self.out.source.push_str(text);
    }

    fn other(&mut self, name: Name) {
        self.out.declared.push((name, self.out.source.len() as u32));
        self.text(&name.text());
        self.out.tokens.push(Token::Other);
    }

    /// A read, its sigil written before it; a marked type name in a group, as `:$Taa` does not parse.
    fn read(&mut self, read: &'p Read) {
        let sigil = match read.mark {
            Marking::Unmarked => "",
            Marking::Written => "$",
            Marking::Built => "\\",
        };
        let grouped = !sigil.is_empty() && read.name.is_type();
        if grouped {
            self.text("(");
        }
        self.out.tokens.push(Token::Mention(self.out.reads.len()));
        self.out.reads.push(Placed {
            read,
            scope: self.scope,
            statement: self.statement,
            offset: self.out.source.len() as u32,
        });
        self.text(sigil);
        self.text(&read.name.text());
        if grouped {
            self.text(")");
        }
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
            BuiltinShape::Let(carriers) => {
                self.text("LET ");
                self.other(statement.binder.expect("a LET binds"));
                self.text(" = ");
                self.carriers(carriers);
            }
            BuiltinShape::Function(callable) => {
                self.text("LET ");
                self.other(statement.binder.expect("a function binds"));
                self.text(" = (FN :{");
                let parameters = callable.body.parameters.iter().zip(&callable.types);
                for (index, (parameter, ty)) in parameters.enumerate() {
                    if index > 0 {
                        self.text(", ");
                    }
                    self.other(*parameter);
                    self.text(" :");
                    self.read(ty);
                }
                self.text("} -> ");
                self.read(&callable.returns);
                self.text(" = #");
                self.body(&callable.body);
                self.text(")");
            }
            BuiltinShape::Union(reads) => {
                self.text("UNION ");
                self.other(statement.binder.expect("a UNION binds"));
                self.text(" = #{");
                for (tag, read) in reads.iter().enumerate() {
                    let tag = char::from(b'a' + tag as u8);
                    self.text(&format!("{}K{tag}: ", if tag == 'a' { "" } else { ", " }));
                    self.out.tokens.push(Token::Other);
                    self.read(read);
                }
                self.text("}");
            }
            BuiltinShape::Bare(carriers) => {
                self.text("(");
                self.carriers(carriers);
                self.text(")");
            }
            BuiltinShape::Expr { key, body } => {
                self.text(&format!("EXPR #({} ", key.text()));
                self.other(body.body.parameters[0]);
                self.text(" :");
                self.read(&body.types[0]);
                self.text(") -> ");
                self.read(&body.returns);
                self.text(" = #");
                self.body(&body.body);
            }
        }
    }

    /// A right-hand side: the deferred carriers in a list, the eager ones as a call's arguments
    /// inside it — or a lone carrier where it stands.
    fn carriers(&mut self, carriers: &'p [Carrier]) {
        let (deferred, eager): (Vec<&Carrier>, Vec<&Carrier>) =
            carriers.iter().partition(|carrier| match carrier {
                Carrier::Read(read) => read.class == Class::Deferred,
                Carrier::Lambda(class, _) | Carrier::Quote(class, _) => *class == Class::Deferred,
                Carrier::Arm { .. } | Carrier::Use { .. } => false,
            });
        match (deferred.len(), eager.len()) {
            (0, 0) => self.text("1"),
            (0, 1) => match eager[0] {
                Carrier::Read(read) if !read.name.is_type() => self.read(read),
                Carrier::Read(read) if read.name.is_type() => self.call(&eager),
                Carrier::Lambda(_, callable) => {
                    self.text("(");
                    self.lambda(callable);
                    self.text(" 1)");
                }
                // Alone, a quote would be a deferred one.
                Carrier::Quote(..) => self.call(&eager),
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
                        Carrier::Quote(_, code) => {
                            self.text("#");
                            self.body(code);
                        }
                        Carrier::Arm { .. } | Carrier::Use { .. } => {
                            unreachable!("an arm and a use are eager")
                        }
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

    /// A call of the builtin `ZZ` over each argument in turn, nested — `(ZZ a (ZZ b (ZZ c)))` — so
    /// every call takes one argument or two, the arities the suites' table holds `ZZ` at.
    fn call(&mut self, arguments: &[&'p Carrier]) {
        for (index, &argument) in arguments.iter().enumerate() {
            if index > 0 {
                self.text(" ");
            }
            self.out.tokens.push(Token::Use(None));
            self.text("(ZZ ");
            self.eager(argument);
        }
        for _ in arguments {
            self.text(")");
        }
    }

    fn eager(&mut self, carrier: &'p Carrier) {
        match carrier {
            Carrier::Read(read) => self.read(read),
            Carrier::Lambda(_, callable) => self.lambda(callable),
            Carrier::Quote(_, code) => {
                self.text("#");
                self.body(code);
            }
            Carrier::Use {
                key,
                mark,
                expected,
            } => {
                self.out.tokens.push(Token::Use(Some(self.out.uses.len())));
                self.out.uses.push(PlacedUse {
                    expected,
                    scope: self.scope,
                });
                let key = match key {
                    Use::Key(key) => key.text(),
                    Use::Builtin => "ZZ".to_string(),
                };
                let sigil = match mark {
                    Marking::Unmarked => "",
                    Marking::Written => "$",
                    Marking::Built => "\\",
                };
                self.text(&format!("{sigil}({key} 1)"));
            }
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
                self.text(" WITH #{");
                for (index, (head, body)) in arms.iter().enumerate() {
                    if index > 0 {
                        self.text(", ");
                    }
                    self.read(head);
                    self.text(": ");
                    self.body(body);
                }
                self.text("})");
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
        self.text(" = #");
        self.body(&callable.body);
        self.text(")");
    }
}
