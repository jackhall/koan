//! `TYPE OF <value>` — the value → type introspection door. Yields the type the value reports for
//! itself ([`KObject::ktype`]) as an ordinary type value, so `TYPE OF 5` is `Number` and
//! `TYPE OF xs` is `LIST OF Number`. Applied to a module it yields that module's principal
//! signature, which is how a module reaches type position: a module name is a value token, so it
//! names no type on its own (see
//! [design/typing/modules.md](../../../design/typing/modules.md)).

use crate::machine::model::Held;
use crate::machine::{arg_held, Action, BodyCtx};
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
        Some(Held::UnresolvedType(ti)) => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`TYPE OF` takes a value; `{}` is already a type",
                ti.render(),
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
    // The type a value reports for itself is owned data — a module's self-sig included — so it
    // seals with an empty reach and allocates into this step's own region.
    Action::Done(Ok(ctx.ctx.alloc_type(value.ktype())))
}

#[cfg(test)]
mod tests;
