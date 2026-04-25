use std::collections::HashMap;

use crate::parse::kexpression::KExpression;

use super::kfunction::{ArgumentBundle, KFunction};
use super::kobject::KObject;

/// A function call that has been resolved but not yet executed: the original parsed expression,
/// the chosen `KFunction`, and the `ArgumentBundle` produced by `KFunction::bind`. Carried
/// inside `KObject::KTask` and is the unit of deferred work in the dispatch system.
pub struct KFuture<'a> {
    pub parsed: KExpression,
    pub function: &'a KFunction<'a>,
    pub bundle: ArgumentBundle<'a>,
}

/// Lexical environment: a parent-scope link plus name → value bindings, with functions also
/// indexed in `functions` so `dispatch` can scan them by signature without rewalking `data`.
pub struct Scope<'a> {
    pub outer: Option<&'a Scope<'a>>,
    pub data: HashMap<String, &'a KObject<'a>>,
    pub functions: Vec<&'a KFunction<'a>>,
}

impl<'a> Scope<'a> {
    pub fn add(&mut self, name: String, obj: &'a KObject<'a>) {
        if let KObject::KFunction(f) = obj {
            self.functions.push(*f);
        }
        self.data.insert(name, obj);
    }

    pub fn dispatch(&'a self, expr: KExpression) -> Result<KFuture<'a>, String> {
        let function = self
            .functions
            .iter()
            .find(|f| f.signature.matches(&expr))
            .copied()
            .ok_or_else(|| "no matching function".to_string())?;
        function.bind(expr)
    }
}
