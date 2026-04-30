use std::collections::HashMap;

use crate::parse::kexpression::KExpression;

use super::kfunction::{ArgumentBundle, KFunction};
use super::kobject::KObject;

/// A function call that has been resolved but not yet executed: the original parsed expression,
/// the chosen `KFunction`, and the `ArgumentBundle` produced by `KFunction::bind`. Carried
/// inside `KObject::KTask` and is the unit of deferred work in the dispatch system.
pub struct KFuture<'a> {
    pub parsed: KExpression<'a>,
    pub function: &'a KFunction<'a>,
    pub bundle: ArgumentBundle<'a>,
}

/// Lexical environment: a parent-scope link plus name → value bindings, with functions also
/// indexed in `functions` so `dispatch` can scan them by signature without rewalking `data`.
/// `out` is the sink used by builtins like `PRINT` — pluggable so tests and embedders can
/// capture program output instead of going to stdout.
pub struct Scope<'a> {
    pub outer: Option<&'a Scope<'a>>,
    pub data: HashMap<String, &'a KObject<'a>>,
    pub functions: Vec<&'a KFunction<'a>>,
    pub out: Box<dyn std::io::Write + 'a>,
}

impl<'a> Scope<'a> {
    pub fn add(&mut self, name: String, obj: &'a KObject<'a>) {
        if let KObject::KFunction(f) = obj {
            self.functions.push(*f);
        }
        self.data.insert(name, obj);
    }

    /// Look up `name` in this scope, walking the `outer` chain on miss. Returns the bound
    /// `KObject` from the nearest enclosing scope, or `None` if unbound at every level.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        if let Some(obj) = self.data.get(name).copied() {
            return Some(obj);
        }
        self.outer.and_then(|outer| outer.lookup(name))
    }

    /// Resolve `expr` against this scope's registered functions, walking the `outer` chain on
    /// miss so child scopes inherit builtins (and any user-defined functions) from their
    /// parents. Returns a bound `KFuture` ready to run, or an error if no signature matches at
    /// any level.
    pub fn dispatch(&self, expr: KExpression<'a>) -> Result<KFuture<'a>, String> {
        if let Some(f) = self
            .functions
            .iter()
            .find(|f| f.signature.matches(&expr))
            .copied()
        {
            return f.bind(expr);
        }
        if let Some(outer) = self.outer {
            return outer.dispatch(expr);
        }
        Err("no matching function".to_string())
    }
}

#[cfg(test)]
impl<'a> Scope<'a> {
    /// Test-only constructor: a root scope with no bindings and a swallowing writer. Generic
    /// over `'a` so callers can chain it under a stack-borrowed outer scope without fighting
    /// `'static`.
    pub fn test_sink() -> Scope<'a> {
        Scope {
            outer: None,
            data: HashMap::new(),
            functions: Vec::new(),
            out: Box::new(std::io::sink()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;
    use crate::dispatch::builtins::default_scope;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn dispatch_walks_outer_chain_to_find_builtin() {
        // Parent owns the LET builtin; child has no functions of its own. Dispatching LET
        // against the child must climb to the parent.
        let outer = default_scope();
        let mut inner = Scope::test_sink();
        inner.outer = Some(&outer);

        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token("x".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };

        assert!(inner.dispatch(expr).is_ok(), "child scope should inherit LET from outer");
    }

    #[test]
    fn dispatch_with_no_outer_and_no_match_errors() {
        let scope = Scope::test_sink();
        let expr = KExpression {
            parts: vec![ExpressionPart::Token("nope".into())],
        };
        assert!(scope.dispatch(expr).is_err());
    }
}
