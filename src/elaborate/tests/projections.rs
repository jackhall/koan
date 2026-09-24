//! A `.` projection in type position: a record field's declared type, read through its newtype
//! layers and chained.

use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;

use super::super::{Elaboration, callable_type};
use super::{brought, declared};

#[test]
fn a_record_field_names_its_declared_type() {
    brought(
        "NEWTYPE Point = :{x :Number, y :Str}\nLET Label = :(Point.y)\nLET Alias = Point\n\
         LET Through = :(Alias.x)\nLET Plain = :{x :Number, y :Str}\nLET Direct = :(Plain.y)",
        |program| {
            assert_eq!(program.bound("Label"), KType::STR);
            assert_eq!(program.bound("Through"), KType::NUMBER);
            assert_eq!(program.bound("Direct"), KType::STR);
        },
    );
}

#[test]
fn a_field_read_chains_where_a_field_is_record_shaped() {
    brought(
        "NEWTYPE Inner = :{x :Number}\nNEWTYPE Outer = :{inner :Inner, bare :{y :Str}}\n\
         UNION Shape = (Circle :{r :Number} Square :{side :Number})\n\
         LET Nominal = :(Outer.inner.x)\nLET Structural = :(Outer.bare.y)\n\
         LET Radius = :(Shape.Circle.r)",
        |program| {
            assert_eq!(program.bound("Nominal"), KType::NUMBER);
            assert_eq!(program.bound("Structural"), KType::STR);
            assert_eq!(program.bound("Radius"), KType::NUMBER);
        },
    );
}

#[test]
fn a_field_read_falls_through_every_newtype_layer() {
    brought(
        "NEWTYPE Point = :{x :Number, y :Str}\nNEWTYPE Boxed = Point\nLET Deep = :(Boxed.x)",
        |program| assert_eq!(program.bound("Deep"), KType::NUMBER),
    );
}

#[test]
fn a_field_the_type_does_not_declare_is_refused() {
    let cases: [(&str, Option<&str>); 4] = [
        (
            "NEWTYPE Point = :{x :Number}\nLET Missing = :(Point.z)",
            Some("Point"),
        ),
        (
            "NEWTYPE Inner = :{x :Number}\nNEWTYPE Outer = :{inner :Inner}\n\
             LET Missing = :(Outer.inner.z)",
            Some("Inner"),
        ),
        ("LET Missing = :(Number.z)", None),
        // A ring of newtypes with no record under it: the read must terminate.
        ("NEWTYPE Loop = Loop\nLET Missing = :(Loop.z)", Some("Loop")),
    ];
    for (source, owner) in cases {
        declared(source, |program, brought| {
            let expected = owner.map_or(KType::NUMBER, |owner| program.bound(owner));
            let z = BinderSymbol::classify("z").unwrap().symbol();
            assert!(
                matches!(brought, Err(Elaboration::NoSuchMember { owner, name }) if owner == expected && name == z),
                "`{source}` refuses the field: {brought:?}"
            );
        });
    }
}

#[test]
fn a_slot_typed_by_a_field_reads_its_declared_type() {
    brought(
        "NEWTYPE Point = :{x :Number, y :Str}\nLET label = FN EXPR (LABEL v :(Point.y)) -> Str = (v)",
        |program| {
            let (types, scratch) = (program.types, program.scratch);
            let form = program
                .birth("label")
                .form()
                .expect("a callable body sits in a form");
            let callable = callable_type(form, program.activation, types, scratch)
                .expect("the definition elaborates");
            let v = BinderSymbol::classify("v").unwrap();
            assert_eq!(
                callable.ktype,
                types
                    .function_type(scratch, &[], &[(v, KType::STR)], KType::STR)
                    .handle
            );
        },
    );
}
