//! A **code view**: code spliced or sliced at run time, held as a balanced tree over the flat
//! syntax it came from, which copies nothing, renumbers nothing and digests nothing until it is
//! materialized. See [README.md § The code node](README.md#the-code-node) and
//! [scope README § Splicing](../scope/README.md#splicing).
//!
//! A view's root is a [`NodeView`]: a node's parts as an AVL tree of [`CodeItem`]s
//! ([values/view.rs](../values/view.rs)), each a stretch of a flat node's parts beside the
//! environment its `$` and filled marks resolve through — a code value's bindings, or a composing
//! quote's own — or one group or builtin-read quote part whose node is itself a view, or one
//! literal part of a flat node whose composing elements' nodes are views ([`LiteralView`]). Every tree
//! node memoizes its parts' [`CodeSummary`], so a view's length, depth, kind and weight read in
//! O(1) and every read or edit is O(log n) in its root's parts.
//!
//! The **materializer** ([`materialized`]) lays a view flat through a writer, rewriting each mark's
//! scope as the scope README's rules say: a mark its environment's root owned takes the canonical
//! number of its `(environment, scope)`, numbered by first appearance, pinned where it now lies in
//! a quote value nested in the laid code; a mark a nested quote value owned and still does keeps
//! its scope; one a quote value owned that now lies at the root takes a number no binding holds.
//! Beside the syntax it gathers each environment's bindings at each scope it meets, keyed by the
//! canonical number.

use crate::memory::{Bump, BumpVec, Writer, collect, program_storage, resident};
use crate::parse::SourceRef;
use crate::parse::{
    BUILTIN_SHAPES, ExpressionPart, KEYWORDS, KExpression, KeyElement, PartsSummary, Quoted,
    Remark, Scope, Spanned, WrittenSlots, builtin_shape_for, literal_laid, remarked,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::view::{self, Leaf, Summary};
use crate::values::{Link, Weight};

use super::code::Binding;
use super::{KValue, Knotted};

/// Where a stretch's `$` and filled marks find their bindings.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Env<'graph, 'cell> {
    /// A code value: its bound run resolves its syntax's marks, each edge through it.
    Code(Knotted<'graph, 'cell>),
    /// A composing quote's own `$` bindings, read where it was born, all at scope 0.
    Own(&'cell [Binding<'graph, 'cell>]),
}

impl<'graph, 'cell> Env<'graph, 'cell> {
    /// The bindings it holds, sorted by scope, then name.
    fn bindings(self) -> &'cell [Binding<'graph, 'cell>] {
        match self {
            Env::Code(member) => member.code().expect("a code environment is code").bound(),
            Env::Own(run) => run,
        }
    }

    /// Its binding's value, each edge resolved through the code value that holds it.
    fn value(self, binding: &Binding<'graph, 'cell>) -> KValue<'graph, 'cell> {
        match self {
            Env::Code(member) => binding.link.resolve(member),
            Env::Own(_) => match binding.link {
                Link::Value(value) => value,
                Link::Edge(_) => unreachable!("a quote's own bindings are value words"),
            },
        }
    }

    /// What copying every binding it holds writes, at most: a code value's whole knot, or an own
    /// run and what its values point at.
    fn weight(self) -> u64 {
        match self {
            Env::Code(member) => crate::values::Knotted::weight(&member).bytes() as u64,
            Env::Own(run) => run
                .iter()
                .fold(
                    Weight::run::<Binding<'_, '_>>(run.len()),
                    |weight, binding| weight.plus(binding.link.referent_weight()),
                )
                .bytes() as u64,
        }
    }

    /// Its identity: the code value's node, or the own run's address.
    fn identity(self) -> (bool, usize) {
        match self {
            Env::Code(member) => (false, std::ptr::from_ref(member.node()).addr()),
            Env::Own(run) => (true, run.as_ptr().addr()),
        }
    }
}

/// How a nested item's node stands in its parent: a group, or a quote part a builtin reads as
/// written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Wrap {
    Group,
    Quote,
}

/// A leaf of a code view's tree.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CodeItem<'graph, 'cell> {
    /// Parts `from..to` of a flat node, read backward where `reversed`, their marks resolving
    /// through `env`, beside their summary read forward.
    Parts {
        node: &'cell KExpression<'cell>,
        from: u32,
        to: u32,
        reversed: bool,
        env: Env<'graph, 'cell>,
        summary: PartsSummary,
    },
    /// One group part, or one quote part a builtin reads as written, whose node is a view.
    Nested {
        wrap: Wrap,
        node: &'cell NodeView<'graph, 'cell>,
    },
    /// One literal part of a flat node, some of whose elements composed.
    Literal(&'cell LiteralView<'graph, 'cell>),
}

impl<'graph, 'cell> CodeItem<'graph, 'cell> {
    /// Parts `[from, to)` of `node`, forward, through `env`; `from < to`.
    pub(crate) fn parts(
        node: &'cell KExpression<'cell>,
        (from, to): (usize, usize),
        env: Env<'graph, 'cell>,
    ) -> Self {
        debug_assert!(from < to, "a stretch is never empty");
        CodeItem::Parts {
            node,
            from: from as u32,
            to: to as u32,
            reversed: false,
            env,
            summary: node.stretch(from, to),
        }
    }
}

