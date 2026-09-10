//! [`RunRegistries`] — the run's owned bundle of run-lifetime lookup state: the
//! [`TypeRegistry`] and the [`LabelInterner`] beside it.
//!
//! A plain field on the scheduler-owned [`RunFrame`](crate::machine::execute) — no
//! `Rc`, no process-global, no `thread_local!` — reached by reference through the execution
//! context and dropped with the run. It lives on the ordinary heap rather than in region
//! storage: both registries own growing maps that need `Drop`, and regions are Drop-free.
//!
//! The reach rule: `&TypeRegistry` is the currency for pure type-structure questions (subtyping,
//! digests, dispatch — none of which need label text); `&RunRegistries` is the currency for
//! anything that renders text or constructs a record.
//!
//! See [design/label-interning.md](../../../design/label-interning.md).

use super::types::{TypeRegistry, display_label};
use crate::machine::model::{Carried, Held};
use crate::parse::labels::LabelInterner;

/// See the module-level documentation.
pub struct RunRegistries {
    pub types: TypeRegistry,
    pub labels: LabelInterner,
}

impl RunRegistries {
    /// A run's registries with an empty interner — a test fixture standing in for the run frame.
    /// Production enters through [`Self::with_labels`], which adopts the table parse filled.
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        RunRegistries::with_labels(LabelInterner::new())
    }

    /// Adopt an interner the parse boundary already populated. The parser is the run's primary
    /// label-construction site, so the run frame takes over the table parse filled rather than
    /// starting an empty one beside it.
    pub(crate) fn with_labels(labels: LabelInterner) -> Self {
        RunRegistries {
            types: TypeRegistry::new(),
            labels,
        }
    }
}

/// The rendering of a produced value against the registries that resolve its labels — the `Display`
/// view [`RunRegistries::carried_summary`] hands back. Owned by the registries rather than by the
/// cell: every arm's surface form is a label lookup or a type name, both of which live here.
pub struct CarriedSummary<'x, 'a> {
    carried: &'x Carried<'a>,
    registries: &'x RunRegistries,
}

/// [`CarriedSummary`] for an owned cell — the two extra arms a capture slot mints render as their
/// captured symbol's surface text and as the raw record-type expression respectively.
pub struct HeldSummary<'x, 'a> {
    cell: &'x Held<'a>,
    registries: &'x RunRegistries,
}

impl RunRegistries {
    /// Render a produced value: an object's summary, a type's name, or an unlowered name's surface
    /// form. `.to_string()` on the view is the owned-`String` spelling.
    pub fn carried_summary<'x, 'a>(&'x self, carried: &'x Carried<'a>) -> CarriedSummary<'x, 'a> {
        CarriedSummary {
            carried,
            registries: self,
        }
    }

    /// [`Self::carried_summary`] for an owned cell.
    pub fn held_summary<'x, 'a>(&'x self, cell: &'x Held<'a>) -> HeldSummary<'x, 'a> {
        HeldSummary {
            cell,
            registries: self,
        }
    }
}

impl std::fmt::Display for CarriedSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.carried {
            Carried::Object(o) => o.write_summary(f, self.registries),
            Carried::Type(t) => t.write_name(f, self.registries),
            Carried::UnresolvedType(ti) => {
                write!(f, "{}", display_label(ti.symbol(), self.registries))
            }
        }
    }
}

impl std::fmt::Display for HeldSummary<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.cell {
            Held::Object(o) => o.write_summary(f, self.registries),
            Held::Type(t) => t.write_name(f, self.registries),
            Held::UnresolvedType(ti) => {
                write!(f, "{}", display_label(ti.symbol(), self.registries))
            }
            Held::Name(b) => write!(f, "{}", display_label(b.symbol(), self.registries)),
            Held::RecordType(e) => e.write_summary(f, &self.registries.labels),
        }
    }
}
