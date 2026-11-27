//! **Module substitution**: what a binding makes a path class's variable, and what a path reads
//! off a module's content tree ([module design](../../../design/modules.md#comparing-two-paths)).
//!
//! A class's paths read through a module binder: where the binding binds that binder, the path is
//! read off the module it carries ([`path_binding`]), as a walk resolving the path under the
//! binding reads it; a view's own class is the carrier the view the binding substitutes hides its
//! label behind; otherwise the class is the one its paths fall in with the binding written into
//! their application and tree ([`Resolved::class_of`]), filed beside its recipe.
//!
//! Each is closed over the class's recipe and what [`ClassFacts`] hands it: the binding's carried
//! types, the tree it substitutes for an open tree, a body's laid signature and declared types. The
//! load's walk is one caller, resolving a path at its first read too ([`Classes::resolved_off`]).

use crate::elaborate::judge::{self, Derives};
use crate::elaborate::{SignatureMember, path_binding, signature_member};
use crate::knot::module::view::Exposure;
use crate::memory::BumpAllocator;
use crate::scope::{
    Closed, Identity, Leaf, PathEntry, Resolution, Resolved, Root, Segment, Shapes, Slot, Static,
    StaticType, Tree, Unknown,
};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{DeclaredType, KType, Parametric, PathClass, TypeNode};
use crate::values::Chain;

/// A path of a class beside its resolution: what the class reads. Any of the class's paths says
/// alike, since its content names the class.
#[derive(Clone, Copy)]
pub struct ClassOf<'o> {
    pub entry: PathEntry<'o>,
    pub resolution: Resolution<'o>,
}

/// What module substitution reads past a class's recipe: laid facts, and the binding's content.
pub trait ClassFacts<'p, 'o>: Derives {
    /// The recipe of `class`: one of its paths beside its resolution.
    fn class_of(&self, class: PathClass) -> Option<ClassOf<'o>>;

    /// The type the binding gives `unknown`: a type name's type, a module binder's carried type.
    fn carried(&self, unknown: Unknown) -> Option<KType>;

    /// `ty`, written over the names of `segment`'s chain, with the binding written in.
    fn rebind(&self, segment: Segment, ty: DeclaredType<Parametric>) -> Closed;

    /// What the load fixed for the declaration at `slot` of the body `body`.
    fn declared(&self, body: Segment, slot: Slot) -> StaticType<'o>;

    /// The signature every run of the `MODULE` or `GROUP` body `body` ties, where it is known
    /// exactly.
    fn signature(&self, body: Segment) -> Option<KType>;

    /// The tree the binding substitutes for the open tree of `identity`, where it substituted one.
    fn substituted(&self, identity: Identity) -> Option<Tree<'o>>;

    /// What the closed view of `identity` binds, where it is known.
    fn viewed(&self, identity: Identity) -> Option<Option<Exposure<'o, 'p>>>;

    /// File `of` as the recipe of the class `renamed`, which the binding renamed a class to.
    fn file_class(&self, renamed: PathClass, of: ClassOf<'o>);
}

/// One reading's context: the lattice's chain, the scratch, the program's shapes, and the facts
/// it reads.
pub struct Classes<'f, 'p, 'x, 'g, 'tier, 'pair, 'reg, F> {
    pub types: &'x Chain<'tier, 'pair, 'reg, 'x>,
    pub scratch: BumpAllocator<'p>,
    pub shapes: &'g Shapes<'g>,
    pub facts: &'f F,
}