/// One list, dict or record literal or template part of a flat node, its marks resolving through
/// `env`, beside the view each of its elements that composed is laid from, keyed by the element's
/// address and standing as a group or a quote. A nested literal's elements are among them, all in
/// the order a walk of [`ExpressionPart::child`] meets them, so each walk reads them in turn.
#[derive(Debug)]
pub(crate) struct LiteralView<'graph, 'cell> {
    part: &'cell Spanned<ExpressionPart<'cell>>,
    /// Whether the literal's slot in the node it came from is read as written.
    before: bool,
    env: Env<'graph, 'cell>,
    children: &'cell [(usize, Wrap, &'cell NodeView<'graph, 'cell>)],
    summary: CodeSummary,
}

impl<'graph, 'cell> LiteralView<'graph, 'cell> {
    /// The literal `part`, its slot read as written where `before`, through `env`, beside its
    /// composed `children`; at least one.
    fn of(
        writer: Writer<'cell, '_>,
        part: &'cell Spanned<ExpressionPart<'cell>>,
        before: bool,
        env: Env<'graph, 'cell>,
        children: &'cell [(usize, Wrap, &'cell NodeView<'graph, 'cell>)],
    ) -> &'cell Self {
        let mut literal = LiteralView {
            part,
            before,
            env,
            children,
            summary: CodeSummary::default(),
        };
        // Its written summary, each composed element's flat copy added to the weight: an element's
        // written weight stays in, which over-prices, as a weight may.
        let mut parts = PartsSummary::of(&part.value);
        let laid = children
            .iter()
            .map(|(_, _, view)| view.exact())
            .sum::<u64>();
        parts.weight = u32::try_from(u64::from(parts.weight) + laid).unwrap_or(u32::MAX);
        parts.deepest = literal.depth(&part.value, &mut 0);
        literal.summary = CodeSummary {
            parts,
            flat: 0,
            slack: children.iter().map(|(_, _, view)| view.slack()).sum(),
            envs: env.weight()
                + children
                    .iter()
                    .map(|(_, _, view)| view.summary().envs)
                    .sum::<u64>(),
        };
        resident(writer, literal)
    }

    /// The composed view `element` is laid from, standing as a group or a quote, if it composed:
    /// child `next`, which a walk meeting the elements in order then steps past.
    fn child(
        &self,
        element: &ExpressionPart<'_>,
        next: &mut usize,
    ) -> Option<(Wrap, &'cell NodeView<'graph, 'cell>)> {
        let &(key, wrap, view) = self.children.get(*next)?;
        (key == std::ptr::from_ref(element).addr()).then(|| {
            *next += 1;
            (wrap, view)
        })
    }

    /// How deep `literal`, this one or one nested in it, lays: one more than its deepest element,
    /// a composed element as deep as its view; `next` as [`child`](Self::child) reads it.
    fn depth(&self, literal: &ExpressionPart<'_>, next: &mut usize) -> u32 {
        let mut deepest = 0;
        let mut at = 0;
        while let Some(element) = literal.child(at) {
            let depth = match self.child(element, next) {
                Some((_, view)) => view.depth(),
                None if element.is_literal() => self.depth(element, next),
                None => PartsSummary::of(element).deepest,
            };
            deepest = deepest.max(depth);
            at += 1;
        }
        1 + deepest
    }
}

/// What a code view's subtree memoizes: its parts' summary, the weight of those of them its
/// stretches hold, the bytes of the binder plans its new nodes may lay beyond what that summary
/// prices, and what copying its environments' bindings writes, at most, each environment once per
/// leaf.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct CodeSummary {
    pub parts: PartsSummary,
    pub flat: u64,
    pub slack: u64,
    pub envs: u64,
}

impl Summary<()> for CodeSummary {
    fn len(&self) -> usize {
        self.parts.len as usize
    }

    fn joined(self, right: Self) -> Self {
        CodeSummary {
            parts: self.parts.joined(right.parts),
            flat: self.flat + right.flat,
            slack: self.slack + right.slack,
            envs: self.envs + right.envs,
        }
    }

    fn reversed(self) -> Self {
        CodeSummary {
            parts: self.parts.reversed(),
            ..self
        }
    }

    fn waits(&self) -> bool {
        false
    }

    fn under(self, (): ()) -> Self {
        self
    }
}

impl Leaf for CodeItem<'_, '_> {
    type Summary = CodeSummary;
    type Lazy = ();

    fn summary(&self) -> CodeSummary {
        match *self {
            CodeItem::Parts {
                reversed,
                env,
                summary,
                ..
            } => CodeSummary {
                parts: if reversed {
                    summary.reversed()
                } else {
                    summary
                },
                flat: u64::from(summary.weight),
                slack: 0,
                envs: env.weight(),
            },
            CodeItem::Nested { wrap, node } => node.as_part(wrap),
            CodeItem::Literal(literal) => literal.summary,
        }
    }

    fn split(self, at: usize) -> (Option<Self>, Option<Self>) {
        let CodeItem::Parts {
            node,
            from,
            to,
            reversed,
            env,
            ..
        } = self
        else {
            unreachable!("a one-part item is never split inside")
        };
        let (from, to, at) = (from as usize, to as usize, at);
        let part = |range: (usize, usize)| {
            let forward = CodeItem::parts(node, range, env);
            if reversed {
                forward.reversed()
            } else {
                forward
            }
        };
        match reversed {
            false => (Some(part((from, from + at))), Some(part((from + at, to)))),
            true => (Some(part((to - at, to))), Some(part((from, to - at)))),
        }
    }

