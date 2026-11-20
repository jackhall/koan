//! **Declared signatures over run-bound names**: a `SIG` whose members read a name a run binds —
//! `SIG Holds = #[(VAL x :Elt)]` under `FOR ALL #[Elt]`. A signature's schema holds no variable,
//! so the signature is a derived name ([`DerivedTerm::Signature`](super::DerivedTerm::Signature))
//! over the skeleton the declaration door read ([`Signed`]), its operands each member's type
//! written over names.
//!
//! The door records a signature through [`Signing`], and both its closed signature and the term's,
//! under a binding, seal through one closed rule, [`sealed_signature`], so a binding written in
//! declares the signature a fresh load with that binding declared gives.

use std::hash::{Hash, Hasher};

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::symbols::{BinderSymbol, KeywordSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    DeclaredType, FoldDirection, KType, Parametric, ReductionMode, SchemaDraft, SigOrigin,
};
use crate::values::Chain;

/// One entry of a declared signature, in the order the door read it. Each entry but a group reads
/// the next operand.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entry<'a> {
    /// A head parameter, the operand its handle.
    Parameter(TypeSymbol),
    /// A manifest type member.
    Manifest(TypeSymbol),
    /// A value slot, a type or a quantified callable's scheme.
    Value(ValueSymbol),
    /// A keyworded member's shape.
    Keyworded,
    /// An operator group over `members`, reducing by `mode`.
    Group(&'a [KeywordSymbol], ReductionMode),
}

impl Hash for Entry<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match *self {
            Entry::Parameter(name) => (0u8, name).hash(state),
            Entry::Manifest(name) => (1u8, name).hash(state),
            Entry::Value(name) => (2u8, name).hash(state),
            Entry::Keyworded => 3u8.hash(state),
            Entry::Group(members, mode) => {
                (4u8, members).hash(state);
                match mode {
                    ReductionMode::Unary => 0u8.hash(state),
                    ReductionMode::FoldLeft => 1u8.hash(state),
                    ReductionMode::FoldRight => 2u8.hash(state),
                    ReductionMode::Pairwise {
                        combiner,
                        direction,
                    } => {
                        let direction = match direction {
                            FoldDirection::Left => 0u8,
                            FoldDirection::Right => 1u8,
                        };
                        (3u8, combiner, direction).hash(state);
                    }
                }
            }
        }
    }
}

/// A signature's skeleton, as the declaration door read a `SIG`, or a `MODULE` body's members
/// declare it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Signed<'a> {
    pub entries: &'a [Entry<'a>],
    /// Whether a `SIG` declares it, rather than a module's own members: part of its identity.
    pub declared: bool,
}

impl Signed<'_> {
    /// The operand among `operands` the member `name` reads, a value slot's or a manifest
    /// member's: `None` where the skeleton declares no such member.
    pub fn member(
        &self,
        operands: &[DeclaredType<Parametric>],
        name: BinderSymbol,
    ) -> Option<DeclaredType<Parametric>> {
        let mut operands = operands.iter().copied();
        for entry in self.entries {
            let named = match *entry {
                Entry::Group(..) => continue,
                Entry::Value(value) => name == BinderSymbol::Value(value),
                Entry::Manifest(member) => name == BinderSymbol::Type(member),
                Entry::Parameter(_) | Entry::Keyworded => false,
            };
            let operand = operands.next()?;
            if named {
                return Some(operand);
            }
        }
        None
    }

    /// The skeleton laid through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Signed<'to> {
        let entries = self.entries.iter().map(|entry| match *entry {
            Entry::Group(members, mode) => {
                Entry::Group(collect(lay, members.iter().copied()), mode)
            }
            Entry::Parameter(name) => Entry::Parameter(name),
            Entry::Manifest(name) => Entry::Manifest(name),
            Entry::Value(name) => Entry::Value(name),
            Entry::Keyworded => Entry::Keyworded,
        });
        Signed {
            entries: collect(lay, entries),
            declared: self.declared,
        }
    }
}

