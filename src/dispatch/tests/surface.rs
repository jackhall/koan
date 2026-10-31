//! A value's type is its surface: every read of a container or tagged value — printing, `==`, a
//! field read, `FROM`, a `USING` source — sees only what the value's carried type names, at the
//! type it names it, at every depth.

use crate::program::tests::weight_back;
use crate::program::{CellSubstrate, Outcome};

use super::super::Koan;
use super::{output, run};
use crate::values::BuildBudget;

/// A function handing back its argument at `Any`, so the load no longer knows the argument's type
/// and a read the load would refuse reaches the operator run.
const HIDE: &str = "EXPR #(HIDE v :Any) -> Any = #(v)\n";

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
             NEWTYPE (Type -> Boxed)\n\
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

/// An element is read at the type its list names, which fills each record with the fields another
/// names: a literal keeps every record's fields, and reads a missing one as `null`.
#[test]
fn a_literal_element_keeps_its_fields_and_reads_a_missing_one_as_null() {
    assert_eq!(
        run("PRINT [{x = 1, y = 2}, {x = 3}]\n\
             PRINT ([{x = 1, y = 2}, {x = 3}] == [{x = 1, y = 9}, {x = 3}])\n\
             PRINT ([{x = 1, y = 2}, {x = 3}] == [{x = 1, y = 2}, {x = 3, y = null}])"),
        "[{x = 1, y = 2}, {x = 3, y = null}]\nfalse\ntrue"
    );
}

#[test]
fn a_field_read_sees_only_the_fields_the_type_names() {
    assert_eq!(
        run(&format!(
            "{HIDE}LET r = ({{x = 1, y = \"a\"}} :! :{{x :Number}})\n\
             PRINT (ATTR (HIDE r) y)"
        )),
        "error: :{x :Number} has no field y"
    );
    assert_eq!(
        run(&format!(
            "{HIDE}LET n = ({{p = {{x = 1, y = 2}}}} :! :{{p :{{x :Number}}}})\n\
             PRINT n.p\n\
             PRINT (ATTR (HIDE n.p) y)"
        )),
        "{x = 1}\nerror: :{x :Number} has no field y"
    );
}

#[test]
fn a_field_read_through_a_tagged_value_sees_its_representation() {
    assert_eq!(
        run("NEWTYPE Point = :{x :Number, y :Number}\n\
             LET p = (Point {x = 1, y = 2, z = 3})\n\
             PRINT p.x\n\
             PRINT p.z"),
        "1\nerror: Point has no field z"
    );
    assert_eq!(
        run("NEWTYPE (Type -> Boxed)\n\
             LET b = ((Boxed {x = 1, y = 2}) :! (Boxed {Type = :{x :Number}}))\n\
             PRINT b.y"),
        "error: :(Boxed {Type = :{x :Number}}) has no field y"
    );
}

#[test]
fn a_projection_reads_only_the_fields_the_type_names() {
    assert_eq!(
        run(&format!(
            "{HIDE}LET r = ({{x = 1, y = \"a\"}} :! :{{x :Number}})\n\
             PRINT (#[y] FROM (HIDE r))"
        )),
        "error: :{x :Number} has no field y"
    );
    assert_eq!(
        run("LET r = {x = 1, y = 2}\nPRINT (#[x y x] FROM r)"),
        "{x = 1, y = 2}"
    );
}

/// A projection is a restamp: it shares its record's runs, so it weighs what the record does.
#[test]
fn a_projection_shares_its_records_runs() {
    let source = "LET r = {x = 1, y = \"a\"}\nLET p = (#[x] FROM r)";
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output()).unwrap();
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Completed)
    );
    assert_eq!(
        weight_back(&mut substrate, "p"),
        weight_back(&mut substrate, "r")
    );
}

/// Every list and dict fills its records at each position, built at the run or written as a
/// literal, through nested lists, dicts and records; one ascribed a narrower element type reads
/// each element at it.
#[test]
fn every_container_fills_its_records_at_each_position() {
    assert_eq!(
        run("LET a = {x = 1, y = 2}\nLET b = {x = 3}\nPRINT [a, b]"),
        "[{x = 1, y = 2}, {x = 3, y = null}]"
    );
    assert_eq!(
        run("PRINT [{p = {x = 1, y = 2}}, {p = {x = 3}}]"),
        "[{p = {x = 1, y = 2}}, {p = {x = 3, y = null}}]"
    );
    assert_eq!(
        run("PRINT [[{x = 1, y = 2}], [{x = 3}]]"),
        "[[{x = 1, y = 2}], [{x = 3, y = null}]]"
    );
    assert_eq!(
        run("PRINT {\"a\": {x = 1, y = 2}, \"b\": {x = 3}}"),
        "{\"a\": {x = 1, y = 2}, \"b\": {x = 3, y = null}}"
    );
    // A knot's list reads a fellow member at its filled type, and the member alone at its own.
    assert_eq!(
        run("NEWTYPE Ring = :{next :Any}\n\
             LET a = [b, {m = 1}]\n\
             LET b = {n = (Ring {next = a})}\n\
             PRINT a\nPRINT b"),
        "@0 = [{m = null, n = Ring({next = @0})}, {m = 1, n = null}]\n\
         {n = @0 = Ring({next = [{m = null, n = @0}, {m = 1, n = null}]})}"
    );
    // A literal whose elements the load does not know exactly fills at the run.
    assert_eq!(
        run("EXPR #(PICK u :Any) -> Any = #([u, {x = 5}])\n\
             PRINT (PICK {x = 1, y = 2})\nPRINT (PICK 3)"),
        "[{x = 1, y = 2}, {x = 5, y = null}]\n[3, {x = 5}]"
    );
    assert_eq!(
        run("PRINT ([{x = 1, y = 2}, {x = 3}] :! :(LIST OF :{x :Number}))"),
        "[{x = 1}, {x = 3}]"
    );
}