impl<'p, 'o: 'p, 'x, 'g, 'tier: 'o, 'pair, 'reg, F: ClassFacts<'p, 'o>>
    Classes<'_, 'p, 'x, 'g, 'tier, 'pair, 'reg, F>
{
    /// What `entry` reads off its root's content tree `tree`, through its application closed
    /// (`through`, `None` where it is not): a root holding the signature `held` exactly, exact at
    /// `exact`, or a view showing `view`. The type it is at every run; else `Err` beside the
    /// signature every run finds no such member in. Module substitution runs it again over what a
    /// binding makes the root ([`class_bound`](Self::class_bound)).
    pub fn resolved_off(
        &self,
        entry: PathEntry<'o>,
        through: Option<Option<KType>>,
        (held, exact, view): (Option<KType>, Option<Parametric>, Option<KType>),
        tree: Tree<'o>,
    ) -> Result<Resolved, Option<Parametric>> {
        let (types, scratch) = (self.types, self.scratch);
        let closed = through.and_then(|through| {
            let read = |held: KType, chain: &[ValueSymbol]| {
                path_binding(types, scratch, held, chain, entry.label, through)
            };
            held.and_then(|held| read(held, entry.chain))
                .or_else(|| view.and_then(|view| read(view, &[])))
        });
        // A label the application pins is that pin, which *fits* solves it to at every run.
        let pinned =
            through
                .flatten()
                .and_then(|through| match judge::closed_type_node(types, through) {
                    TypeNode::SignatureApply { pins, .. } => {
                        pins.get(BinderSymbol::Type(entry.label).symbol())
                    }
                    _ => None,
                });
        // A type member a body binds closed, read as the module's own.
        let bound = || match (tree, entry.through) {
            (Tree::Body { body, .. }, None) => {
                let shape = self.shapes.shape_at(body);
                let (slot, _) = shape.slot(BinderSymbol::Type(entry.label))?;
                match self.facts.declared(body, slot) {
                    Static::Closed(held) if shape.declarations(slot).is_some() => Some(held),
                    _ => None,
                }
            }
            _ => None,
        };
        // A root exact at an application pinning the label reads that pin, closed or a path's
        // class: at every run the root carries that application, so the path binds what the pin
        // does.
        let read_pin = exact.and_then(|exact| self.read_pin(exact, entry.chain, entry.label));
        match (read_pin, pinned.or(closed).or_else(bound)) {
            (Some(resolved), _) => Ok(resolved),
            (None, Some(closed)) => Ok(Resolved::Closed(closed)),
            // A root whose signature, after the chain, declares no such member reads none at any
            // run, through a known application or over names: the module the chain reaches is
            // what a run finding no such member names. One that declares it, which the load
            // cannot close — a member a nested view hides — is read by class. A root exact at a
            // variable — the type a `:Module` binder carries, a view's name — is what a run
            // finding no such member names.
            (None, None) => Err(match held {
                Some(held) => through.and_then(|_| {
                    self.declaring_none(Parametric::from(held), entry.chain, entry.label)
                }),
                None => exact.filter(|exact| {
                    judge::node_unless_variable(types, *exact).is_none()
                        || self
                            .declaring_none(*exact, entry.chain, entry.label)
                            .is_some()
                }),
            }),
        }
    }

    /// The signature a module of `exact`, a root's exact static type, holds after the value
    /// members `chain`, where it declares no type member `label` at any binding: a signature
    /// declares its members whatever its pins are.
    fn declaring_none(
        &self,
        exact: Parametric,
        chain: &[ValueSymbol],
        label: TypeSymbol,
    ) -> Option<Parametric> {
        let (types, scratch) = (self.types, self.scratch);
        let signature = |held: Parametric| {
            matches!(
                judge::node_unless_variable(types, held),
                Some(TypeNode::Signature { .. } | TypeNode::SignatureApply { .. })
            )
        };
        let mut held = exact;
        for step in chain {
            if !signature(held) {
                return None;
            }
            match signature_member(self.facts, types, scratch, held, BinderSymbol::Value(*step)) {
                Some(SignatureMember {
                    declared: DeclaredType::Type(inner),
                    unpinned: None,
                }) => held = inner,
                _ => return None,
            }
        }
        (signature(held)
            && signature_member(self.facts, types, scratch, held, BinderSymbol::Type(label))
                .is_none())
        .then_some(held)
    }

    /// The pin of `label` in the application `exact`, a root's exact static type, holds after the
    /// value members `chain`: a closed pin, or a path variable's class. `None` for any other pin,
    /// and where `exact` holds no application pinning `label` there.
    fn read_pin(
        &self,
        exact: Parametric,
        chain: &[ValueSymbol],
        label: TypeSymbol,
    ) -> Option<Resolved> {
        let (types, scratch) = (self.types, self.scratch);
        let held =
            crate::elaborate::declared_through(self.facts, types, scratch, exact, chain, label)?;
        let TypeNode::SignatureApply { pins, .. } = judge::node_unless_variable(types, held)?
        else {
            return None;
        };
        let pin = pins.get(BinderSymbol::Type(label).symbol())?;
        if let Some(class) = judge::path_class(types, pin) {
            return Some(Resolved::Class(class));
        }
        // A pin over names is that type at every run, which a binding closes.
        Some(match judge::static_of(types, pin) {
            Some(closed) => Resolved::Closed(closed),
            None => Resolved::Over(pin),
        })
    }

    /// What the binding makes the variable of `class`, named by a record of `segment`'s: where the
    /// class's paths read through a module binder the binding binds, the path read off the module
    /// it carries, as a walk resolving the path under that binding reads it
    /// ([`resolve_entry`](Self::resolve_entry)) — module substitution; a view's own class, the
    /// carrier the view the binding substitutes hides its label behind; otherwise the class the
    /// paths fall in with the binding written into their application and tree, where it differs
    /// ([`Resolved::class_of`]). `None` where the walk holds no path of the class, or the binding
    /// leaves it as it is.
    pub fn class_bound(&self, segment: Segment, class: PathClass) -> Option<Resolved> {
        let of = self.facts.class_of(class)?;
        // A root the binding binds, itself or through another holder of its tree, is exactly the
        // module it carries, whatever its tree's leaves read: the path is read off that, as a
        // fresh load with the root declared reads it.
        if let Some(carried) = self.bound_root(of.entry.root) {
            let entry = of.entry;
            let closed = match of.entry.through {
                None => Some(None),
                Some(through) => match self.facts.rebind(segment, through.into()) {
                    Closed::Same => judge::static_of(self.types, through).map(Some),
                    Closed::Type(DeclaredType::Type(closed)) => Some(Some(closed)),
                    _ => None,
                },
            };
            let resolved = (self.resolved_off(
                entry,
                closed,
                (Some(carried), None, None),
                of.resolution.tree,
            ))
            .ok();
            if resolved.is_some() {
                return resolved;
            }
        }
        let tree = of.resolution.tree;
        let substituted = self.facts.substituted(tree.identity());
        // The application the paths are read through, the binding written in.
        let (through, closed) = match of.entry.through {
            None => (None, Some(None)),
            Some(through) => match self.facts.rebind(segment, through.into()) {
                Closed::Same => (
                    Some(through),
                    judge::static_of(self.types, through).map(Some),
                ),
                Closed::Type(DeclaredType::Type(closed)) => {
                    (Some(closed.into()), Some(Some(closed)))
                }
                Closed::Partial(DeclaredType::Type(partial)) => (Some(partial), None),
                _ => return None,
            },
        };
        match tree {
            // A view's own class: the carrier the view the binding substitutes hides its label
            // behind.
            Tree::View { .. } if of.entry.chain.is_empty() => {
                if let Some(substituted) = substituted
                    && let Some(viewed) = self.facts.viewed(substituted.identity())
                    && let Some(built) = viewed.and_then(|viewed| viewed.built)
                    && let Some(carrier) =
                        crate::type_lattice::member(built.to.bindings, of.entry.label)
                {
                    return judge::static_of(self.types, carrier).map(Resolved::Closed);
                }
            }
            _ => {}
        }
        // The root as the binding shows it: a bound leaf's module, by the type it carries, read
        // through the leaf's chain; then the substituted tree, by the signature it shows closed.
        let leaf = match tree {
            Tree::Leaf(Leaf::Bound {
                binder,
                chain,
                within: None,
                ..
            }) => {
                (self.facts.carried(binder.unknown())).map(|carried| (carried, None, chain, tree))
            }
            _ => None,
        };
        let applied = substituted.and_then(|tree| {
            let held = self.shown(tree)?;
            Some((held, Some(held.into()), &[][..], tree))
        });
        for (held, exact, chain, tree) in leaf.into_iter().chain(applied) {
            let entry = PathEntry { chain, ..of.entry };
            if let Ok(resolved) = self.resolved_off(entry, closed, (Some(held), exact, None), tree)
            {
                return Some(resolved);
            }
        }
        // The class the paths fall in, the binding written into their application and tree.
        let tree = substituted.unwrap_or(tree);
        let renamed = Resolved::class_of(of.entry.label, through, tree.identity())?;
        if renamed == class {
            return None;
        }
        let entry = PathEntry {
            through,
            name: PathEntry::minted(of.entry.root, of.entry.chain, of.entry.label, through),
            ..of.entry
        };
        let resolution = Resolution {
            tree,
            resolved: Resolved::Class(renamed),
            ..of.resolution
        };
        self.facts
            .file_class(renamed, ClassOf { entry, resolution });
        Some(Resolved::Class(renamed))
    }

    /// The type the module the binding binds to `root`'s binder carries, where it binds one.
    fn bound_root(&self, root: Root) -> Option<KType> {
        let Root::Binder(binder) = root else {
            return None;
        };
        self.facts.carried(binder.unknown())
    }

    /// The signature `tree`, a module's content tree, shows at every run, where it fixes one: a
    /// retype's carried type, a frame's module by the signature recorded beside its digest, a
    /// body's own, which the load types whatever its captures hold.
    pub fn shown(&self, tree: Tree<'o>) -> Option<KType> {
        match tree {
            Tree::Retyped { carried, .. } => judge::static_of(self.types, carried),
            Tree::Digest(_, Some(bare)) => bare.signature,
            Tree::Body { body, .. } => self.facts.signature(body),
            _ => None,
        }
    }
}
