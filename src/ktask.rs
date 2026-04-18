use crate::kobject::KObject;
use crate::ktraits::{Parseable, Executable};
use std::collections::HashMap;

pub struct KTask {
    pub base: KObject,
}

impl Parseable for KTask {
    fn equal(&self, other: &dyn Parseable) -> bool { self.base.equal(other) }
    fn summarize(&self) -> String { self.base.summarize() }
}

impl Executable for KTask {
    fn execute(&self, _args: &[&dyn Parseable]) -> Box<dyn Parseable> {
        Box::new(KObject { name: self.base.name.clone(), remaining_args: HashMap::new() })
    }
}
