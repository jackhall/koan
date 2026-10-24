//! A body's **sites**: each part and node its statements hold, at any depth, numbered in program
//! order. A [`Site`] is an index into its shape's table, plain data that crosses a park and
//! resolves back to its part through the shape, so two builds of one text agree on every site
//! however their parts are laid in memory. Sites of two shapes never compare.
//!
//! The table covers every part [`each_part_within`](super::each_part) reaches from the body's
//! statements, nested bodies and quotes included, so a part an enclosing shape records of a nested
//! one is found there too. A part reached twice — an operand the rewrite hoists stays where it is
//! written — and a node and its first part, which may share an address, take one site.

use crate::memory::{
    BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, Lays, bump_set, bump_table, collect,
};
use crate::parse::{ExpressionPart, KExpression};

use super::super::keyed::KeyedEntry;

/// A part's or a node's place among its shape's sites.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Site(pub(in crate::scope) u32);

impl Site {
    /// Its index into its shape's table.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl KeyedEntry for Site {
    type Key = Site;

    fn key(&self) -> Site {
        *self
    }
}

/// What a site names: a part, or a whole node no part shares an address with.
#[derive(Clone, Copy)]
pub enum Located<'graph> {
    Part(&'graph ExpressionPart<'graph>),
    Node(&'graph KExpression<'graph>),
}

/// One body's sites: each site by the address of what it names, and what each names.
#[derive(Clone, Copy)]
pub struct Sites<'graph> {
    by_address: &'graph [(usize, Site)],
    located: &'graph [Located<'graph>],
}

impl<'graph> Sites<'graph> {
    /// A body with no statements.
    pub(in crate::scope) const EMPTY: Self = Sites {
        by_address: &[],
        located: &[],
    };

    /// The sites of `statements`, laid through `lay`.
    pub(in crate::scope) fn of(
        lay: impl Lays<'graph>,
        scratch: BumpAllocator<'_>,
        statements: &'graph [KExpression<'graph>],
    ) -> Self {
        let mut numbering = Numbering {
            located: BumpVec::new_in(scratch),
            by_address: BumpVec::new_in(scratch),
            numbered: bump_table(scratch),
            nodes: bump_set(scratch),
            parts: bump_set(scratch),
        };
        for node in statements {
            numbering.node(node);
        }
        numbering.by_address.sort_unstable();
        Sites {
            by_address: collect(lay, numbering.by_address.iter().copied()),
            located: collect(lay, numbering.located.iter().copied()),
        }
    }

    /// The site of `part`, which this body's statements hold.
    pub fn site(&self, part: &ExpressionPart<'_>) -> Site {
        self.at(std::ptr::from_ref(part).addr())
    }

    /// The site of a whole node — its parts run's address, which every copy of the node shares.
    pub fn site_of_node(&self, node: &KExpression<'_>) -> Site {
        self.at(node.parts.as_ptr().addr())
    }

    /// The site of `part` where this body's statements hold it.
    pub fn find(&self, part: &ExpressionPart<'_>) -> Option<Site> {
        self.find_at(std::ptr::from_ref(part).addr())
    }

    /// The site of `node` where this body's statements hold it.
    pub fn find_node(&self, node: &KExpression<'_>) -> Option<Site> {
        self.find_at(node.parts.as_ptr().addr())
    }

    /// The site of whatever this body's statements hold at `address`.
    #[cfg(test)]
    pub fn find_at_address(&self, address: usize) -> Option<Site> {
        self.find_at(address)
    }

    /// What `site` names.
    pub fn located(&self, site: Site) -> Located<'graph> {
        debug_assert!(site.index() < self.located.len(), "a site of this shape");
        self.located[site.index()]
    }

    /// The address `site` names, which the searches through a statement compare against.
    pub(in crate::scope) fn address(&self, site: Site) -> usize {
        match self.located(site) {
            Located::Part(part) => std::ptr::from_ref(part).addr(),
            Located::Node(node) => node.parts.as_ptr().addr(),
        }
    }

    /// How many sites the body has.
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.located.len()
    }

    /// Every site of the body, in program order.
    #[cfg(test)]
    pub fn all(&self) -> impl Iterator<Item = Site> {
        (0..self.located.len() as u32).map(Site)
    }

    fn at(&self, address: usize) -> Site {
        self.find_at(address)
            .expect("a part or node of this shape's statements")
    }

    fn find_at(&self, address: usize) -> Option<Site> {
        (self
            .by_address
            .binary_search_by_key(&address, |(at, _)| *at))
        .ok()
        .map(|at| self.by_address[at].1)
    }
}

/// [`Sites::of`]'s walk: what it has numbered, and what it has walked into.
struct Numbering<'graph, 'x> {
    located: BumpVec<'x, Located<'graph>>,
    by_address: BumpVec<'x, (usize, Site)>,
    numbered: BumpBackedMap<'x, usize, Site>,
    nodes: BumpBackedSet<'x, usize>,
    parts: BumpBackedSet<'x, usize>,
}

impl<'graph> Numbering<'graph, '_> {
    /// Number `located` at `address`, unless something there already is.
    fn number(&mut self, address: usize, located: Located<'graph>) {
        if !self.numbered.contains_key(&address) {
            let site = Site(self.located.len() as u32);
            self.numbered.insert(address, site);
            self.located.push(located);
            self.by_address.push((address, site));
        }
    }

    fn node(&mut self, node: &'graph KExpression<'graph>) {
        let address = node.parts.as_ptr().addr();
        if !self.nodes.insert(address) {
            return;
        }
        self.number(address, Located::Node(node));
        for part in node.parts {
            self.part(&part.value);
        }
    }

    fn part(&mut self, part: &'graph ExpressionPart<'graph>) {
        let address = std::ptr::from_ref(part).addr();
        if !self.parts.insert(address) {
            return;
        }
        // A node's first part may share the node's address: the part is what the site names.
        if let Some(&site) = self.numbered.get(&address) {
            self.located[site.index()] = Located::Part(part);
        } else {
            self.number(address, Located::Part(part));
        }
        match part {
            ExpressionPart::Expression(node)
            | ExpressionPart::SigiledTypeExpr(node)
            | ExpressionPart::RecordType(node)
            | ExpressionPart::QuotedExpression(node)
            | ExpressionPart::MarkedUse(_, node) => self.node(node.reference()),
            ExpressionPart::ListLiteral(items) => items.iter().for_each(|item| self.part(item)),
            ExpressionPart::DictLiteral(pairs) => {
                for (key, value) in pairs.iter() {
                    self.part(key);
                    self.part(value);
                }
            }
            ExpressionPart::RecordLiteral(fields) => {
                fields.iter().for_each(|(_, value)| self.part(value))
            }
            ExpressionPart::Keyword(_)
            | ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::MarkedName(..)
            | ExpressionPart::Literal(_) => {}
        }
    }
}
