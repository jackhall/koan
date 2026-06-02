use super::*;

fn hash_of<V: Hash>(r: &Record<V>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    r.hash(&mut h);
    h.finish()
}

fn rec(pairs: &[(&str, i32)]) -> Record<i32> {
    Record::from_pairs(pairs.iter().map(|(k, v)| (k.to_string(), *v)))
}

#[test]
fn iter_preserves_insertion_order() {
    let r = rec(&[("x", 1), ("y", 2), ("z", 3)]);
    let names: Vec<&str> = r.keys().map(String::as_str).collect();
    assert_eq!(names, ["x", "y", "z"]);
}

#[test]
fn eq_is_order_blind() {
    let a = rec(&[("x", 1), ("y", 2)]);
    let b = rec(&[("y", 2), ("x", 1)]);
    assert_eq!(a, b);
}

#[test]
fn eq_distinguishes_values_and_names() {
    assert_ne!(rec(&[("x", 1)]), rec(&[("x", 2)]));
    assert_ne!(rec(&[("x", 1)]), rec(&[("y", 1)]));
    assert_ne!(rec(&[("x", 1)]), rec(&[("x", 1), ("y", 2)]));
}

#[test]
fn hash_agrees_with_eq() {
    let a = rec(&[("x", 1), ("y", 2)]);
    let b = rec(&[("x", 1), ("y", 2)]);
    assert_eq!(a, b);
    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_is_order_independent() {
    let a = rec(&[("x", 1), ("y", 2), ("z", 3)]);
    let b = rec(&[("z", 3), ("x", 1), ("y", 2)]);
    assert_eq!(a, b);
    assert_eq!(hash_of(&a), hash_of(&b));
}

/// `mix` binds name to value before the commutative fold, so swapping which name
/// carries which value changes the hash — a XOR-of-value-hashes fold would not.
#[test]
fn hash_binds_name_to_value() {
    let a = rec(&[("x", 1), ("y", 2)]);
    let b = rec(&[("x", 2), ("y", 1)]);
    assert_ne!(a, b);
    assert_ne!(hash_of(&a), hash_of(&b));
}

#[test]
fn empty_record_is_empty() {
    let r: Record<i32> = Record::new();
    assert!(r.is_empty());
    assert_eq!(r.len(), 0);
    assert_eq!(r, Record::new());
    assert_eq!(hash_of(&r), hash_of(&Record::<i32>::new()));
}

#[test]
fn get_returns_value_by_name() {
    let r = rec(&[("x", 10), ("y", 20)]);
    assert_eq!(r.get("y"), Some(&20));
    assert_eq!(r.get("missing"), None);
    assert_eq!(r.len(), 2);
}

/// Last-wins on a duplicate name: the keys stay unique so `Hash`/`Eq` remain
/// well-defined even though the parser rejects duplicates upstream.
#[test]
fn from_pairs_duplicate_name_is_last_wins() {
    let r = rec(&[("x", 1), ("x", 9)]);
    assert_eq!(r.len(), 1);
    assert_eq!(r.get("x"), Some(&9));
}
