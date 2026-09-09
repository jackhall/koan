//! Compact one-line rendering of a tree, for tests and diagnostics. Layout groups print as
//! `L(...)`, bracketed groups with their own delimiters, strings with their quotes, and a glued
//! item is followed by `~` instead of a space.

use crate::{Item, Kind, Node};

pub fn render(items: &[Item<'_>]) -> String {
    let mut out = String::new();
    render_items(items, &mut out);
    out
}

fn render_items(items: &[Item<'_>], out: &mut String) {
    for (i, item) in items.iter().enumerate() {
        render_item(item, out);
        if i + 1 < items.len() {
            out.push(if item.glued { '~' } else { ' ' });
        }
    }
}

fn render_item(item: &Item<'_>, out: &mut String) {
    match &item.node {
        Node::Atom(text) => out.push_str(text),
        Node::Str { quote, body } => {
            out.push(*quote);
            out.push_str(body);
            out.push(*quote);
        }
        Node::Comma => out.push(','),
        Node::Group { kind, items } => {
            let (open, close) = kind.delimiters().unwrap_or(('(', ')'));
            if *kind == Kind::Layout {
                out.push('L');
            }
            out.push(open);
            render_items(items, out);
            out.push(close);
        }
    }
}
