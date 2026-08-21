//! [`RunRegistries`] — the run's owned bundle of run-lifetime lookup state: the
//! [`TypeRegistry`] and the [`LabelInterner`] beside it.
//!
//! A plain field on the scheduler-owned run [`CallFrame`](crate::machine::core::CallFrame) — no
//! `Rc`, no process-global, no `thread_local!` — reached by reference through the execution
//! context and dropped with that frame. It lives on the ordinary heap rather than in region
//! storage: both registries own growing maps that need `Drop`, and regions are Drop-free.
//!
//! The reach rule: `&TypeRegistry` is the currency for pure type-structure questions (subtyping,
//! digests, dispatch — none of which need label text); `&RunRegistries` is the currency for
//! anything that renders text or constructs a record.
//!
//! See [design/label-interning.md](../../../design/label-interning.md).

use super::labels::LabelInterner;
use super::types::TypeRegistry;

/// See the module-level documentation.
pub struct RunRegistries {
    pub types: TypeRegistry,
    pub labels: LabelInterner,
}

impl RunRegistries {
    pub(crate) fn new() -> Self {
        RunRegistries {
            types: TypeRegistry::new(),
            labels: LabelInterner::new(),
        }
    }
}
