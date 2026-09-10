//! The run-lifetime state the interpreter owns beside the run's scheduler: the frame adopting the
//! run-root scope, the [`RunRegistries`] every step consults, and the [`RunWriter`] `PRINT` writes
//! to.
//!
//! One [`RunFrame`] per run, held by [`AmbientContext`](super::ambient) and reached only through it,
//! so a verdict recorded or a label interned anywhere in the run is visible everywhere in it and
//! all of it drops together at run teardown. A per-call [`CallFrame`] owns none of this — it is a
//! region shell and nothing more, which is why the run's lookup state lives here rather than as two
//! `Option` fields every per-call frame carries `None` in.
//!
//! See [per-call-region/frames.md](../../../design/per-call-region/frames.md).

use std::cell::RefCell;
use std::rc::Rc;

use crate::machine::core::CallFrame;
use crate::machine::core::Scope;
use crate::machine::model::{LabelInterner, RunRegistries};

/// **The run's output sink** — where `PRINT` writes. One per run, held by the [`RunFrame`] beside
/// the run's [`RunRegistries`] and reached the same way: through the execution context, never off a
/// scope. `RefCell` because writing is a `&self` act on a value the whole run shares, and nothing
/// holds the borrow across a call.
///
/// Write errors are dropped: `PRINT` is a statement with no error channel, so there is nothing for a
/// caller to do with one. This is a stopgap — see
/// [monadic side effects](../../../roadmap/foundation/monadic-side-effects.md), which replaces
/// direct writer plumbing with an effect the language expresses.
pub struct RunWriter(RefCell<Box<dyn std::io::Write>>);

impl RunWriter {
    /// Wrap the caller-supplied sink. `'static` is what every entry point already passes — stdout, a
    /// sink, an `Rc`-shared buffer — and it is what lets the writer rest on state that names no
    /// region.
    pub fn new(out: Box<dyn std::io::Write>) -> Self {
        RunWriter(RefCell::new(out))
    }

    /// Write `bytes` to the run's sink, dropping any write error.
    pub fn write_out(&self, bytes: &[u8]) {
        let _ = self.0.borrow_mut().write_all(bytes);
    }
}

/// The run's own frame: the non-dying [`CallFrame`] adopting the run-root scope, plus the two pieces
/// of run-lifetime state nothing else may own.
///
/// The frame is here rather than the state being on the frame because the ownership runs that way:
/// a top-level slot carries this `CallFrame` as its cart exactly as a call carries its own, and the
/// registries and the writer are the *run's*, with one home and no per-call counterpart.
pub(in crate::machine::execute) struct RunFrame {
    /// The frame adopting the run-root scope — a top-level submission's cart, so the root
    /// re-projects from it as `Yoked` rather than anchoring at `'run`.
    frame: Rc<CallFrame>,
    /// The run's lookup state — the type registry and the label interner. Owned outright, not
    /// `Rc`-shared: nothing needs shared ownership, and the heap tables inside must `Drop`.
    registries: RunRegistries,
    /// The run's output sink, with the same home and the same reach path as
    /// [`registries`](Self::registries).
    writer: RunWriter,
}

impl RunFrame {
    /// Adopt `scope` as the run root and take the run's two owned pieces of state: `out`, the
    /// output sink, and `labels`, the interner parse populated.
    pub(in crate::machine::execute) fn adopting<'a>(
        scope: &'a Scope<'a>,
        out: Box<dyn std::io::Write>,
        labels: LabelInterner,
    ) -> RunFrame {
        RunFrame {
            frame: scope.adopt_as_run_frame(),
            registries: RunRegistries::with_labels(labels),
            writer: RunWriter::new(out),
        }
    }

    /// The run-root frame itself.
    pub(in crate::machine::execute) fn frame(&self) -> &Rc<CallFrame> {
        &self.frame
    }

    /// The run's type registry and label interner.
    pub(in crate::machine::execute) fn registries(&self) -> &RunRegistries {
        &self.registries
    }

    /// The run's output sink, handed to a builtin body as `BodyCtx::out`.
    pub(in crate::machine::execute) fn writer(&self) -> &RunWriter {
        &self.writer
    }
}