    fn reversed(self) -> Self {
        match self {
            CodeItem::Parts {
                node,
                from,
                to,
                reversed,
                env,
                summary,
            } => CodeItem::Parts {
                node,
                from,
                to,
                reversed: !reversed,
                env,
                summary,
            },
            one @ (CodeItem::Nested { .. } | CodeItem::Literal(_)) => one,
        }
    }

    fn under(self, (): ()) -> Self {
        self
    }

    #[cfg(test)]
    fn holds_its_laws(&self) -> bool {
        match *self {
            CodeItem::Parts {
                node,
                from,
                to,
                summary,
                ..
            } => from < to && summary == node.stretch(from as usize, to as usize),
            CodeItem::Nested { node, .. } => node.holds_its_laws(),
            CodeItem::Literal(literal) => {
                (literal.children.iter()).all(|(_, _, node)| node.holds_its_laws())
            }
        }
    }
}

/// A code view's tree, named without projections so a view stays covariant in its lifetimes.
pub(crate) type CodeNode<'graph, 'cell> =
    view::Node<'cell, CodeItem<'graph, 'cell>, CodeSummary, ()>;

/// A node of code as a view: its parts as an AVL tree of items, `None` for a node of no parts,
/// beside the source its syntax reports.
#[derive(Debug)]
pub(crate) struct NodeView<'graph, 'cell> {
    tree: Option<&'cell CodeNode<'graph, 'cell>>,
    source: SourceRef,
}

impl<'graph, 'cell> NodeView<'graph, 'cell> {
    /// A node over `tree`, laid through `writer`.
    pub(crate) fn of(
        writer: Writer<'cell, '_>,
        tree: Option<&'cell CodeNode<'graph, 'cell>>,
        source: SourceRef,
    ) -> &'cell Self {
        resident(writer, NodeView { tree, source })
    }

    /// A node of the one item `item`.
    pub(crate) fn single(
        writer: Writer<'cell, '_>,
        item: CodeItem<'graph, 'cell>,
        source: SourceRef,
    ) -> &'cell Self {
        Self::of(writer, Some(view::of(writer, item)), source)
    }

    fn summary(&self) -> CodeSummary {
        self.tree
            .map_or_else(CodeSummary::default, |tree| tree.summary())
    }

    /// How many parts it holds.
    pub(crate) fn len(&self) -> usize {
        self.summary().parts.len as usize
    }

    /// The part at `at`, below [`len`](Self::len): the item holding it, and the part itself where
    /// the item is a stretch of a flat node, or the literal as written where it is a literal.
    fn located(
        &self,
        at: usize,
    ) -> (
        CodeItem<'graph, 'cell>,
        Option<(&'cell ExpressionPart<'cell>, usize)>,
    ) {
        let tree = self.tree.expect("a part lies in a node that holds one");
        let (item, offset, flip, _) = tree.locate(at);
        match item {
            CodeItem::Parts {
                node,
                from,
                to,
                reversed,
                ..
            } => {
                let index = match reversed ^ flip {
                    false => from as usize + offset,
                    true => to as usize - 1 - offset,
                };
                (item, Some((&node.parts[index].value, index)))
            }
            CodeItem::Nested { .. } => (item, None),
            // Its index is read only by a stretch's `code i`, which a literal never answers.
            CodeItem::Literal(literal) => (item, Some((&literal.part.value, 0))),
        }
    }

    /// How deep the node it lays nests, as [`KExpression::depth`] counts it: its deepest part
    /// under the levels the node adds — those of an operator run its rewrite folds, two for a lone
    /// `!=` pair, and one otherwise.
    pub(crate) fn depth(&self) -> u32 {
        let parts = self.summary().parts;
        let levels = if parts.is_operator_run() {
            (parts.len - 1) / 2 + 3
        } else if parts.len == 3
            && let (_, Some((ExpressionPart::Keyword(symbol), _))) = self.located(1)
            && *symbol == KEYWORDS.unequal.symbol()
        {
            2
        } else {
            1
        };
        levels + parts.deepest
    }

    /// What its flat copy writes past the binder plans' share: its own node at its bound, and its
    /// parts, nested views' flat copies among them.
    fn exact(&self) -> u64 {
        let parts = self.summary().parts;
        KExpression::own_bound(parts.len as usize, 0) + u64::from(parts.weight)
    }

    /// The binder plans' share its flat copy may write beyond [`exact`](Self::exact): its own
    /// node's and every new node's below it. A plan's long key is a signature's slots, a part of
    /// a stretch, never a nested view, so each node's share is priced over its stretches alone and
    /// does not compound as views nest.
    fn slack(&self) -> u64 {
        let summary = self.summary();
        let len = summary.parts.len as usize;
        summary.slack + KExpression::own_bound(len, summary.flat) - KExpression::own_bound(len, 0)
    }

    /// Its summary as one part of a node, standing as `wrap`.
    fn as_part(&self, wrap: Wrap) -> CodeSummary {
        let word = (size_of::<Spanned<ExpressionPart<'_>>>() + size_of::<KeyElement>()) as u64;
        CodeSummary {
            parts: PartsSummary {
                len: 1,
                weight: u32::try_from(word + self.exact()).unwrap_or(u32::MAX),
                non_groups: u32::from(wrap == Wrap::Quote),
                keywords: 0,
                breaks: 0,
                first_keyword: false,
                last_keyword: false,
                deepest: self.depth(),
            },
            flat: 0,
            slack: self.slack(),
            envs: self.summary().envs,
        }
    }

    /// The code kind of the node it lays, as [`KExpression::code_kind`] reads it: a block where
    /// two parts or more are all groups; an expression past the longest builtin shape's key, which
    /// no builtin shape matches; read off its key where the builtin shape it matches declares no
    /// binder; and otherwise the kind of a shallow node over its parts, whose binder plan says, read
    /// in storage of its own, each nested item stood in by an empty group or quote.
    fn kind(&self) -> KType {
        let parts = self.summary().parts;
        if parts.len >= 2 && parts.non_groups == 0 {
            return KType::BLOCK;
        }
        if parts.len as usize > longest_builtin_key() || parts.len == 0 {
            return KType::EXPRESSION;
        }
        let key = (0..parts.len as usize).map(|at| match self.located(at) {
            (_, Some((part, _))) => part.key_element(),
            (_, None) => KeyElement::Slot,
        });
        match builtin_shape_for(key) {
            Some(shape) if shape.id.declares_member() => return KType::DECLARATION,
            Some(shape) if shape.binder.is_some() => {}
            _ if parts.len > 1 => return KType::EXPRESSION,
            _ => {
                return match self.located(0) {
                    (_, Some((part, _))) => KExpression::lone_kind(part),
                    (
                        CodeItem::Nested {
                            wrap: Wrap::Group, ..
                        },
                        None,
                    ) => KType::EXPRESSION,
                    (_, None) => KType::LITERAL,
                };
            }
        }
        let storage = program_storage();
        let writer = storage.brand().writer();
        let empty = KExpression::nested(writer, &[], self.source);
        let scratch = Bump::new();
        let mut run = BumpVec::with_capacity_in(parts.len as usize, &scratch);
        for at in 0..parts.len as usize {
            let value = match self.located(at) {
                (_, Some((part, _))) => *part,
                (
                    CodeItem::Nested {
                        wrap: Wrap::Group, ..
                    },
                    None,
                ) => ExpressionPart::Expression(empty),
                (_, None) => ExpressionPart::QuotedExpression(empty),
            };
            run.push(Spanned::bare(value));
        }
        KExpression::build(writer, &run, self.source).code_kind()
    }

    #[cfg(test)]
    pub(crate) fn holds_its_laws(&self) -> bool {
        self.tree.is_none_or(|tree| tree.holds_its_laws())
    }
}