/// Reading a field a record's type does not name is refused at the load, filled lists or not.
#[test]
fn a_field_no_type_names_is_refused_at_the_load() {
    assert_eq!(
        run("LET r = {x = 1, y = 2}\nPRINT (ATTR r z)"),
        "load: <test>:2:7: :{x :Number y :Number} has no field z"
    );
}

/// No record shows a value it stores under a field the type it is held at hides, whatever type
/// later names that field or would show it: building a container whose fill names it, passing it
/// to a wider parameter, retyping it wider, or reading a field through a widened holder.
#[test]
fn a_hidden_field_never_shows_again() {
    let xs = "LET xs = ([{x = 1, y = 3}] :! :(LIST OF :{x :Number}))\n";
    assert_eq!(
        run(&format!(
            "{xs}LET zs = [xs, [{{x = 2, y = 4}}]]\nPRINT zs\n\
             PRINT (zs :! :(LIST OF (LIST OF (:{{x :Number, y :Number}} | :{{x :Number, y :Null}}))))"
        )),
        "[[{x = 1, y = null}], [{x = 2, y = 4}]]\n[[{x = 1, y = null}], [{x = 2, y = 4}]]"
    );
    assert_eq!(
        run("LET h = ({p = {x = 2, f = 3}} :! :{p :{x :Number}})\n\
             PRINT [h, {p = {x = 1, f = null}}]"),
        "[{p = {f = null, x = 2}}, {p = {f = null, x = 1}}]"
    );
    let gh = "LET xs = ([{g = 2, h = 1}] :! :(LIST OF :{h :Number}))\n";
    assert_eq!(
        run(&format!(
            "{gh}LET f = (FN :{{ys :(LIST OF (:{{g :Number}} | :{{h :Number}}))}} -> Any = #(PRINT ys))\n\
             f {{ys = xs}}\n\
             PRINT (xs :! :(LIST OF (:{{g :Number}} | :{{h :Number}})))\n\
             PRINT (xs :! :(LIST OF Any))\n\
             PRINT ((xs :! :(LIST OF Any)) == [{{g = 2, h = 1}}])\n\
             LET r = ({{g = 2, h = 1}} :! :{{h :Number}})\n\
             PRINT (r :! Any)"
        )),
        "[{h = 1}]\n[{h = 1}]\n[{h = 1}]\nfalse\n{h = 1}"
    );
    assert_eq!(
        run("LET r = ({p = {g = 2, h = 1}} :! :{p :{h :Number}})\n\
             LET w = (r :! :{p :Any})\n\
             PRINT w.p\n\
             LET inner = w.p\n\
             PRINT (ATTR inner g)"),
        "{h = 1}\nerror: :{h :Number} has no field g"
    );
    // A holder rebuilt over a copy, and a copied record, still hide what their types hide.
    assert_eq!(
        run(
            "LET xs = ([{x = 1, y = 3}, {x = 2, g = 5}] :! :(LIST OF :{x :Number}))\n\
             LET zs = [xs, [{x = 9, g = 4}]]\n\
             PRINT (zs :! :(LIST OF (LIST OF Any)))"
        ),
        "[[{x = 1}, {x = 2}], [{g = 4, x = 9}]]"
    );
    // A holder rebuilt over a copy is typed by what it holds, so no type promises a field it hid.
    assert_eq!(
        run("LET r = {a = {x = 1, y = 2}}\n\
             LET h = ({q = r} :! :{q :{a :{x :Number}}})\n\
             LET t = (h :! :{q :Any}).q\n\
             LET f = (FN :{s :{a :{x :Number, y :Number}}} -> Any = #(PRINT s.a.y))\n\
             f {s = t}"),
        "error: :(FN :{s :{a :{x :Number y :Number}}} -> Any) cannot be called with \
         :{s :{a :{x :Number}}}"
    );
    assert_eq!(
        run(
            "LET h = ({l = [{x = 1, y = 2}]} :! :{l :(LIST OF :{x :Number})})\n\
             LET inner = (h :! :{l :Any}).l\n\
             PRINT (inner :! :(LIST OF :{x :Number, y :Number}))"
        ),
        "error: :(LIST OF :{x :Number}) does not satisfy its ascription \
         :(LIST OF :{x :Number y :Number})"
    );
    assert_eq!(
        run(
            "LET r = ({a = {x = 1, y = 3}, g = 5} :! :{a :{x :Number}})\n\
             LET zs = [r, {a = {x = 2}, g = null}]\n\
             PRINT (zs :! :(LIST OF :{a :Any}))"
        ),
        "[{a = {x = 1}}, {a = {x = 2}}]"
    );
    // A knot's list holds a hiding value at its view under the list's fill.
    assert_eq!(
        run(&format!(
            "NEWTYPE Ring = :{{next :Any}}\n{xs}\
             LET a = [xs, [{{x = 2, y = 4}}], b]\nLET b = (Ring {{next = a}})\nPRINT a"
        )),
        "@0 = [[{x = 1, y = null}], [{x = 2, y = 4}], Ring({next = @0})]"
    );
    // A field a holder hid, filled at `Null` beside a record naming it, is no longer stored.
    assert_eq!(
        run("LET r = ({p = {g = 2, h = 1}} :! :{p :{h :Number}})\n\
             LET l = [r.p, {g = 5, h = 2}]\nPRINT l\nPRINT (l :! :(LIST OF Any))"),
        "[{g = null, h = 1}, {g = 5, h = 2}]\n[{h = 1}, {g = 5, h = 2}]"
    );
}
