#![allow(dead_code)]

mod ktraits;
mod kobject;
mod ktask;
mod kloop;
mod klist;
mod kstring;
mod kexpression;
mod monad;
mod parse;

fn main() {
    println!("{}", eval("Hello, world!").value);
}


struct Object<T> {
    value: T
}


fn eval(expr: &str) -> Object<&str> {
    return Object {value: expr};
}
