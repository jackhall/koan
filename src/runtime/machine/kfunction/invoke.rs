use std::rc::Rc;

use crate::ast::{ExpressionPart, KExpression};

use crate::runtime::machine::core::{CallArena, RuntimeArena, Scope};
use crate::runtime::model::values::KObject;

use super::argument_bundle::ArgumentBundle;
use super::body::{Body, BodyResult};
use super::scheduler_handle::SchedulerHandle;
use super::KFunction;

impl<'a> KFunction<'a> {
    /// Run this function's body for an already-bound call. Builtins call straight through;
    /// user-defined functions allocate a per-call child scope, bind parameters into it,
    /// substitute parameter Identifiers in a body clone with `Future(value)`, and return a
    /// tail-call so the caller's slot is rewritten in place.
    ///
    /// The child scope and substitution are complementary: substitution covers parameter
    /// references in typed-slot positions (`(PRINT x)` needs `x` as a `Future(KString)`),
    /// the child scope covers Identifier-slot lookups (`(x)` parens-wrapped) and is the
    /// substrate for closure capture.
    pub fn invoke(
        &'a self,
        scope: &'a Scope<'a>,
        sched: &mut dyn SchedulerHandle<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        match &self.body {
            Body::Builtin(f) => f(scope, sched, bundle),
            Body::UserDefined(expr) => {
                // Per-call frame whose arena owns the child scope, parameter clones, and
                // substituted-body allocations. `outer` is the FN's captured definition
                // scope (lexical scoping). Closure escapes whose captured scope lives in a
                // per-call arena are kept alive externally via the lifted
                // `KFunction(&fn, Some(Rc))` on the user-bound value.
                let outer = self.captured_scope();
                let frame: Rc<CallArena> = CallArena::new(outer, None);
                // SAFETY: heap-pinning makes `arena_ptr` and `scope_ptr` valid for the
                // box's life; allocations into the arena live until `frame` drops.
                let arena_ptr: *const RuntimeArena = frame.arena();
                let scope_ptr: *const Scope<'_> = frame.scope();
                let inner_arena: &'a RuntimeArena = unsafe { &*(arena_ptr as *const _) };
                let child: &'a Scope<'a> = unsafe { &*(scope_ptr as *const _) };
                for (name, rc) in bundle.args.iter() {
                    let cloned = rc.deep_clone();
                    let allocated = inner_arena.alloc_object(cloned);
                    // The signature parser enforces parameter-name uniqueness upstream, so
                    // `bind_value`'s rebind error here would indicate a signature-parser
                    // invariant break rather than a recoverable case.
                    let _ = child.bind_value(name.clone(), allocated);
                }
                let substituted = substitute_params(expr.clone(), &bundle, inner_arena);
                BodyResult::tail_with_frame(substituted, frame, self)
            }
        }
    }
}

/// Replace every `Identifier(name)` in `expr` whose name is in `bundle.args` with a
/// `Future(value)` allocated in `arena`. Recurses into nested `Expression`, `ListLiteral`,
/// and `DictLiteral` parts; other parts pass through unchanged.
pub(crate) fn substitute_params<'a>(
    expr: KExpression<'a>,
    bundle: &ArgumentBundle<'a>,
    arena: &'a RuntimeArena,
) -> KExpression<'a> {
    KExpression {
        parts: expr
            .parts
            .into_iter()
            .map(|p| substitute_part(p, bundle, arena))
            .collect(),
    }
}

fn substitute_part<'a>(
    part: ExpressionPart<'a>,
    bundle: &ArgumentBundle<'a>,
    arena: &'a RuntimeArena,
) -> ExpressionPart<'a> {
    match part {
        ExpressionPart::Identifier(name) => match bundle.get(&name) {
            Some(value) => {
                let allocated: &'a KObject<'a> = arena.alloc_object(value.deep_clone());
                ExpressionPart::Future(allocated)
            }
            None => ExpressionPart::Identifier(name),
        },
        ExpressionPart::Expression(boxed) => {
            ExpressionPart::Expression(Box::new(substitute_params(*boxed, bundle, arena)))
        }
        ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(
            items
                .into_iter()
                .map(|p| substitute_part(p, bundle, arena))
                .collect(),
        ),
        ExpressionPart::DictLiteral(pairs) => ExpressionPart::DictLiteral(
            pairs
                .into_iter()
                .map(|(k, v)| {
                    (
                        substitute_part(k, bundle, arena),
                        substitute_part(v, bundle, arena),
                    )
                })
                .collect(),
        ),
        other => other,
    }
}
