//! Where the holes a `USING` fills lie in the syntax its code was written as: each name part to
//! mark filled, and each keyworded use or operator run to hold under a filled use, beside the keys
//! it fills. `knot`'s `USING` copies the written syntax with these marks (scope README § Holes and
//! marks).
//!
//! A shape resolves its statements as the operator-run rewrite left them, which are the written
//! statements except where the rewrite built a node. So one walk reads each shape's own statements,
//! every body nested in them read through its own shape, and answers each place it finds in the
//! written syntax through [`Shapes::origin`]: a node nothing changed is the written node, a node
//! rebuilt over a rewritten part stands for the written node it rebuilt, and a node chained for an
//! operator run stands for that run. Every operand of one run resolves alike, so a name hole any
//! use of the run reads fills each operand of the run that names it, and a key hole any of its uses
//! selects from is filled at the run, its other keys left holes.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{ExpressionPart, KExpression};
use crate::symbols::{BinderSymbol, KeySymbol};

use super::shape::{BodyShape, Candidate, CaptureSlot, Lands};
use super::shapes::{Origin, Shapes};

/// The written places a `USING` fills, by address: each name part it marks filled, and each node
/// it holds under a filled use beside a key it fills, sorted.
pub struct FilledSites<'x> {
    names: BumpVec<'x, usize>,
    uses: BumpVec<'x, (usize, KeySymbol)>,
}

impl FilledSites<'_> {
    /// Whether `part` is a name the `USING` fills.
    pub fn name(&self, part: &ExpressionPart<'_>) -> bool {
        self.names
            .binary_search(&std::ptr::from_ref(part).addr())
            .is_ok()
    }

    /// The keys the `USING` fills at `node`, sorted: empty where it fills none.
    pub fn keys(&self, node: &KExpression<'_>) -> impl Iterator<Item = KeySymbol> + '_ {
        let address = node.parts.as_ptr().addr();
        let from = self.uses.partition_point(|(at, _)| *at < address);
        self.uses[from..]
            .iter()
            .take_while(move |(at, _)| *at == address)
            .map(|(_, key)| *key)
    }
}

/// Where the holes `holes` of `code`, a code shape `shapes` holds, lie in the syntax it was
/// written as.
pub fn filled_sites<'x>(
    shapes: &Shapes<'_>,
    code: &BodyShape<'_>,
    holes: &[CaptureSlot],
    scratch: BumpAllocator<'x>,
) -> FilledSites<'x> {
    let mut locating = Locating {
        shapes,
        code,
        holes,
        sites: FilledSites {
            names: BumpVec::new_in(scratch),
            uses: BumpVec::new_in(scratch),
        },
    };
    locating.body(code);
    let sites = &mut locating.sites;
    sites.names.sort_unstable();
    sites.names.dedup();
    sites.uses.sort_unstable();
    sites.uses.dedup();
    locating.sites
}

/// What a node of a shape's statements stands for in the written syntax.
#[derive(Clone, Copy)]
enum Written<'a, 'graph> {
    /// A written node, its parts the written parts at the same indices.
    Node(&'a KExpression<'graph>),
    /// A node chained for this written operator run.
    Run(KExpression<'graph>),
}

/// [`filled_sites`]'s walk.
struct Locating<'held, 'graph, 'x> {
    shapes: &'held Shapes<'graph>,
    code: &'held BodyShape<'graph>,
    holes: &'held [CaptureSlot],
    sites: FilledSites<'x>,
}

impl<'graph> Locating<'_, 'graph, '_> {
    /// Whether `lands` is one of the holes being filled.
    fn fills(&self, lands: Option<Lands>) -> bool {
        matches!(lands, Some(Lands::Hole { at, capture })
            if at == self.code.segment() && self.holes.contains(&capture))
    }

    fn body(&mut self, shape: &BodyShape<'graph>) {
        for statement in shape.body() {
            self.node(shape, statement);
        }
    }

    fn node(&mut self, shape: &BodyShape<'graph>, node: &KExpression<'graph>) {
        match self.shapes.origin(node) {
            None => self.within(shape, node, Written::Node(node)),
            // A rebuilt node is its written node with a part rewritten: read part by part.
            Some(Origin::Rebuilt(written)) => {
                debug_assert_eq!(node.parts.len(), written.parts.len());
                self.within(shape, node, Written::Node(&written))
            }
            Some(Origin::Run(run)) => self.within(shape, node, Written::Run(run)),
        }
    }

