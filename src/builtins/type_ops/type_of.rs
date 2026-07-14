//! `TYPE OF <value>` — the value → type introspection door. Yields the type the value reports for
//! itself ([`KObject::ktype`]) as an ordinary type value, so `TYPE OF 5` is `Number` and
//! `TYPE OF xs` is `LIST OF Number`. Applied to a module it yields that module's principal
//! signature, which is how a module reaches type position: a module name is a value token, so it
//! names no type on its own (see
//! [design/typing/modules.md](../../../design/typing/modules.md)).

use crate::machine::core::kfunction::action::{arg_held, Action, BodyCtx};
use crate::machine::model::values::Held;
use crate::machine::{KError, KErrorKind};

pub(super) fn body<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    let value = match arg_held(ctx.args, "value") {
        Some(Held::Object(o)) => o,
        // The `Any` slot admits both channels, so a type argument reaches the body rather than
        // falling through dispatch; a type's own type is not a question this language asks.
        Some(Held::Type(t)) => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`TYPE OF` takes a value; `{}` is already a type",
                t.name(),
            )))))
        }
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("value".into())))),
    };
    if value.is_unstamped_empty_container() {
        return Action::Done(Err(KError::new(KErrorKind::ShapeError(
            "`TYPE OF` an empty, unstamped container: its element type is unknowable — ascribe \
             the container first"
                .into(),
        ))));
    }
    match ctx.arg_carrier("value") {
        // The value arrived resolved, so its type may borrow the value's own region — a module's
        // self-sig borrows the module. Home the result under the argument carrier's stored reach.
        Some(dep) => Action::Done(Ok(ctx.ctx.alloc_type_of_value(dep))),
        // No carrier: a literal argument is region-pure, so its type rebuilds owned at `'static`.
        None => match value.ktype().to_static() {
            Some(owned) => Action::Done(Ok(ctx.ctx.alloc_type(owned))),
            None => Action::Done(Err(KError::new(KErrorKind::ShapeError(
                "`TYPE OF`: the value's type reaches a region but the value arrived without a \
                 carrier"
                    .into(),
            )))),
        },
    }
}

#[cfg(test)]
mod tests;
