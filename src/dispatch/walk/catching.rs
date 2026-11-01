//! The walk's rules for `TRY` and `CATCH`: each body is a block typed where it is written.
//!
//! A `TRY` is its body's static type and its kept arms' spanned, as a `MATCH`'s arms are
//! ([`spanned`]): its arms are read as a `MATCH … UNDER Error`'s over a scrutinee of `Error`, and a
//! label naming no variant of `Error` refuses the load. A `TRY` need not name every kind: a fault no
//! arm takes passes on. A `CATCH` is `Result {Ok = <body>, Error = Error}`, at its body's upper end
//! over `Never`, as a read's `Result` is.
//!
//! See [README.md § Static types](../README.md#static-types).

use crate::elaborate::{applied_union, arm_shapes, variant_named};
use crate::memory::BumpVec;
use crate::parse::KExpression;
use crate::scope::{Guard, ShapeError, Store};
use crate::type_lattice::{Interval, KType};
use crate::values::unknown;

use super::super::builtins::{error_type, result_type};
use super::arms::spanned;
use super::lookups::variant;
use super::{Entering, Pass};

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// `TRY <body> WITH <arms>`: the body typed as a block, each arm as a `MATCH … UNDER Error`'s,
    /// and the two spanned.
    pub(super) fn tried(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let body = self.caught_body(level, node)?;
        let error = error_type(self.builtins);
        let shape = self.chain[level].shape;
        for arm in arm_shapes(shape, node) {
            if let Some(Guard::Label(label)) = arm.arm().map(|arm| arm.guard)
                && variant_named(types, error, label).is_none()
            {
                return Err(ShapeError::NoVariant {
                    label,
                    union: error,
                    at: node.source,
                });
            }
        }
        let scrutinee = Interval {
            lower: KType::NEVER.into(),
            upper: error.into(),
        };
        let arms = self.armed(
            level,
            node,
            scrutinee,
            Some(error.into()),
            Some(error),
            true,
        )?;
        Ok(spanned(types, scratch, &[body, arms]))
    }

    /// `CATCH <body>`: `Result` applied at the body's upper end and `Error`, over `Never`.
    pub(super) fn caught(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let body = self.caught_body(level, node)?;
        let error = error_type(self.builtins);
        let mut arguments = BumpVec::with_capacity_in(2, scratch);
        arguments.extend([
            (variant("Ok"), body.upper),
            (variant("Error"), error.into()),
        ]);
        arguments.sort_unstable_by_key(|(name, _)| *name);
        Ok(Interval {
            lower: KType::NEVER.into(),
            upper: applied_union(types, scratch, result_type(self.builtins), &arguments),
        })
    }

    /// The `TRY` or `CATCH` `node`'s body, typed as a block.
    fn caught_body(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
    ) -> Result<Interval, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let Some(body) = shape.site_of_body(node).and_then(|site| shape.nested(site)) else {
            return Ok(unknown());
        };
        Ok(self
            .block(level, body, Entering::Plain)?
            .unwrap_or_else(unknown))
    }
}