/// The longest key a builtin shape matches: a node of more parts matches none.
fn longest_builtin_key() -> usize {
    (BUILTIN_SHAPES.iter())
        .map(|shape| shape.elements.len())
        .max()
        .unwrap_or(0)
}

/// A code view: its root node, and the code kind of the node it lays, which its carried type is
/// unbuilt at.
#[derive(Debug)]
pub(crate) struct CodeTree<'graph, 'cell> {
    root: &'cell NodeView<'graph, 'cell>,
    kind: KType,
}

impl<'graph, 'cell> CodeTree<'graph, 'cell> {
    /// A view over `root`, laid through `writer`.
    pub(crate) fn of(
        writer: Writer<'cell, '_>,
        root: &'cell NodeView<'graph, 'cell>,
    ) -> &'cell Self {
        resident(
            writer,
            CodeTree {
                root,
                kind: root.kind(),
            },
        )
    }

    pub(crate) fn root(&self) -> &'cell NodeView<'graph, 'cell> {
        self.root
    }

    /// The code kind of the node it lays.
    pub(crate) fn kind(&self) -> KType {
        self.kind
    }

    /// How many parts its root holds.
    pub(crate) fn len(&self) -> usize {
        self.root.len()
    }

    /// How deep the syntax it lays nests.
    pub(crate) fn depth(&self) -> u32 {
        self.root.depth()
    }

    /// What its flat copy writes, at most: its syntax, every binder plan its new nodes may lay,
    /// and its environments' bindings, each priced once per leaf.
    pub(crate) fn weight(&self) -> Weight {
        let summary = self.root.summary();
        let bytes = self.root.exact() + self.root.slack() + summary.envs;
        Weight::text(usize::try_from(bytes).unwrap_or(usize::MAX))
    }

    /// The view of `node`'s parts through `env`: the root of a flat code value, its marks resolving
    /// through its bindings.
    pub(crate) fn over(
        writer: Writer<'cell, '_>,
        node: &'cell KExpression<'cell>,
        env: Env<'graph, 'cell>,
    ) -> &'cell NodeView<'graph, 'cell> {
        let tree = (!node.parts.is_empty())
            .then(|| view::of(writer, CodeItem::parts(node, (0, node.parts.len()), env)));
        NodeView::of(writer, tree, node.source)
    }

    /// `code i`: part `at` of the root as code, `at` below [`len`](Self::len) — a group part as
    /// the node it wraps, any other part as a one-part node.
    pub(crate) fn part(
        &self,
        writer: Writer<'cell, '_>,
        at: usize,
    ) -> &'cell NodeView<'graph, 'cell> {
        match self.root.located(at) {
            (CodeItem::Parts { node, env, .. }, Some((part, index))) => match part {
                ExpressionPart::Expression(inner) => Self::over(writer, inner, env),
                _ => {
                    let item = CodeItem::parts(node, (index, index + 1), env);
                    NodeView::single(writer, item, node.source)
                }
            },
            (
                CodeItem::Nested {
                    wrap: Wrap::Group,
                    node,
                },
                _,
            ) => node,
            (item, _) => NodeView::single(writer, item, self.root.source),
        }
    }

    /// `code {from, to}`: the root's parts `[from, to)`, `from ≤ to ≤ len`, read backward where
    /// `backward` is set.
    pub(crate) fn slice(
        &self,
        writer: Writer<'cell, '_>,
        (from, to): (usize, usize),
        backward: bool,
    ) -> &'cell NodeView<'graph, 'cell> {
        let tree = (self.root.tree).and_then(|tree| view::sliced(writer, tree, from, to));
        let tree = match backward {
            true => tree.map(|tree| view::reversed(writer, tree)),
            false => tree,
        };
        NodeView::of(writer, tree, self.root.source)
    }
}