    /// `node`'s use and parts, `node` standing for `written`.
    fn within(
        &mut self,
        shape: &BodyShape<'graph>,
        node: &KExpression<'graph>,
        written: Written<'_, 'graph>,
    ) {
        self.uses(shape, node, written);
        for (index, part) in node.parts.iter().enumerate() {
            let at = match written {
                Written::Node(node) => Some(&node.parts[index].value),
                Written::Run(_) => None,
            };
            self.part(shape, &part.value, at, written);
        }
    }

    /// Record each key of `node`, a keyworded use, a filled key hole is selected from at.
    fn uses(
        &mut self,
        shape: &BodyShape<'graph>,
        node: &KExpression<'graph>,
        written: Written<'_, 'graph>,
    ) {
        let Some(list) = (shape.sites().find_node(node)).and_then(|site| shape.candidates(site))
        else {
            return;
        };
        let filled = list.candidates.iter().any(|candidate| match candidate {
            Candidate::Spread(coordinate) => {
                self.fills(self.shapes.follow(shape.segment(), *coordinate))
            }
            Candidate::One(_) => false,
        });
        if filled {
            let at = match written {
                Written::Node(node) => node.parts.as_ptr().addr(),
                Written::Run(run) => run.parts.as_ptr().addr(),
            };
            self.sites.uses.push((at, list.key));
        }
    }

    /// `part` of a node standing for `written`, `at` the written part it is where that node is
    /// written.
    fn part(
        &mut self,
        shape: &BodyShape<'graph>,
        part: &ExpressionPart<'graph>,
        at: Option<&ExpressionPart<'graph>>,
        written: Written<'_, 'graph>,
    ) {
        if let Some(site) = shape.sites().find(part) {
            // A body nested here is read through its own shape.
            if let Some(nested) = shape.nested_in_place(site) {
                return self.body(nested);
            }
            if let Some(mention) = shape.mention(site)
                && self.fills(Some(mention.lands))
            {
                match (at, written) {
                    (Some(at), _) => self.sites.names.push(std::ptr::from_ref(at).addr()),
                    (None, Written::Run(run)) => self.operands(&run, mention.name),
                    (None, Written::Node(_)) => unreachable!("a written node's parts are written"),
                }
            }
        }
        let item_at = |index: usize| match at {
            Some(ExpressionPart::ListLiteral(items)) => Some(items[index].part()),
            _ => None,
        };
        match part {
            ExpressionPart::Expression(node)
            | ExpressionPart::SigiledTypeExpr(node)
            | ExpressionPart::RecordType(node)
            | ExpressionPart::QuotedExpression(node)
            | ExpressionPart::MarkedUse(_, node)
            | ExpressionPart::FilledUse(_, _, node) => self.node(shape, node),
            ExpressionPart::ListLiteral(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.part(shape, item.part(), item_at(index), written);
                }
            }
            ExpressionPart::DictLiteral(pairs) => {
                for (index, (key, value)) in pairs.iter().enumerate() {
                    let at = match at {
                        Some(ExpressionPart::DictLiteral(written)) => Some(&written[index]),
                        _ => None,
                    };
                    self.part(shape, key, at.map(|(key, _)| key), written);
                    self.part(shape, value, at.map(|(_, value)| value), written);
                }
            }
            ExpressionPart::RecordLiteral(fields) => {
                for (index, (_, value)) in fields.iter().enumerate() {
                    let at = match at {
                        Some(ExpressionPart::RecordLiteral(written)) => Some(&written[index].1),
                        _ => None,
                    };
                    self.part(shape, value, at, written);
                }
            }
            _ => {}
        }
    }

    /// Record each operand of the operator run `run` that names `name`.
    fn operands(&mut self, run: &KExpression<'graph>, name: BinderSymbol) {
        for part in run.parts {
            let names = match (part.value, name) {
                (ExpressionPart::Identifier(written), BinderSymbol::Value(name)) => written == name,
                (ExpressionPart::Type(written), BinderSymbol::Type(name)) => written == name,
                _ => false,
            };
            if names {
                self.sites
                    .names
                    .push(std::ptr::from_ref(&part.value).addr());
            }
        }
    }
}
