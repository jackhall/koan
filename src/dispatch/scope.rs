use std::collections::HashMap;

use crate::parse::kexpression::KExpression;

use super::kfunction::{ArgumentBundle, KFunction};
use super::kobject::KObject;

pub struct KFuture<'a> {
    pub parsed: KExpression,
    pub function: &'a KFunction<'a>,
    pub bundle: ArgumentBundle<'a>,
}

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

    pub fn lookup(&self, expr: &KExpression) -> Option<&'a KFunction<'a>> {
        self.functions
            .iter()
            .find(|f| f.signature.matches(expr))
            .copied()
    }

    pub fn dispatch(&'a self, expr: KExpression) -> Result<KFuture<'a>, String> {
        let function = self
            .lookup(&expr)
            .ok_or_else(|| "no matching function".to_string())?;
        function.bind(expr)
    }
}