/// A view laid flat: its root syntax, and the bindings its marks read, sorted by scope, then
/// name, each a value word of its environment, beside where its value stands among the view's
/// [`listed`] values.
pub(crate) struct Flat<'to, 'graph, 'cell, 'x> {
    pub body: &'to KExpression<'to>,
    pub bindings: BumpVec<'x, Binding<'graph, 'cell>>,
    pub sources: BumpVec<'x, usize>,
}

/// Each environment `root`'s stretches resolve through, once, in pre-order of its leaves: the
/// order [`listed`] lists their bindings in.
fn environments<'graph, 'cell, 'x>(
    root: &NodeView<'graph, 'cell>,
    seen: &mut crate::memory::BumpBackedSet<'x, (bool, usize)>,
    out: &mut BumpVec<'x, Env<'graph, 'cell>>,
) {
    for (item, _, _) in root.tree.into_iter().flat_map(|tree| tree.leaves()) {
        match item {
            CodeItem::Parts { env, .. } => {
                if seen.insert(env.identity()) {
                    out.push(env);
                }
            }
            CodeItem::Nested { node, .. } => environments(node, seen, out),
            CodeItem::Literal(literal) => {
                if seen.insert(literal.env.identity()) {
                    out.push(literal.env);
                }
                for (_, _, node) in literal.children {
                    environments(node, seen, out);
                }
            }
        }
    }
}

/// Every value `tree`'s environments bind, each edge resolved, in [`environments`]' order, every
/// binding of each: what a crossing copies of a view and its content digest covers, without laying
/// it flat. Its flat bindings are among them, each at its [`Flat::sources`] position.
pub(crate) fn listed<'graph, 'cell>(
    tree: &CodeTree<'graph, 'cell>,
    out: &mut dyn FnMut(KValue<'graph, 'cell>),
) {
    let scratch = Bump::new();
    let (mut seen, mut envs) = (crate::memory::bump_set(&scratch), BumpVec::new_in(&scratch));
    environments(tree.root, &mut seen, &mut envs);
    for env in envs {
        env.bindings()
            .iter()
            .for_each(|binding| out(env.value(binding)));
    }
}

/// `tree` laid flat through `writer`, its bindings gathered in `scratch`: see the module
/// documentation.
pub(crate) fn materialized<'to, 'graph, 'cell, 'x>(
    writer: Writer<'to, '_>,
    tree: &CodeTree<'graph, 'cell>,
    scratch: &'x Bump,
) -> Flat<'to, 'graph, 'cell, 'x> {
    // Where each environment's bindings start among the listed values, by its identity.
    let (mut seen, mut envs) = (crate::memory::bump_set(scratch), BumpVec::new_in(scratch));
    environments(tree.root, &mut seen, &mut envs);
    let mut offsets = BumpVec::with_capacity_in(envs.len(), scratch);
    let mut offset = 0;
    for env in &envs {
        offsets.push((env.identity(), offset));
        offset += env.bindings().len();
    }
    offsets.sort_unstable_by_key(|(identity, _)| *identity);
    let mut laying = Laying {
        writer,
        scopes: BumpVec::new_in(scratch),
        offsets,
        bindings: BumpVec::new_in(scratch),
        next: 0,
    };
    let body = laying.node(tree.root, false);
    debug_assert!(
        body.depth() as u32 <= tree.depth(),
        "a view's depth bounds its laying"
    );
    let mut gathered = laying.bindings;
    gathered.sort_unstable_by_key(|(binding, _)| binding.key());
    let mut bindings = BumpVec::with_capacity_in(gathered.len(), scratch);
    let mut sources = BumpVec::with_capacity_in(gathered.len(), scratch);
    for (binding, source) in gathered {
        bindings.push(binding);
        sources.push(source);
    }
    Flat {
        body,
        bindings,
        sources,
    }
}

/// One materialization's writer, its canonical scopes by environment and source scope, sorted for
/// a binary search, where each environment's bindings start among the listed values, the bindings
/// it gathered beside theirs, and the next scope number.
struct Laying<'to, 'w, 'graph, 'cell, 'x> {
    writer: Writer<'to, 'w>,
    scopes: BumpVec<'x, ((bool, usize), Scope, u32)>,
    offsets: BumpVec<'x, ((bool, usize), usize)>,
    bindings: BumpVec<'x, (Binding<'graph, 'cell>, usize)>,
    next: u32,
}

