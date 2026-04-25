#![allow(dead_code)]

mod parse;
mod dispatch;

/// CLI entry point; currently just evaluates a hardcoded greeting through the stub `eval`.
fn main() {
    println!("{}", eval("Hello, world!").value);
}


/// Placeholder generic value wrapper; stands in for whatever `eval` will eventually return.
struct Object<T> {
    value: T
}


/// Stub evaluator: wraps the input expression in an `Object` without parsing or executing it.
fn eval(expr: &str) -> Object<&str> {
    return Object {value: expr};
}