/// What the declaration door records a declared signature through, as it would fill a
/// [`SchemaDraft`]: its skeleton and, in order, each type it reads.
pub struct Signing<'x> {
    entries: BumpVec<'x, Entry<'x>>,
    operands: BumpVec<'x, DeclaredType<Parametric>>,
    scratch: BumpAllocator<'x>,
    declared: bool,
}

impl<'x> Signing<'x> {
    /// An empty record of a `SIG` declaration.
    pub fn new(scratch: BumpAllocator<'x>) -> Self {
        Signing {
            entries: BumpVec::new_in(scratch),
            operands: BumpVec::new_in(scratch),
            scratch,
            declared: true,
        }
    }

    /// An empty record of a module's own signature.
    pub fn module(scratch: BumpAllocator<'x>) -> Self {
        Signing {
            declared: false,
            ..Signing::new(scratch)
        }
    }

    fn read(&mut self, entry: Entry<'x>, ty: DeclaredType<Parametric>) {
        self.entries.push(entry);
        self.operands.push(ty);
    }

    /// Declare the head parameter `name`, read by the members as `parameter`.
    pub fn insert_parameter(&mut self, name: TypeSymbol, parameter: Parametric) {
        self.read(Entry::Parameter(name), parameter.into());
    }

    /// Fix the manifest member `name` to `ty`.
    pub fn insert_manifest(&mut self, name: TypeSymbol, ty: Parametric) {
        self.read(Entry::Manifest(name), ty.into());
    }

    /// Declare the value slot `name` at `ty`.
    pub fn insert_value_slot(&mut self, name: ValueSymbol, ty: DeclaredType<Parametric>) {
        self.read(Entry::Value(name), ty);
    }

    /// Declare a keyworded member of the shape `shape`.
    pub fn push_keyworded(&mut self, shape: DeclaredType<Parametric>) {
        self.read(Entry::Keyworded, shape);
    }

    /// Declare an operator group over `members`, reducing by `mode`.
    pub fn push_operator_group(&mut self, members: &[KeywordSymbol], mode: ReductionMode) {
        let members = self.scratch.alloc_slice_copy(members);
        self.entries.push(Entry::Group(members, mode));
    }

    /// The skeleton and the types it reads.
    pub fn finished(self) -> (Signed<'x>, &'x [DeclaredType<Parametric>]) {
        (
            Signed {
                entries: self.entries.leak(),
                declared: self.declared,
            },
            self.operands.leak(),
        )
    }
}

/// The signature `signed` reads at `operands`, each type it read: its draft filled in the order
/// read and interned, a `SIG`'s declared and a module's its own. Where an operand still reads a run-bound name, the signature holds it.
pub fn sealed_signature(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    signed: &Signed<'_>,
    operands: &[DeclaredType<Parametric>],
) -> KType {
    let mut draft = SchemaDraft::new(scratch);
    if signed.declared {
        draft.origin = SigOrigin::Declared;
    }
    let mut read = operands.iter().copied();
    let mut next = || read.next().expect("an entry reads its operand");
    for entry in signed.entries {
        match *entry {
            Entry::Parameter(name) => match next() {
                DeclaredType::Type(parameter) => draft.insert_parameter(name, parameter),
                DeclaredType::Scheme(_) => unreachable!("a head parameter is a type"),
            },
            Entry::Manifest(name) => match next() {
                DeclaredType::Type(ty) => draft.insert_manifest(name, ty),
                DeclaredType::Scheme(_) => unreachable!("a manifest member is a type"),
            },
            Entry::Value(name) => draft.insert_value_slot(name, next()),
            Entry::Keyworded => draft.push_keyworded(next()),
            Entry::Group(members, mode) => draft.push_operator_group(members, mode),
        }
    }
    types.signature(scratch, draft)
}