impl<'to, 'graph, 'cell> Laying<'to, '_, 'graph, 'cell, '_> {
    /// `view` laid as a node; `quoted` where a quote value lies between the laid root and it.
    fn node(&mut self, view: &NodeView<'graph, 'cell>, quoted: bool) -> &'to KExpression<'to> {
        let writer = self.writer;
        let scratch = Bump::new();
        // Which slots of the node being laid are read as written, from its key before its parts.
        let mut items = BumpVec::new_in(&scratch);
        let mut keys = BumpVec::new_in(&scratch);
        let mut splicing = false;
        for (item, flip, _) in view.tree.into_iter().flat_map(|tree| tree.leaves()) {
            match item {
                CodeItem::Parts {
                    node,
                    from,
                    to,
                    reversed,
                    env,
                    ..
                } => {
                    let indices = from as usize..to as usize;
                    let before = node.written_slots();
                    let mut push = |index: usize| {
                        let part = &node.parts[index];
                        splicing |= matches!(part.value, ExpressionPart::Splice(_));
                        keys.push(part.value.key_element());
                        items.push(Laid::Part(node, index, before.at(index), env));
                    };
                    match reversed ^ flip {
                        false => indices.for_each(&mut push),
                        true => indices.rev().for_each(&mut push),
                    }
                }
                CodeItem::Nested { wrap, node } => {
                    keys.push(KeyElement::Slot);
                    items.push(Laid::Nested(wrap, node));
                }
                CodeItem::Literal(literal) => {
                    keys.push(KeyElement::Slot);
                    items.push(Laid::Literal(literal));
                }
            }
        }
        let now = WrittenSlots::of(builtin_shape_for(keys.iter().copied()), splicing);
        let mut run = BumpVec::with_capacity_in(items.len(), &scratch);
        for (index, item) in items.iter().enumerate() {
            let written = now.at(index);
            let (value, span) = match *item {
                Laid::Part(node, source, before, env) => {
                    let part = &node.parts[source];
                    let at = Quoted {
                        before: false,
                        now: quoted,
                    };
                    let mut remark = Remarking { laying: self, env };
                    let value = remarked(writer, &part.value, (before, written), at, &mut remark);
                    (value, part.span)
                }
                Laid::Nested(Wrap::Group, node) => (
                    ExpressionPart::Expression(self.node(node, quoted)),
                    Some(node.source.span),
                ),
                Laid::Nested(Wrap::Quote, node) => {
                    let inner = self.node(node, quoted || !written);
                    (
                        ExpressionPart::QuotedExpression(inner),
                        Some(node.source.span),
                    )
                }
                Laid::Literal(literal) => {
                    let mut next = 0;
                    let written = (literal.before, written);
                    let value =
                        self.literal(&literal.part.value, literal, written, quoted, &mut next);
                    debug_assert_eq!(next, literal.children.len(), "every composed element lays");
                    (value, literal.part.span)
                }
            };
            run.push(Spanned { value, span });
        }
        KExpression::nested(writer, &run, view.source)
    }

    /// `part`, `view`'s literal or one nested in it, laid with its slot read as `written` before and
    /// now, each composed element laid from its view as [`Laid::Nested`] is, every other element
    /// as a stretch's part is; `next` as [`LiteralView::child`] reads it.
    fn literal(
        &mut self,
        part: &ExpressionPart<'cell>,
        view: &LiteralView<'graph, 'cell>,
        written: (bool, bool),
        quoted: bool,
        next: &mut usize,
    ) -> ExpressionPart<'to> {
        let writer = self.writer;
        let scratch = Bump::new();
        literal_laid(
            writer,
            &scratch,
            part,
            &mut |element| match view.child(element, next) {
                Some((Wrap::Group, node)) => ExpressionPart::Expression(self.node(node, quoted)),
                Some((Wrap::Quote, node)) => {
                    ExpressionPart::QuotedExpression(self.node(node, quoted || !written.1))
                }
                None if element.is_literal() => self.literal(element, view, written, quoted, next),
                None => {
                    let at = Quoted {
                        before: false,
                        now: quoted,
                    };
                    let mut remark = Remarking {
                        laying: self,
                        env: view.env,
                    };
                    remarked(writer, element, written, at, &mut remark)
                }
            },
        )
    }

    /// The canonical number of `scope` of `env`, its bindings gathered on first sight.
    fn canonical(&mut self, env: Env<'graph, 'cell>, scope: Scope) -> u32 {
        let key = (env.identity(), scope);
        let at =
            (self.scopes).binary_search_by_key(&key, |(identity, scope, _)| (*identity, *scope));
        match at {
            Ok(at) => self.scopes[at].2,
            Err(at) => {
                let number = self.fresh();
                self.scopes.insert(at, (key.0, scope, number));
                let canonical = Scope {
                    number,
                    pinned: false,
                };
                let offset = (self.offsets)
                    .binary_search_by_key(&key.0, |(identity, _)| *identity)
                    .map(|at| self.offsets[at].1)
                    .expect("every environment a laid stretch reads is listed");
                for (index, binding) in env.bindings().iter().enumerate() {
                    if binding.scope == scope {
                        let gathered = Binding {
                            scope: canonical,
                            name: binding.name,
                            link: Link::Value(env.value(binding)),
                        };
                        self.bindings.push((gathered, offset + index));
                    }
                }
                number
            }
        }
    }

    /// A scope number no binding holds yet.
    fn fresh(&mut self) -> u32 {
        self.next += 1;
        self.next - 1
    }
}

