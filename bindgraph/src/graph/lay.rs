//! Laying a text into its graph, step by step, returning the first error in step order.
//!
//! 1. A site name at the root's entry, and (2) a question that reads nothing, are malformed, as
//!    is a declared ask over no name or over a name its question does not read.
//! 3. Each call is checked alone: its arm, its mapped names (bound at its target: the callee's
//!    entry, or an onward call's statement), and on an arm its mentions.
//! 4. Each `AnswersName` edge is checked alone: what it types, its arm and mentions, that the
//!    edges typing one name hang on different arms of one question, and the name it takes.
//! 5. An answered name whose deciding question, call mapping or term on a site reads the name
//!    itself, directly or through other answered names, is malformed. A cycle through what a
//!    callee returns is no such read: it is a knot, which evaluation resolves.
//! 6. **Bases**, a fixpoint over sets of site names that only grow: a site name's base is itself;
//!    an answered name's is the union over its edges of its question's reads' bases (on an arm),
//!    its call's inputs' bases, and its term's mentions' bases (on a site). A call's inputs are
//!    its mentions, its question's reads, and each site name its callee **captures** that the
//!    call's site sees. A body captures the site names bound outside it in the base of a name
//!    some reader inside it waits on.
//! 7. Every base a question reads, or a call or a term on a site mentions, is visible at its
//!    site.
//! 8. The captures order each inner entry among its encloser's statements.
//! 9. Every entry is a `BindingNode`, and so is every statement in some reader's base, a term on
//!    a site among the readers.
//! 10. A question's owner, a call's owner and an answered name's node are each the latest site
//!     in the relevant base, or the root's entry where it is empty.
//! 11. Each body's inherited names, each node's relevant names and borrowed questions. Then each
//!     **onward call**, the latest target first: it passes its mentions and each name its target
//!     reads that is bound before the target, and is owned at the latest of those, strictly
//!     before its target. One whose target is no node is dropped; one owned at its own target, or
//!     binding a statement another binds, is malformed.
//! 12. Each edge is filed on its arm, its call or its node. Each node's handle takes one field
//!     per distinct ask its owned questions declare, in `QuestionId` then declaration order.
//! 13. [`grows`] refuses a closed walk that builds a name over itself; an onward call's mapping
//!     draws arcs into the names it maps, as a term on a site does. Each statement node's chain
//!     of onward calls from its body's entry, or from before it, is recorded for taking.
//! 14. The text components ([`needs`](super::needs)): where a knot can form, the strongly
//!     connected components of the arcs of need that hold a cycle.
//!
//! See [the crate's README](../../README.md#order-and-ownership).

use std::collections::HashMap;

use super::grows::{self, Arc, Through};
use super::{
    AnswerBranch, Answered, AnswersName, Ask, BindingNode, BindsAfter, Chains, Edge, Field, Graph,
    LayError, On, OpenCaller, PassesName, SiteQuestion, Source,
};
use crate::domain::{Facts, Kept, Nesting};
use crate::id::{AnswersId, BodyId, EdgeId, NameId, NodeId, QuestionId, Site};
use crate::text::{AnswersText, EdgeText, Malformed, NameText, OnText, SourceText, Text};

/// A set of names, sorted and without repeats.
type Names = Vec<NameId>;

/// Adds `name` to `into`; whether it was new.
fn insert(into: &mut Names, name: NameId) -> bool {
    match into.binary_search(&name) {
        Ok(_) => false,
        Err(at) => {
            into.insert(at, name);
            true
        }
    }
}

/// Adds every name of `from` to `into`; whether any was new.
fn union_into(into: &mut Names, from: &[NameId]) -> bool {
    let mut changed = false;
    for &name in from {
        changed |= insert(into, name);
    }
    changed
}

/// Adds to `body`'s set, and to each encloser's, each of `names` bound outside that body.
fn mark_outside(
    chains: &Chains,
    sets: &mut [Names],
    site_of: impl Fn(NameId) -> Site,
    body: BodyId,
    names: &[NameId],
) -> bool {
    let mut changed = false;
    let mut at = Some(body);
    while let Some(ancestor) = at {
        for &name in names {
            if !chains.within(ancestor, site_of(name).body()) {
                changed |= insert(&mut sets[ancestor.index()], name);
            }
        }
        at = chains.encloser[ancestor.index()];
    }
    changed
}

