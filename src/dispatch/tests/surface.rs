//! A value's type is its surface: every read of a container or tagged value — printing, `==`, a
//! field read, `FROM`, a `USING` source — sees only what the value's carried type names, at the
//! type it names it, at every depth.

use super::run;

#[test]
fn a_retyped_record_prints_and_compares_at_its_type() {
    assert_eq!(
        run("LET r = ({x = 1, y = \"a\"} :! :{x :Number})\n\
             PRINT r\n\
             PRINT (r == {x = 1})"),
        "{x = 1}\ntrue"
    );
}

#[test]
fn a_part_is_read_at_the_type_its_container_names_it() {
    assert_eq!(
        run("LET l = ([{x = 1, y = 2}] :! (LIST OF :{x :Number}))\n\
             PRINT l\n\
             PRINT (l == [{x = 1}])\n\
             LET d = ({\"k\": {x = 1, y = 2}} :! (MAP Str -> :{x :Number}))\n\
             PRINT d\n\
             LET n = ({p = {x = 1, y = 2}} :! :{p :{x :Number}})\n\
             PRINT n"),
        "[{x = 1}]\ntrue\n{\"k\": {x = 1}}\n{p = {x = 1}}"
    );
}

#[test]
fn a_payload_is_read_at_its_identitys_representation() {
    assert_eq!(
        run("NEWTYPE Point = :{x :Number, y :Number}\n\
             LET p = (Point {x = 1, y = 2, z = 3})\n\
             PRINT p\n\
             PRINT (p == (Point {x = 1, y = 2}))\n\
             NEWTYPE (Type AS Boxed)\n\
             LET b = ((Boxed {x = 1, y = 2}) :! (Boxed {Type = :{x :Number}}))\n\
             PRINT b"),
        "Point({x = 1, y = 2})\ntrue\n:(Boxed {Type = :{x :Number}})({x = 1})"
    );
}

/// One data node reached at two seen types shows two surfaces, and the marks tell them apart: the
/// narrow one is no cycle, the full one labels its return.
#[test]
fn one_node_seen_at_two_types_renders_at_each() {
    let program = |views: &str| {
        format!(
            "NEWTYPE Wrap = Any\n\
             LET r = {{w = (Wrap r), v = 1}}\n\
             LET two = ({{p = r, q = r}} :! {views})\n\
             PRINT two"
        )
    };
    assert_eq!(
        run(&program(":{p :{v :Number}, q :{w :Wrap, v :Number}}")),
        "{p = {v = 1}, q = @0 = {v = 1, w = Wrap(@0)}}"
    );
    assert_eq!(
        run(&program(":{p :{w :Wrap, v :Number}, q :{v :Number}}")),
        "{p = @0 = {v = 1, w = Wrap(@0)}, q = {v = 1}}"
    );
}

/// A pair of nodes entered at one pair of seen types is not taken as entered at another, so a
/// field the narrow view hides still decides the wide one.
#[test]
fn one_node_seen_at_two_types_compares_at_each() {
    let program = |views: &str| {
        format!(
            "NEWTYPE Wrap = Any\n\
             LET a = {{w = (Wrap a), v = 1, u = 1}}\n\
             LET b = {{w = (Wrap b), v = 1, u = 2}}\n\
             LET l = ({{p = a, q = a}} :! {views})\n\
             LET m = ({{p = b, q = b}} :! {views})\n\
             PRINT (l == m)"
        )
    };
    assert_eq!(
        run(&program(
            ":{p :{v :Number}, q :{w :Wrap, v :Number, u :Number}}"
        )),
        "false"
    );
    assert_eq!(
        run(&program(
            ":{p :{w :Wrap, v :Number, u :Number}, q :{v :Number}}"
        )),
        "false"
    );
}

/// An element is read at the type its list names, so a literal whose join widens a record hides
/// that record's extra fields. [A container literal's element
/// type](../../../roadmap/gradual-typing/container-literal-types.md) changes this.
#[test]
fn a_widened_literal_element_hides_its_extra_fields() {
    assert_eq!(
        run("PRINT [{x = 1, y = 2}, {x = 3}]\n\
             PRINT ([{x = 1, y = 2}, {x = 3}] == [{x = 1, y = 9}, {x = 3}])"),
        "[{x = 1}, {x = 3}]\ntrue"
    );
}