/// One part of a node being laid, as the walk gathers them: a part of a flat node, whether its
/// slot there is read as written, and the environment it reads through; a nested view; or a
/// literal some of whose elements composed.
#[derive(Clone, Copy)]
enum Laid<'graph, 'cell> {
    Part(&'cell KExpression<'cell>, usize, bool, Env<'graph, 'cell>),
    Nested(Wrap, &'cell NodeView<'graph, 'cell>),
    Literal(&'cell LiteralView<'graph, 'cell>),
}

/// The materializer's [`Remark`] for the parts of one stretch, through its environment.
struct Remarking<'l, 'to, 'w, 'graph, 'cell, 'x> {
    laying: &'l mut Laying<'to, 'w, 'graph, 'cell, 'x>,
    env: Env<'graph, 'cell>,
}

impl<'graph, 'cell> Remark for Remarking<'_, '_, '_, 'graph, 'cell, '_> {
    fn scope(&mut self, scope: Scope, at: Quoted) -> Scope {
        // A composing quote captured every mark of its own parts that will lie at the root, so
        // its marks lay where they lie now.
        let before = match self.env {
            Env::Code(_) => at.before && !scope.pinned,
            Env::Own(_) => at.now,
        };
        match (before, at.now) {
            (false, now) => Scope {
                number: self.laying.canonical(self.env, scope),
                pinned: now,
            },
            (true, true) => scope,
            (true, false) => Scope {
                number: self.laying.fresh(),
                pinned: false,
            },
        }
    }
}

/// Why a quote holding a splice could not compose where it was born.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeRefused {
    /// A splice's operand is no list.
    NotAList { value: KType },
    /// A spliced element is no code.
    NotCode { element: KType },
    /// The composed code nests deeper than the syntax depth limit.
    TooDeep,
}

/// The view a composing quote of `body` composes where it is born, `staged` its bindings read
/// there: its own parts through those its `$` names read, and each splice's elements spliced in,
/// by the level rule. See the scope README § Splicing. A splice's operand is read only to compose,
/// so the view holds it only where a `$` name reads it too.
pub(crate) fn composed<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    body: &'cell KExpression<'cell>,
    staged: &[Binding<'graph, 'cell>],
    types: &crate::values::Chain<'_, '_, '_, '_>,
    scratch: crate::memory::BumpAllocator<'_>,
) -> Result<&'cell NodeView<'graph, 'cell>, ComposeRefused> {
    let (mut operands, mut marked) = (BumpVec::new_in(scratch), BumpVec::new_in(scratch));
    body.each_owned(&mut |part| match *part {
        ExpressionPart::Splice(name) => operands.push(BinderSymbol::Value(name)),
        ExpressionPart::MarkedName(_, name) => marked.push(name),
        _ => {}
    });
    let read = |binding: &&Binding<'graph, 'cell>| {
        !operands.contains(&binding.name) || marked.contains(&binding.name)
    };
    let mut own = BumpVec::new_in(scratch);
    own.extend(staged.iter().filter(read).copied());
    Composer {
        writer,
        own: collect(writer, own.iter().copied()),
        staged,
        scratch,
    }
    .node(body, types)
    .map(|composed| composed.expect("a composing quote holds a splice among the parts it owns"))
}

/// One composition's writer, the bindings its quote's own `$` names read and all it staged, and
/// the scratch its operands are read in.
struct Composer<'cell, 'w, 'graph, 'x> {
    writer: Writer<'cell, 'w>,
    own: &'cell [Binding<'graph, 'cell>],
    staged: &'x [Binding<'graph, 'cell>],
    scratch: crate::memory::BumpAllocator<'x>,
}