impl<K: Kept> Text<K> {
    /// Lays the graph: checks the description, computes each name's base, orders the sites,
    /// picks each question's and call's owner, binds each answered name, files the edges, fixes
    /// each node's handle fields, refuses a recursion that grows, and finds where a knot can form.
    /// Reads no binding.
    pub fn lay<F: Facts<Kept = K>>(self, facts: &F) -> Result<Graph<K>, LayError> {
        let Text {
            bodies,
            names: name_texts,
            questions,
            edges,
            answers,
        } = self;
        let root = BodyId::new(0);
        let encloser: Vec<_> = bodies.iter().map(|body| body.encloser).collect();
        let mut depth = vec![0; bodies.len()];
        for (body, up) in encloser.iter().enumerate() {
            // A body is made after its encloser, so the encloser's depth is already known.
            if let Some(up) = up {
                depth[body] = depth[up.index()] + 1;
            }
        }
        let mut chains = Chains {
            encloser,
            depth,
            captured_upto: vec![None; bodies.len()],
        };
        let site_name = |name: NameId| match name_texts.get(name.index()) {
            Some(NameText::Site(site)) => Some(*site),
            _ => None,
        };

        // 1 and 2.
        for (index, name) in name_texts.iter().enumerate() {
            if *name == NameText::Site(Site::Entry(root)) {
                let name = NameId::new(index);
                return Err(Malformed::RootBindsName { name }.into());
            }
        }
        for (index, question) in questions.iter().enumerate() {
            if question.reads.is_empty() {
                let question = QuestionId::new(index);
                return Err(Malformed::ReadsNothing { question }.into());
            }
            for (names, _) in &question.asks {
                let question_id = QuestionId::new(index);
                if names.is_empty() {
                    let question = question_id;
                    return Err(Malformed::AsksNoName { question }.into());
                }
                if let Some(&name) = names.iter().find(|name| !question.reads.contains(name)) {
                    let question = question_id;
                    return Err(Malformed::AsksUnread { question, name }.into());
                }
            }
        }

        // 3. Calls alone. Each mapped term's mentions are kept for the bases and the arcs.
        let mut mentions: Vec<Vec<Vec<(NameId, Nesting)>>> = Vec::with_capacity(edges.len());
        for (index, edge_text) in edges.iter().enumerate() {
            let edge = EdgeId::new(index);
            let reads = match edge_text.on {
                OnText::Arm { question, arm } => {
                    let question = &questions[question.index()];
                    if arm >= question.arms {
                        let edge = Edge::Passes(edge);
                        return Err(Malformed::ArmOutOfRange { edge }.into());
                    }
                    Some(&question.reads)
                }
                OnText::Site(_) => None,
            };
            let mut per_pair = Vec::with_capacity(edge_text.mapping.len());
            for (at, (name, term)) in edge_text.mapping.iter().enumerate() {
                let name = *name;
                let target = edge_text.target;
                if site_name(name) != Some(target) {
                    return Err(Malformed::MapsUnboundName { edge, name }.into());
                }
                if edge_text.mapping[..at]
                    .iter()
                    .any(|(earlier, _)| *earlier == name)
                {
                    return Err(Malformed::MapsNameTwice { edge, name }.into());
                }
                let mentioned = facts.mentions(*term);
                for &(name, _) in &mentioned {
                    match reads {
                        Some(reads) if !reads.contains(&name) => {
                            let edge = Edge::Passes(edge);
                            return Err(Malformed::MentionsUnread { edge, name }.into());
                        }
                        // A name of no text is visible nowhere.
                        None if name.index() >= name_texts.len() => {
                            let edge = Edge::Passes(edge);
                            return Err(Malformed::MentionsOffChain { edge, name }.into());
                        }
                        _ => {}
                    }
                }
                per_pair.push(mentioned);
            }
            mentions.push(per_pair);
        }

        // 4. `AnswersName` edges alone. Each term on a site's mentions, with how it mentions
        // each, and its names sorted.
        let site_mentions: Vec<Vec<(NameId, Nesting)>> = (answers.iter())
            .map(|answer| match &answer.source {
                SourceText::Site { content, .. } => facts.mentions(*content),
                SourceText::Arm { .. } | SourceText::Call { .. } => Vec::new(),
            })
            .collect();
        let site_names: Vec<Names> = (site_mentions.iter())
            .map(|mentioned| {
                let mut names = Names::new();
                for &(name, _) in mentioned {
                    insert(&mut names, name);
                }
                names
            })
            .collect();
        let hang = |answer: &AnswersText<K>| match answer.source {
            SourceText::Arm { question, arm, .. } => Some((question, arm)),
            SourceText::Call { edge, .. } => match edges[edge.index()].on {
                OnText::Arm { question, arm } => Some((question, arm)),
                OnText::Site(_) => None,
            },
            SourceText::Site { .. } => None,
        };
        for (index, answer) in answers.iter().enumerate() {
            let edge = AnswersId::new(index);
            let name = answer.name;
            if site_name(name).is_some() {
                return Err(Malformed::AnswersSiteName { edge, name }.into());
            }
            if let SourceText::Arm {
                question,
                arm,
                content,
            } = &answer.source
            {
                let question = &questions[question.index()];
                if *arm >= question.arms {
                    let edge = Edge::Answers(edge);
                    return Err(Malformed::ArmOutOfRange { edge }.into());
                }
                for (name, _) in facts.mentions(*content) {
                    if !question.reads.contains(&name) {
                        let edge = Edge::Answers(edge);
                        return Err(Malformed::MentionsUnread { edge, name }.into());
                    }
                }
            }
            // A name of no text is visible nowhere.
            if let Some(&name) =
                (site_names[index].iter()).find(|name| name.index() >= name_texts.len())
            {
                let edge = Edge::Answers(edge);
                return Err(Malformed::MentionsOffChain { edge, name }.into());
            }
            let here = hang(answer);
            for earlier in answers[..index]
                .iter()
                .filter(|earlier| earlier.name == name)
            {
                let apart = matches!(
                    (here, hang(earlier)),
                    (Some((question, arm)), Some((other, other_arm)))
                        if question == other && arm != other_arm
                );
                if !apart {
                    return Err(Malformed::AnswersNameTwice { edge, name }.into());
                }
            }
        }
        let mut typed: Vec<Vec<AnswersId>> = vec![Vec::new(); name_texts.len()];
        for (index, answer) in answers.iter().enumerate() {
            typed[answer.name.index()].push(AnswersId::new(index));
        }
        for (index, name) in name_texts.iter().enumerate() {
            if *name == NameText::Answered && typed[index].is_empty() {
                let name = NameId::new(index);
                return Err(Malformed::Unanswered { name }.into());
            }
        }
        for (index, answer) in answers.iter().enumerate() {
            if let SourceText::Call { edge, takes } = answer.source
                && let Some(site) = site_name(takes)
            {
                let callee = edges[edge.index()].callee();
                if site.body() != callee && !chains.visible(site, Site::Entry(callee)) {
                    let edge = AnswersId::new(index);
                    return Err(Malformed::TakesOffCallee { edge, name: takes }.into());
                }
            }
        }
        let deciding: Vec<Option<QuestionId>> = typed
            .iter()
            .map(|edges| {
                let first = edges.first()?;
                hang(&answers[first.index()]).map(|(question, _)| question)
            })
            .collect();

        // 5. Own reads: a cycle among the answered names each answered name reads directly.
        let mut direct = vec![Vec::new(); name_texts.len()];
        for (index, answer) in answers.iter().enumerate() {
            let mut reads: Vec<NameId> = Vec::new();
            if let Some((question, _)) = hang(answer) {
                reads.extend(&questions[question.index()].reads);
            }
            if let SourceText::Call { edge, .. } = answer.source {
                let mentioned = mentions[edge.index()].iter().flatten();
                reads.extend(mentioned.map(|(name, _)| *name));
            }
            reads.extend(&site_names[index]);
            for read in reads {
                if site_name(read).is_none() {
                    direct[read.index()].push(answer.name.index());
                }
            }
        }
        let component = grows::components(&direct);
        let mut size = vec![0usize; name_texts.len()];
        for &member in &component {
            size[member] += 1;
        }
        for (name, reads) in direct.iter().enumerate() {
            if size[component[name]] > 1 || reads.contains(&name) {
                let name = NameId::new(name);
                return Err(Malformed::AnswersOwnRead { name }.into());
            }
        }

        // 6. Bases, the calls' inputs and the bodies' captures, together to a fixpoint.
        let site_of = |name: NameId| site_name(name).expect("a base holds only site names");
        let call_site = |edge: &EdgeText<K>| match edge.on {
            OnText::Arm { question, .. } => questions[question.index()].site,
            OnText::Site(site) => site,
        };
        let mut base: Vec<Names> = (0..name_texts.len())
            .map(|index| {
                let name = NameId::new(index);
                site_name(name).map_or_else(Vec::new, |_| vec![name])
            })
            .collect();
        let reads_base = |base: &[Names], reads: &[NameId]| {
            let mut set = Names::new();
            for read in reads {
                union_into(&mut set, &base[read.index()]);
            }
            set
        };
        let mut inputs: Vec<Names> = vec![Names::new(); edges.len()];
        let mut captured: Vec<Names> = vec![Names::new(); bodies.len()];
        loop {
            let mut changed = false;
            for (index, edge) in edges.iter().enumerate() {
                let mentioned = mentions[index].iter().flatten().map(|(name, _)| *name);
                let mut set = reads_base(&base, &mentioned.collect::<Vec<_>>());
                if let OnText::Arm { question, .. } = edge.on {
                    union_into(
                        &mut set,
                        &reads_base(&base, &questions[question.index()].reads),
                    );
                }
                let at = call_site(edge);
                if edge.onward().is_none() {
                    for &name in &captured[edge.callee().index()] {
                        if chains.visible(site_of(name), at) {
                            insert(&mut set, name);
                        }
                    }
                }
                changed |= union_into(&mut inputs[index], &set);
            }
            for (index, answer) in answers.iter().enumerate() {
                let set = match &answer.source {
                    SourceText::Arm { question, .. } => {
                        reads_base(&base, &questions[question.index()].reads)
                    }
                    SourceText::Call { edge, .. } => inputs[edge.index()].clone(),
                    SourceText::Site { .. } => reads_base(&base, &site_names[index]),
                };
                changed |= union_into(&mut base[answer.name.index()], &set);
            }
            for question in &questions {
                let set = reads_base(&base, &question.reads);
                changed |=
                    mark_outside(&chains, &mut captured, site_of, question.site.body(), &set);
            }
            for (index, answer) in answers.iter().enumerate() {
                if let SourceText::Site { site, .. } = answer.source {
                    let set = reads_base(&base, &site_names[index]);
                    changed |= mark_outside(&chains, &mut captured, site_of, site.body(), &set);
                }
            }
            for (index, edge) in edges.iter().enumerate() {
                let body = call_site(edge).body();
                changed |= mark_outside(&chains, &mut captured, site_of, body, &inputs[index]);
            }
            for answer in &answers {
                if let SourceText::Call { edge, takes } = answer.source {
                    let callee = edges[edge.index()].callee();
                    let set = &base[takes.index()];
                    changed |= mark_outside(&chains, &mut captured, site_of, callee, set);
                }
            }
            if !changed {
                break;
            }
        }

        // 7. Visibility of every base a question reads, or a call or a term on a site mentions.
        for (index, question) in questions.iter().enumerate() {
            for &name in &question.reads {
                if base[name.index()]
                    .iter()
                    .any(|&bound| !chains.visible(site_of(bound), question.site))
                {
                    let question = QuestionId::new(index);
                    return Err(Malformed::ReadOffChain { question, name }.into());
                }
            }
        }
        for (index, edge) in edges.iter().enumerate() {
            if let OnText::Site(at) = edge.on {
                for &(name, _) in mentions[index].iter().flatten() {
                    if base[name.index()]
                        .iter()
                        .any(|&bound| !chains.visible(site_of(bound), at))
                    {
                        let edge = Edge::Passes(EdgeId::new(index));
                        return Err(Malformed::MentionsOffChain { edge, name }.into());
                    }
                }
            }
        }
        for (index, answer) in answers.iter().enumerate() {
            if let SourceText::Site { site: at, .. } = answer.source {
                for &name in &site_names[index] {
                    if base[name.index()]
                        .iter()
                        .any(|&bound| !chains.visible(site_of(bound), at))
                    {
                        let edge = Edge::Answers(AnswersId::new(index));
                        return Err(Malformed::MentionsOffChain { edge, name }.into());
                    }
                }
            }
        }

        // 8. Each inner body's latest captured statement of its encloser.
        for (body, captured) in captured.iter().enumerate() {
            let Some(up) = chains.encloser[body] else {
                continue;
            };
            chains.captured_upto[body] = captured
                .iter()
                .filter_map(|&name| match site_of(name) {
                    Site::Statement(at, index) if at == up => Some(index),
                    _ => None,
                })
                .max();
        }

        // 9. Every entry is a node, then every statement in some reader's base, in (body, index)
        // order.
        let question_bases: Vec<Names> = questions
            .iter()
            .map(|question| reads_base(&base, &question.reads))
            .collect();
        let takes_bases = answers.iter().filter_map(|answer| match answer.source {
            SourceText::Call { takes, .. } => Some(&base[takes.index()]),
            SourceText::Arm { .. } | SourceText::Site { .. } => None,
        });
        // A term on a site is a reader: its name is placed at the latest site of its base.
        let site_bases = (answers.iter())
            .filter(|answer| matches!(answer.source, SourceText::Site { .. }))
            .map(|answer| &base[answer.name.index()]);
        let mut statement_sites: Vec<Site> = (question_bases.iter().chain(&inputs))
            .chain(takes_bases)
            .chain(site_bases)
            .flatten()
            .map(|&name| site_of(name))
            .filter(|site| matches!(site, Site::Statement(..)))
            .collect();
        statement_sites.sort_by_key(|site| (site.body(), site.rank()));
        statement_sites.dedup();
        let sites = (0..bodies.len()).map(|body| Site::Entry(BodyId::new(body)));
        let sites: Vec<Site> = sites.chain(statement_sites).collect();
        let node_at: HashMap<Site, NodeId> = sites
            .iter()
            .enumerate()
            .map(|(index, site)| (*site, NodeId::new(index)))
            .collect();

        // A body's statement nodes, in index order, for `BindsAfter`.
        let mut statement_nodes: Vec<Vec<(u32, NodeId)>> = vec![Vec::new(); bodies.len()];
        for (index, site) in sites.iter().enumerate() {
            if let Site::Statement(body, statement) = *site {
                statement_nodes[body.index()].push((statement, NodeId::new(index)));
            }
        }
        let entry = |body: BodyId| NodeId::new(body.index());
        // The latest statement node of `body` at or before `upto`, else `body`'s entry.
        let latest = |body: BodyId, upto: Option<u32>| {
            let nodes = &statement_nodes[body.index()];
            let before = upto.map_or(0, |upto| nodes.partition_point(|(index, _)| *index <= upto));
            before
                .checked_sub(1)
                .map_or(entry(body), |last| nodes[last].1)
        };
        let mut nodes: Vec<BindingNode> = sites
            .iter()
            .map(|&site| {
                let earlier = match site {
                    Site::Entry(body) => chains.encloser[body.index()]
                        .map(|up| latest(up, chains.captured_upto[body.index()])),
                    Site::Statement(body, 0) => Some(entry(body)),
                    Site::Statement(body, index) => Some(latest(body, Some(index - 1))),
                };
                let open = matches!(site, Site::Entry(body) if bodies[body.index()].open_caller);
                BindingNode {
                    site,
                    names: Vec::new(),
                    answered: Vec::new(),
                    owns: Vec::new(),
                    follows: Vec::new(),
                    binds_after: earlier.map(|earlier| BindsAfter { earlier }),
                    open_caller: open.then_some(OpenCaller),
                    fields: Vec::new(),
                    width: 0,
                }
            })
            .collect();
        for (index, name) in name_texts.iter().enumerate() {
            if let NameText::Site(site) = name
                && let Some(&node) = node_at.get(site)
            {
                nodes[node.index()].names.push(NameId::new(index));
            }
        }

        // 10. Owners and placement: the latest site in a base, or the root's entry; for what is
        // asked or called at `at` inside a staged body, no earlier than that body's entry.
        let staged_entry = |at: Site| {
            std::iter::successors(Some(at.body()), |body| chains.encloser[body.index()])
                .find(|body| bodies[body.index()].staged)
                .map(Site::Entry)
        };
        let owner_of = |set: &Names, at: Option<Site>| {
            let sites = set.iter().map(|&name| site_of(name));
            let site = chains.latest(sites.chain(at.and_then(staged_entry)));
            let site = site.unwrap_or(Site::Entry(root));
            debug_assert!(
                set.iter()
                    .all(|&name| chains.before(site_of(name), site, false))
            );
            node_at[&site]
        };
        let question_owners: Vec<NodeId> = (question_bases.iter().zip(&questions))
            .map(|(set, question)| owner_of(set, Some(question.site)))
            .collect();
        let call_owners: Vec<NodeId> = (inputs.iter().zip(&edges))
            .map(|(set, edge)| owner_of(set, Some(call_site(edge))))
            .collect();
        let mut names = Vec::with_capacity(name_texts.len());
        let mut answered = Vec::with_capacity(name_texts.len());
        for (index, name) in name_texts.iter().enumerate() {
            match name {
                NameText::Site(site) => {
                    names.push(*site);
                    answered.push(None);
                }
                NameText::Answered => {
                    let node = owner_of(&base[index], None);
                    nodes[node.index()].answered.push(NameId::new(index));
                    names.push(sites[node.index()]);
                    answered.push(Some(Answered {
                        node,
                        deciding: deciding[index],
                        edges: typed[index].clone(),
                    }));
                }
            }
        }
        for (index, answer) in answers.iter().enumerate() {
            if let SourceText::Call { edge, takes } = answer.source
                && site_name(takes).is_none()
            {
                let callee = edges[edge.index()].callee();
                let site = names[takes.index()];
                if site.body() != callee && !chains.visible(site, Site::Entry(callee)) {
                    let edge = AnswersId::new(index);
                    return Err(Malformed::TakesOffCallee { edge, name: takes }.into());
                }
            }
        }

        // 11. Inherited names, a fixpoint: what a reader inside a body reads from outside it,
        // and what a callee inherits that a call inside sees.
        let named = |name: NameId| names[name.index()];
        let mut inherits: Vec<Names> = vec![Names::new(); bodies.len()];
        loop {
            let mut changed = false;
            for question in &questions {
                let body = question.site.body();
                changed |= mark_outside(&chains, &mut inherits, named, body, &question.reads);
            }
            for (index, edge) in edges.iter().enumerate() {
                let at = call_site(edge);
                let mut read: Names = Names::new();
                for &(name, _) in mentions[index].iter().flatten() {
                    insert(&mut read, name);
                }
                for &name in &inherits[edge.callee().index()] {
                    if chains.visible(named(name), at) {
                        insert(&mut read, name);
                    }
                }
                changed |= mark_outside(&chains, &mut inherits, named, at.body(), &read);
            }
            for answer in &answers {
                if let SourceText::Call { edge, takes } = answer.source {
                    let callee = edges[edge.index()].callee();
                    changed |= mark_outside(&chains, &mut inherits, named, callee, &[takes]);
                }
            }
            for (index, answer) in answers.iter().enumerate() {
                if let SourceText::Site { site, .. } = answer.source {
                    let body = site.body();
                    changed |=
                        mark_outside(&chains, &mut inherits, named, body, &site_names[index]);
                }
            }
            if !changed {
                break;
            }
        }

        // 12. Filing: questions with their branches, then calls and `AnswersName` edges.
        let mut sited: Vec<SiteQuestion<K>> = questions
            .into_iter()
            .zip(&question_owners)
            .enumerate()
            .map(|(index, (question, &owner))| {
                nodes[owner.index()].owns.push(QuestionId::new(index));
                SiteQuestion {
                    site: question.site,
                    reads: question.reads,
                    owner,
                    branches: (0..question.arms)
                        .map(|_| AnswerBranch::default())
                        .collect(),
                    asks: (question.asks.into_iter())
                        .map(|(names, asked)| Ask::new(names, asked))
                        .collect(),
                    fields: Vec::new(),
                }
            })
            .collect();
        // Each node's handle: a field per distinct ask its owned questions declare.
        for node in &mut nodes {
            let mut distinct: Vec<&Ask<K>> = Vec::new();
            let mut per_question = Vec::with_capacity(node.owns.len());
            for &question in &node.owns {
                let mut fields = Vec::new();
                for ask in &sited[question.index()].asks {
                    let field = match distinct.iter().position(|seen| *seen == ask) {
                        Some(field) => field,
                        None => {
                            distinct.push(ask);
                            distinct.len() - 1
                        }
                    };
                    fields.push(u32::try_from(field).expect("a node's fields fit u32"));
                }
                per_question.push(fields);
            }
            let counts = distinct.iter().map(|ask| facts.answer_count(ask.question));
            (node.fields, node.width) = Field::packed(counts);
            for (&question, fields) in node.owns.iter().zip(per_question) {
                sited[question.index()].fields = fields;
            }
        }
        let mut laid: Vec<PassesName<K>> = Vec::with_capacity(edges.len());
        for (index, (edge_text, &owner)) in edges.into_iter().zip(&call_owners).enumerate() {
            let edge = EdgeId::new(index);
            let on = match edge_text.on {
                OnText::Arm { question, arm } => {
                    sited[question.index()].branches[arm as usize]
                        .follows
                        .push(edge);
                    On::Arm { question, arm }
                }
                OnText::Site(site) => On::Site(site),
            };
            if edge_text.onward().is_none() {
                nodes[owner.index()].follows.push(edge);
            }
            laid.push(PassesName {
                on,
                owner,
                target: edge_text.target,
                mapping: edge_text.mapping,
                answers: Vec::new(),
                passes: Vec::new(),
            });
        }
        let mut typing: Vec<AnswersName<K>> = Vec::with_capacity(answers.len());
        for (index, answer) in answers.into_iter().enumerate() {
            let edge = AnswersId::new(index);
            let source = match answer.source {
                SourceText::Arm {
                    question,
                    arm,
                    content,
                } => {
                    sited[question.index()].branches[arm as usize]
                        .binds
                        .push(edge);
                    Source::Arm {
                        question,
                        arm,
                        content,
                    }
                }
                SourceText::Call { edge: call, takes } => {
                    laid[call.index()].answers.push(edge);
                    Source::Call { edge: call, takes }
                }
                SourceText::Site { site, content } => Source::Site { site, content },
            };
            typing.push(AnswersName {
                name: answer.name,
                source,
                mentions: site_names[index].clone(),
            });
        }

        // Each node's relevant names and borrowed questions, now that calls have owners.
        let call_inherits = |edge: EdgeId| -> Vec<NameId> {
            if laid[edge.index()].onward().is_some() {
                return Vec::new();
            }
            let at = match laid[edge.index()].on {
                On::Arm { question, .. } => sited[question.index()].site,
                On::Site(site) => site,
            };
            let inherited = inherits[laid[edge.index()].callee().index()]
                .iter()
                .copied();
            inherited
                .filter(|&name| chains.visible(named(name), at))
                .collect()
        };
        let passes: Vec<Names> = (0..laid.len())
            .map(|index| {
                let mut passed = Names::new();
                for &(name, _) in mentions[index].iter().flatten() {
                    insert(&mut passed, name);
                }
                for name in call_inherits(EdgeId::new(index)) {
                    insert(&mut passed, name);
                }
                passed
            })
            .collect();
        let mut relevant: Vec<Names> = vec![Names::new(); nodes.len()];
        let mut borrows: Vec<Vec<QuestionId>> = vec![Vec::new(); nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            let set = &mut relevant[index];
            for &question in &node.owns {
                union_into(set, &sited[question.index()].reads);
            }
            let call = |set: &mut Names, edge: EdgeId| {
                union_into(set, &passes[edge.index()]);
                if let On::Arm { question, .. } = laid[edge.index()].on {
                    union_into(set, &sited[question.index()].reads);
                }
            };
            for &edge in &node.follows {
                call(set, edge);
                if let On::Arm { question, .. } = laid[edge.index()].on
                    && sited[question.index()].owner.index() != index
                    && !borrows[index].contains(&question)
                {
                    borrows[index].push(question);
                }
            }
            for &name in &node.answered {
                insert(set, name);
                let placed = answered[name.index()].as_ref().expect("an answered name");
                if let Some(question) = placed.deciding {
                    union_into(set, &sited[question.index()].reads);
                    if sited[question.index()].owner.index() != index
                        && !borrows[index].contains(&question)
                    {
                        borrows[index].push(question);
                    }
                }
                for &typed in &placed.edges {
                    match typing[typed.index()].source {
                        Source::Call { edge, .. } => call(set, edge),
                        Source::Site { .. } => {
                            union_into(set, &typing[typed.index()].mentions);
                        }
                        Source::Arm { .. } => {}
                    }
                }
            }
            borrows[index].sort_unstable();
        }
        let mut passes = passes;
        let mut onward_into: Vec<Option<EdgeId>> = vec![None; nodes.len()];
        // Onward calls, the latest target first: each passes what its mapping mentions
        // and what its target's evaluation reads from before it, and is owned where the last of
        // those is bound. One whose target is no node is dropped.
        let mut onward: Vec<(usize, Site, NodeId)> = (laid.iter().enumerate())
            .filter_map(|(index, edge)| {
                let target = edge.onward()?;
                Some((index, target, *node_at.get(&target)?))
            })
            .collect();
        onward.sort_by_key(|&(_, site, _)| {
            std::cmp::Reverse((chains.depth[site.body().index()], site.rank()))
        });
        for (index, target, node) in onward {
            let mut passed = passes[index].clone();
            for &name in &relevant[node.index()] {
                if named(name) != target {
                    insert(&mut passed, name);
                }
            }
            let sites = passed.iter().map(|&name| named(name));
            let owner_site = chains.latest(sites).unwrap_or(Site::Entry(root));
            let owner = node_at[&owner_site];
            if owner == node {
                return Err(Malformed::ArrivesThroughItself {
                    edge: EdgeId::new(index),
                }
                .into());
            }
            if onward_into[node.index()].is_some() {
                return Err(Malformed::ArrivesTwice {
                    edge: EdgeId::new(index),
                }
                .into());
            }
            onward_into[node.index()] = Some(EdgeId::new(index));
            laid[index].owner = owner;
            let follows = &mut nodes[owner.index()].follows;
            let at = follows.partition_point(|held| held.index() < index);
            follows.insert(at, EdgeId::new(index));
            union_into(&mut relevant[owner.index()], &passed);
            passes[index] = passed;
        }
        for (edge, passed) in laid.iter_mut().zip(passes) {
            edge.passes = passed;
        }

        // 13. Growth: mapping arcs, content arcs on arms and sites, and summary arcs, in edge
        // order.
        let mut arcs = Vec::new();
        for (index, edge) in laid.iter().enumerate() {
            for ((to, _), mentioned) in edge.mapping.iter().zip(&mentions[index]) {
                for &(from, nesting) in mentioned {
                    let edge = Edge::Passes(EdgeId::new(index));
                    let to = *to;
                    arcs.push(Arc {
                        from,
                        to,
                        nesting,
                        edge,
                    });
                }
            }
        }
        let mut content_arcs: Vec<Vec<Arc>> = vec![Vec::new(); typing.len()];
        let mut throughs = Vec::new();
        for (index, answer) in typing.iter().enumerate() {
            let edge = AnswersId::new(index);
            match &answer.source {
                Source::Arm { content, .. } | Source::Site { content, .. } => {
                    for (from, nesting) in facts.mentions(*content) {
                        content_arcs[index].push(Arc {
                            from,
                            to: answer.name,
                            nesting,
                            edge: Edge::Answers(edge),
                        });
                    }
                }
                Source::Call { edge: call, takes } => {
                    let mapping = laid[call.index()].mapping.iter();
                    let mapped = mapping
                        .zip(&mentions[call.index()])
                        .map(|((name, _), mentioned)| (*name, mentioned.clone()));
                    throughs.push(Through {
                        edge,
                        name: answer.name,
                        takes: *takes,
                        callee: laid[call.index()].callee(),
                        mapped: mapped.collect(),
                    });
                }
            }
        }
        let mut flat: Vec<Arc> = content_arcs.iter().flatten().copied().collect();
        // An onward call's mapping types a name of its own body, as a term on a site does.
        for (index, edge) in laid.iter().enumerate() {
            if edge.onward().is_none() {
                continue;
            }
            for ((to, _), mentioned) in edge.mapping.iter().zip(&mentions[index]) {
                for &(from, nesting) in mentioned {
                    flat.push(Arc {
                        from,
                        to: *to,
                        nesting,
                        edge: Edge::Passes(EdgeId::new(index)),
                    });
                }
            }
        }
        let outside =
            |through: &Through, name: NameId| !chains.within(through.callee, named(name).body());
        let mut summaries = grows::summaries(names.len(), &flat, &throughs, outside).into_iter();
        for (index, answer) in typing.iter().enumerate() {
            match answer.source {
                Source::Arm { .. } | Source::Site { .. } => arcs.append(&mut content_arcs[index]),
                Source::Call { .. } => arcs.extend(summaries.next().expect("one per through")),
            }
        }
        if let Some(arc) = grows::first_growth(names.len(), &arcs) {
            debug_assert_eq!(arc.nesting, Nesting::Built);
            return Err(LayError::Grows {
                edge: arc.edge,
                name: arc.to,
            });
        }

        // Each statement node's chain of onward calls, from its body's entry or from before it.
        let reached: Vec<Option<Vec<EdgeId>>> = (0..nodes.len())
            .map(|index| {
                let body = nodes[index].site.body();
                let mut chain = Vec::new();
                let mut at = index;
                loop {
                    let edge = onward_into[at]?;
                    chain.push(edge);
                    let owner = laid[edge.index()].owner;
                    let site = nodes[owner.index()].site;
                    if site.body() != body || matches!(site, Site::Entry(_)) {
                        break;
                    }
                    at = owner.index();
                }
                chain.reverse();
                Some(chain)
            })
            .collect();

        let mut graph = Graph {
            reached,
            chains,
            names,
            answered,
            nodes,
            node_at,
            questions: sited,
            edges: laid,
            answers: typing,
            relevant,
            borrows,
            name_components: Vec::new(),
            question_components: Vec::new(),
        };
        let components = super::needs::text_components(&graph);
        graph.name_components = components.names;
        graph.question_components = components.questions;
        Ok(graph)
    }
}
