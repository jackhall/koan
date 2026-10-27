//! A **term**: the plain-data copy of a type's nodes [`TypeChain::export`] writes, so a type a
//! chain that dies interned can be minted again, by [`TypeChain::mint`], in any chain over the same
//! root. Minting is the transfer an adopt takes ([transfer.rs](super::transfer)), the term standing
//! in as its source, so one descent re-lays a type wherever it goes and one type keeps one handle.
//!
//! Each node of a term keeps its source's handle and content, its children by reference to the
//! term's own nodes. A node the reader resolves itself — the root's, or one the reader the export
//! names already holds — is a stub holding its handle alone, which the term never offers as a
//! source, so a term carries no type its reader has. A term is `Copy`, covariant and drop-free: it
//! may rest where no type may, since no chain resolves a handle through it.

use std::cell::RefCell;

use crate::bump::{BumpAllocator, BumpVec, LaysRuns};

use super::chain::TypeChain;
use super::component::Component;
use super::digest::ComponentMember;
use super::handle::{DeclaredType, Handle, Parametric, wrap};
use super::node::NodeSchema;
use super::stored::{Content, Linked, StoredNode, copied};
use super::tiers::{Habitat, lay_one};
use super::transfer::{Source, schema_handles, side_handles};

/// The nodes of one type a chain holds past what its reader resolves, copied as plain data: see
/// the module documentation.
#[derive(Clone, Copy)]
pub struct Term<'a> {
    handle: Handle,
    /// Every node copied, sorted by digest. No stub is listed.
    nodes: &'a [Linked<'a>],
}

impl Term<'_> {
    /// How many nodes the term copied: none where its reader resolved the type itself.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl<'a> Source<'a> for &Term<'a> {
    fn reach(&self, handle: Handle) -> Option<Linked<'a>> {
        let at = (self.nodes)
            .binary_search_by_key(&handle.digest(), |node| node.digest())
            .ok()?;
        Some(self.nodes[at])
    }
}

/// What an export has copied so far.
struct Copied<'a, 's> {
    /// Every node met, a copy or a stub, by its handle.
    nodes: BumpVec<'s, (Handle, Linked<'a>)>,
    /// Every component copied, by digest.
    components: BumpVec<'s, &'a Component<'a>>,
    /// The copies alone, in the order laid.
    laid: BumpVec<'s, Linked<'a>>,
}

impl<'t, H: Habitat<'t>> TypeChain<'t, H> {
    /// A term of `kt`'s nodes laid through `lay`, each node `reader` resolves a stub. Every
    /// handle a side table holds is copied too, as an adopt makes each resolve. The working set
    /// is staged over `scratch`.
    pub fn export<'a, 'r>(
        &self,
        kt: impl Into<DeclaredType<Parametric>>,
        reader: &TypeChain<'r, impl Habitat<'r>>,
        lay: BumpAllocator<'a>,
        scratch: BumpAllocator<'_>,
    ) -> Term<'a> {
        let handle = kt.into().raw();
        let met = RefCell::new(Copied {
            nodes: BumpVec::new_in(scratch),
            components: BumpVec::new_in(scratch),
            laid: BumpVec::new_in(scratch),
        });
        self.export_handle(handle, reader, lay, &met);
        let mut nodes = met.into_inner().laid;
        nodes.sort_unstable_by_key(|node| node.digest());
        Term {
            handle,
            nodes: copied(lay, &nodes),
        }
    }

    /// Make `term`'s type resolve in this chain, laying in the own tier each of its nodes the
    /// chain lacks. Every stub must resolve here as it did for the export's reader.
    pub fn mint(&self, term: &Term<'_>) {
        self.adopt_in(&term, wrap::<Parametric>(term.handle));
        debug_assert!(
            self.resolve(term.handle)
                .is_some_and(|node| node.handle() == term.handle),
            "a minted term gives back its handle"
        );
    }

    /// [`export`](Self::export) of one handle: its copy, or its stub, met once.
    fn export_handle<'a, 'r>(
        &self,
        handle: Handle,
        reader: &TypeChain<'r, impl Habitat<'r>>,
        lay: BumpAllocator<'a>,
        copied: &RefCell<Copied<'a, '_>>,
    ) -> Linked<'a> {
        if let Some((_, met)) = (copied.borrow().nodes.iter()).find(|(met, _)| *met == handle) {
            return met;
        }
        if handle.in_root() || reader.resolves(wrap::<Parametric>(handle)) {
            let stub = lay_one(lay, StoredNode::copied(handle, Content::Never));
            copied.borrow_mut().nodes.push((handle, stub));
            return stub;
        }
        let from = self.enter(handle);
        let content = from.content().transfer(
            lay,
            &mut |child| self.export_handle(child.handle(), reader, lay, copied),
            &mut |component| self.export_component(component, reader, lay, copied),
        );
        let node = lay_one(lay, StoredNode::copied(handle, content));
        {
            let mut copied = copied.borrow_mut();
            copied.nodes.push((handle, node));
            copied.laid.push(node);
        }
        side_handles(content, &mut |side| {
            self.export_handle(side.raw(), reader, lay, copied);
        });
        node
    }

    /// `from` copied through `lay`, its members' schemas' handles exported after.
    fn export_component<'a, 'r>(
        &self,
        from: &Component<'_>,
        reader: &TypeChain<'r, impl Habitat<'r>>,
        lay: BumpAllocator<'a>,
        copied: &RefCell<Copied<'a, '_>>,
    ) -> &'a Component<'a> {
        if let Some(met) = (copied.borrow().components.iter()).find(|met| met.digest == from.digest)
        {
            return met;
        }
        let members = from.members();
        let relaid = lay.lay(members.len(), |at| ComponentMember {
            schema: copied_schema(lay, members[at].schema),
            ..members[at]
        });
        let component = Component::absolute(lay, from.digest, relaid);
        copied.borrow_mut().components.push(component);
        for member in relaid {
            schema_handles(member.schema, &mut |side| {
                self.export_handle(side.raw(), reader, lay, copied);
            });
        }
        component
    }
}

/// `schema` with its parameter names copied through `lay`.
fn copied_schema<'a>(lay: BumpAllocator<'a>, schema: NodeSchema<'_>) -> NodeSchema<'a> {
    match schema {
        NodeSchema::NewType(repr) => NodeSchema::NewType(repr),
        NodeSchema::TypeConstructor {
            representation,
            param_names,
        } => NodeSchema::TypeConstructor {
            representation,
            param_names: copied(lay, param_names),
        },
    }
}