impl<'cell, 'graph> Composer<'cell, '_, 'graph, '_> {
    /// `node` composed, or `None` where no splice lies among the parts it owns. A node is at
    /// **statement level** where every part but its splices is a group: each element adds its
    /// statements — a block's root its own, any other element one group of it — and a node of one
    /// statement is that statement. Otherwise, at **part level**, an element of one part adds that
    /// part, and any other element one group of it. A literal part composes where an element of
    /// it does, a nested literal's too. Each part is visited once: a node's stretches
    /// are laid only once a part of it composes.
    fn node(
        &self,
        node: &'cell KExpression<'cell>,
        types: &crate::values::Chain<'_, '_, '_, '_>,
    ) -> Result<Option<&'cell NodeView<'graph, 'cell>>, ComposeRefused> {
        let writer = self.writer;
        let statement = (node.parts.iter()).all(|part| {
            matches!(
                part.value,
                ExpressionPart::Expression(_) | ExpressionPart::Splice(_)
            )
        });
        let slots = node.written_slots();
        let wrapped = |root: &'cell NodeView<'graph, 'cell>, wrap| {
            view::of(writer, CodeItem::Nested { wrap, node: root })
        };
        let env = Env::Own(self.own);
        // Each part that composes, beside what it adds in its place.
        let mut composing = BumpVec::new_in(self.scratch);
        for (index, part) in node.parts.iter().enumerate() {
            let added = match part.value {
                ExpressionPart::Splice(name) => {
                    let mut added = None;
                    for element in self.elements(name, types)? {
                        let root = super::code::root_view(writer, element).root();
                        let summary = root.summary().parts;
                        let own = match statement {
                            true => summary.len >= 2 && summary.non_groups == 0,
                            false => summary.len == 1,
                        };
                        let element = if own {
                            root.tree
                        } else {
                            Some(wrapped(root, Wrap::Group))
                        };
                        added = view::joined(writer, added, element);
                    }
                    added
                }
                ExpressionPart::Expression(inner) => match self.node(inner, types)? {
                    Some(inner) => Some(wrapped(inner, Wrap::Group)),
                    None => continue,
                },
                ExpressionPart::QuotedExpression(inner) if slots.at(index) => {
                    match self.node(inner, types)? {
                        Some(inner) => Some(wrapped(inner, Wrap::Quote)),
                        None => continue,
                    }
                }
                ref literal if literal.is_literal() => {
                    let mut children = BumpVec::new_in(self.scratch);
                    self.literal(literal, slots.at(index), types, &mut children)?;
                    if children.is_empty() {
                        continue;
                    }
                    let children = collect(writer, children.iter().copied());
                    let literal = LiteralView::of(writer, part, slots.at(index), env, children);
                    Some(view::of(writer, CodeItem::Literal(literal)))
                }
                _ => continue,
            };
            composing.push((index, added));
        }
        if composing.is_empty() {
            return Ok(None);
        }
        let mut tree: Option<&'cell CodeNode<'graph, 'cell>> = None;
        let mut start = 0;
        for (index, added) in composing.iter().copied() {
            if start < index {
                let stretch = CodeItem::parts(node, (start, index), env);
                tree = view::joined(writer, tree, Some(view::of(writer, stretch)));
            }
            tree = view::joined(writer, tree, added);
            start = index + 1;
        }
        if start < node.parts.len() {
            let stretch = CodeItem::parts(node, (start, node.parts.len()), env);
            tree = view::joined(writer, tree, Some(view::of(writer, stretch)));
        }
        let mut composed = NodeView::of(writer, tree, node.source);
        if statement && composed.len() == 1 {
            composed = match composed.located(0) {
                (
                    CodeItem::Nested {
                        wrap: Wrap::Group,
                        node,
                    },
                    _,
                ) => node,
                (CodeItem::Parts { env, .. }, Some((ExpressionPart::Expression(group), _))) => {
                    CodeTree::over(writer, group, env)
                }
                _ => composed,
            };
        }
        if composed.depth() as usize > crate::parse::MAX_SYNTAX_DEPTH {
            return Err(ComposeRefused::TooDeep);
        }
        Ok(Some(composed))
    }

    /// Each element of `literal` that composes, by its address, beside its view and how it stands:
    /// a group, or a quote where the literal's slot is read as `written`. A nested literal's
    /// elements are among them.
    fn literal(
        &self,
        literal: &'cell ExpressionPart<'cell>,
        written: bool,
        types: &crate::values::Chain<'_, '_, '_, '_>,
        out: &mut BumpVec<'_, (usize, Wrap, &'cell NodeView<'graph, 'cell>)>,
    ) -> Result<(), ComposeRefused> {
        let mut at = 0;
        while let Some(element) = literal.child(at) {
            at += 1;
            let (wrap, inner) = match element {
                nested if nested.is_literal() => {
                    self.literal(nested, written, types, out)?;
                    continue;
                }
                ExpressionPart::Expression(inner) => (Wrap::Group, inner),
                ExpressionPart::QuotedExpression(inner) if written => (Wrap::Quote, inner),
                _ => continue,
            };
            if let Some(view) = self.node(inner, types)? {
                out.push((std::ptr::from_ref(element).addr(), wrap, view));
            }
        }
        Ok(())
    }

    /// The elements of the list the splice operand `name` is bound to, each a code member.
    fn elements(
        &self,
        name: crate::symbols::ValueSymbol,
        types: &crate::values::Chain<'_, '_, '_, '_>,
    ) -> Result<impl Iterator<Item = Knotted<'graph, 'cell>> + use<'graph, 'cell, '_>, ComposeRefused>
    {
        let binding =
            crate::values::binding_at(self.staged, Scope::WRITTEN, BinderSymbol::Value(name))
                .expect("a composing quote captures each splice's operand");
        let value = Env::Own(self.own).value(binding);
        let listed = matches!(value, crate::values::Value::List(_))
            || matches!(
                value.as_circular(),
                Some((_, crate::values::Circular::List(_)))
            );
        let surface = (value.surface(types, self.scratch)).filter(|_| listed);
        let Some(surface) = surface else {
            return Err(ComposeRefused::NotAList {
                value: value.concrete_ktype(),
            });
        };
        let mut elements = BumpVec::with_capacity_in(surface.len(), self.scratch);
        for at in 0..surface.len() {
            let element = surface.child(at, types, self.scratch).value();
            match element.as_code() {
                Some(code) => elements.push(code),
                None => {
                    return Err(ComposeRefused::NotCode {
                        element: element.concrete_ktype(),
                    });
                }
            }
        }
        Ok(elements.into_iter())
    }
}
