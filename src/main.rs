#![allow(dead_code)]

mod parse;
mod dispatch;

fn main() {
    println!("{}", eval("Hello, world!").value);
}


struct Object<T> {
    value: T
}


fn eval(expr: &str) -> Object<&str> {
    return Object {value: expr};
}
