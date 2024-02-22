//! Module for creating control flow graphs for Move functions.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    iter,
};

use move_binary_format::file_format::Bytecode;

/// A block of bytecode without any control flow
/// (i.e. no `BrTrue`, `BrFalse`, `Branch`).
/// A block of bytecode is a node in the control flow graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Block<'a> {
    code: &'a [Bytecode],
}

impl<'a> Block<'a> {
    pub fn new(code: &'a [Bytecode]) -> Self {
        Self { code }
    }
}

/// Labels for nodes in the control flow graph.
/// Nodes (i.e. blocks) are one of: the entrypoint to the program,
/// a specific offset in the overall array of bytecode, or the end
/// of the function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Label {
    Entry,
    Point(usize),
    Exit,
}

impl Label {
    fn cmp_inner(&self, other: &Self) -> Ordering {
        if self == other {
            return Ordering::Equal;
        }
        match self {
            // Entry is less than all others
            Self::Entry => Ordering::Less,
            // Exit is greater than all others
            Self::Exit => Ordering::Greater,
            Self::Point(x) => match other {
                // points are greater than entry
                Self::Entry => Ordering::Greater,
                // points are less than exit
                Self::Exit => Ordering::Less,
                Self::Point(y) => x.cmp(y),
            },
        }
    }
}

impl PartialOrd for Label {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Label {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cmp_inner(other)
    }
}

impl Label {
    pub fn new(index: usize) -> Self {
        if index == 0 {
            Self::Entry
        } else {
            Self::Point(index)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cfg<'a> {
    blocks: BTreeMap<Label, Block<'a>>,
    // Edges are directed as start -> end.
    edges: BTreeMap<Label, BTreeSet<Label>>,
}

impl<'a> Cfg<'a> {
    pub fn new(bytecode: &'a [Bytecode]) -> Self {
        // Locations that are destinations of a branch.
        let mut branch_dests = BTreeSet::new();
        branch_dests.insert(0); // 0 is the entry point of the function
        branch_dests.insert(bytecode.len()); // the end of the bytecode is the exit

        // Locations where branch instructions are
        let mut branch_origins = BTreeSet::new();

        for (i, b) in bytecode.iter().enumerate() {
            match b {
                Bytecode::BrTrue(x) | Bytecode::BrFalse(x) => {
                    let x = *x as usize;
                    branch_origins.insert(i);
                    // Both x and i + 1 are branch destinations because we jump to x
                    // if the condition is met and simply go to the next bytecode otherwise.
                    branch_dests.insert(x);
                    branch_dests.insert(i + 1);
                }
                Bytecode::Branch(x) => {
                    let x = *x as usize;
                    branch_origins.insert(i);
                    branch_dests.insert(x);
                }
                _ => continue,
            }
        }

        // Collect points into an ordered list
        let branch_points: Vec<usize> = branch_dests.union(&branch_origins).copied().collect();
        let blocks: BTreeMap<Label, Block<'a>> = branch_points
            .iter()
            .zip(branch_points.iter().skip(1))
            .filter_map(|(start, end)| {
                if branch_origins.contains(start) {
                    return None;
                }
                let start = *start;
                let end = *end;
                Some((Label::new(start), Block::new(&bytecode[start..end])))
            })
            .chain(iter::once((Label::Exit, Block::new(&[]))))
            .collect();

        let mut edges = BTreeMap::new();
        let mut current_label = None;
        for (i, b) in bytecode.iter().enumerate() {
            let maybe_label = Label::new(i);
            if blocks.contains_key(&maybe_label) {
                // We have entered a new block.
                // If we were already in a block then that
                // block transition to the new one.
                if let Some(l) = current_label {
                    insert_edge(&mut edges, l, maybe_label);
                }
                current_label = Some(maybe_label);
            }
            match b {
                Bytecode::BrTrue(x) | Bytecode::BrFalse(x) => {
                    let x = *x as usize;
                    let dest_label = Label::new(x);
                    if blocks.contains_key(&dest_label) {
                        if let Some(l) = current_label {
                            insert_edge(&mut edges, l, dest_label);
                        }
                    } else {
                        // This can only happen if the destination
                        // is another branch instruction.
                        // In this case we need to check the transitions of that
                        // instruction.
                        let dests = resolve_destinations(bytecode, &blocks, x);
                        if let Some(l) = current_label {
                            for dest_label in dests {
                                insert_edge(&mut edges, l, dest_label);
                            }
                        }
                    }

                    // No need to worry about the i + 1 destination,
                    // it will be handled by the next iteration of the
                    // loop because we keep `current_label`.
                }
                Bytecode::Branch(x) => {
                    let x = *x as usize;
                    let dest_label = Label::new(x);
                    if blocks.contains_key(&dest_label) {
                        if let Some(l) = current_label {
                            insert_edge(&mut edges, l, dest_label);
                        }
                    } else {
                        // This can only happen if the destination
                        // is another branch instruction.
                        // In this case we need to check the transitions of that
                        // instruction.
                        let dests = resolve_destinations(bytecode, &blocks, x);
                        if let Some(l) = current_label {
                            for dest_label in dests {
                                insert_edge(&mut edges, l, dest_label);
                            }
                        }
                    }

                    // Set current label to none because we have
                    // jumped out of the current block.
                    current_label = None;
                }
                Bytecode::Abort | Bytecode::Ret => {
                    // Abort and Ret signify the end of the function
                    if let Some(l) = current_label {
                        let dest_label = Label::Exit;
                        insert_edge(&mut edges, l, dest_label);
                    }
                    current_label = None;
                }
                _ => continue,
            }
        }
        // The last block exits the function
        if let Some(l) = current_label {
            let dest_label = Label::Exit;
            insert_edge(&mut edges, l, dest_label);
        }

        Self { blocks, edges }
    }
}

fn insert_edge(edges: &mut BTreeMap<Label, BTreeSet<Label>>, start: Label, end: Label) {
    let xs = edges.entry(start).or_default();
    xs.insert(end);
}

// Figure out what nodes of the CFG could be visited
// from the bytecode at the given index.
fn resolve_destinations<'a>(
    bytecode: &'a [Bytecode],
    blocks: &BTreeMap<Label, Block<'a>>,
    index: usize,
) -> Vec<Label> {
    let mut result = Vec::new();
    let mut to_visit = VecDeque::new();
    let mut visited = HashSet::new();
    to_visit.push_back(index);
    while let Some(index) = to_visit.pop_front() {
        visited.insert(index);
        match &bytecode[index] {
            Bytecode::BrTrue(x) | Bytecode::BrFalse(x) => {
                let x = *x as usize;
                let dest_label = Label::new(x);
                if blocks.contains_key(&dest_label) {
                    result.push(dest_label);
                } else if !visited.contains(&x) {
                    to_visit.push_back(x);
                }
                // Can visit the next index too if the condition fails
                let x = index + 1;
                if !visited.contains(&x) {
                    to_visit.push_back(x);
                }
            }
            Bytecode::Branch(x) => {
                let x = *x as usize;
                let dest_label = Label::new(x);
                if blocks.contains_key(&dest_label) {
                    result.push(dest_label);
                } else if !visited.contains(&x) {
                    to_visit.push_back(x);
                }
            }
            _ => {
                let dest_label = Label::new(index);
                if blocks.contains_key(&dest_label) {
                    result.push(dest_label);
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trivial_cfg() {
        let bytecode = vec![
            Bytecode::LdU32(0),
            Bytecode::LdU32(1),
            Bytecode::LdU32(2),
            Bytecode::LdU32(3),
            Bytecode::LdU32(4),
            Bytecode::LdU32(5),
        ];
        let cfg = Cfg::new(&bytecode);
        let expected = build_expected_cfg(
            [(Label::Entry, bytecode.as_slice()), (Label::Exit, &[])],
            [(Label::Entry, &[Label::Exit])],
        );
        assert_eq!(cfg, expected);
    }

    #[test]
    fn test_simple_cfg() {
        let bytecode = vec![
            Bytecode::LdU32(0),
            Bytecode::LdU32(0),
            Bytecode::LdU32(0),
            Bytecode::LdU32(0),
            Bytecode::LdU32(0),
            Bytecode::BrFalse(7),
            Bytecode::Branch(9),
            Bytecode::LdU32(0),
            Bytecode::Abort,
            Bytecode::Ret,
        ];
        let cfg = Cfg::new(&bytecode);
        let expected = build_expected_cfg(
            [
                (Label::Entry, &bytecode[0..5]),
                (Label::Point(7), &bytecode[7..9]),
                (Label::Point(9), &bytecode[9..]),
                (Label::Exit, &[]),
            ],
            [
                (Label::Entry, [Label::Point(7), Label::Point(9)].as_slice()),
                (Label::Point(7), &[Label::Exit]),
                (Label::Point(9), &[Label::Exit]),
            ],
        );
        assert_eq!(cfg, expected);
    }

    #[test]
    fn test_while_loop_cfg() {
        let bytecode = vec![
            Bytecode::LdU32(1),
            Bytecode::StLoc(1),
            Bytecode::LdU32(0),
            Bytecode::StLoc(2),
            Bytecode::CopyLoc(1),
            Bytecode::CopyLoc(0),
            Bytecode::Le,
            Bytecode::BrFalse(18),
            Bytecode::Branch(9),
            Bytecode::MoveLoc(2),
            Bytecode::CopyLoc(1),
            Bytecode::Add,
            Bytecode::StLoc(2),
            Bytecode::MoveLoc(1),
            Bytecode::LdU32(1),
            Bytecode::Add,
            Bytecode::StLoc(1),
            Bytecode::Branch(4),
            Bytecode::MoveLoc(2),
            Bytecode::Ret,
        ];
        let cfg = Cfg::new(&bytecode);
        let expected = build_expected_cfg(
            [
                (Label::Entry, &bytecode[0..4]),
                (Label::Point(4), &bytecode[4..7]),
                (Label::Point(9), &bytecode[9..17]),
                (Label::Point(18), &bytecode[18..20]),
                (Label::Exit, &[]),
            ],
            [
                (Label::Entry, [Label::Point(4)].as_slice()),
                (Label::Point(4), &[Label::Point(9), Label::Point(18)]),
                (Label::Point(9), &[Label::Point(4)]),
                (Label::Point(18), &[Label::Exit]),
            ],
        );
        assert_eq!(cfg, expected);
    }

    #[test]
    fn test_break_loop() {
        let bytecode = vec![
            Bytecode::LdU32(0), // Label::Entry
            Bytecode::StLoc(1),
            Bytecode::LdU32(1),
            Bytecode::StLoc(2),
            Bytecode::CopyLoc(0),
            Bytecode::LdU32(0),
            Bytecode::Eq,
            Bytecode::BrFalse(10), // Label::Point(7)
            Bytecode::MoveLoc(1),
            Bytecode::Ret,
            Bytecode::CopyLoc(0), // Label::Point(10)
            Bytecode::LdU32(1),
            Bytecode::Eq,
            Bytecode::BrFalse(16), // Label::Point(13)
            Bytecode::MoveLoc(2),
            Bytecode::Ret,
            Bytecode::MoveLoc(1), // Label::Point(16)
            Bytecode::CopyLoc(2),
            Bytecode::Add,
            Bytecode::StLoc(3),
            Bytecode::MoveLoc(2),
            Bytecode::StLoc(1),
            Bytecode::MoveLoc(3),
            Bytecode::StLoc(2),
            Bytecode::CopyLoc(0),
            Bytecode::LdU32(1),
            Bytecode::Eq,
            Bytecode::BrFalse(29), // Label::Point(27)
            Bytecode::Branch(34),
            Bytecode::MoveLoc(0), // Label::Point(29)
            Bytecode::LdU32(1),
            Bytecode::Sub,
            Bytecode::StLoc(0),
            Bytecode::Branch(16), // Label::Point(33)
            Bytecode::MoveLoc(2), // Label::Point(34)
            Bytecode::Ret,
        ];
        let cfg = Cfg::new(&bytecode);
        let expected = build_expected_cfg(
            [
                (Label::Entry, &bytecode[0..7]),
                (Label::Point(8), &bytecode[8..10]),
                (Label::Point(10), &bytecode[10..13]),
                (Label::Point(14), &bytecode[14..16]),
                (Label::Point(16), &bytecode[16..27]),
                (Label::Point(29), &bytecode[29..33]),
                (Label::Point(34), &bytecode[34..36]),
                (Label::Exit, &[]),
            ],
            [
                (Label::Entry, [Label::Point(8), Label::Point(10)].as_slice()),
                (Label::Point(8), &[Label::Exit]),
                (Label::Point(10), &[Label::Point(14), Label::Point(16)]),
                (Label::Point(14), &[Label::Exit]),
                (Label::Point(16), &[Label::Point(29), Label::Point(34)]),
                (Label::Point(29), &[Label::Point(16)]),
                (Label::Point(34), &[Label::Exit]),
            ],
        );
        assert_eq!(cfg, expected);
    }

    #[test]
    fn test_while_loop_with_if_in_body() {
        let bytecode = vec![
            Bytecode::LdU32(0), // Label::Entry
            Bytecode::StLoc(1),
            Bytecode::CopyLoc(0),
            Bytecode::LdU32(1),
            Bytecode::Neq,
            Bytecode::BrFalse(29), // Label::Point(5)
            Bytecode::Branch(7),
            Bytecode::CopyLoc(0), // Label::Point(7)
            Bytecode::LdU32(2),
            Bytecode::Mod,
            Bytecode::LdU32(0),
            Bytecode::Eq,
            Bytecode::BrFalse(18), // Label::Point(12)
            Bytecode::MoveLoc(0),
            Bytecode::LdU32(2),
            Bytecode::Div,
            Bytecode::StLoc(0),
            Bytecode::Branch(24), // Label::Point(17)
            Bytecode::LdU32(3),
            Bytecode::MoveLoc(0),
            Bytecode::Mul,
            Bytecode::LdU32(1),
            Bytecode::Add,
            Bytecode::StLoc(0),
            Bytecode::MoveLoc(1), // Label::Point(24)
            Bytecode::LdU32(1),
            Bytecode::Add,
            Bytecode::StLoc(1),
            Bytecode::Branch(2), // Label::Point(28)
            Bytecode::MoveLoc(1),
            Bytecode::Ret,
        ];
        let cfg = Cfg::new(&bytecode);
        let expected = build_expected_cfg(
            [
                (Label::Entry, &bytecode[0..2]),
                (Label::Point(2), &bytecode[2..5]),
                (Label::Point(7), &bytecode[7..12]),
                (Label::Point(13), &bytecode[13..17]),
                (Label::Point(18), &bytecode[18..24]),
                (Label::Point(24), &bytecode[24..28]),
                (Label::Point(29), &bytecode[29..31]),
                (Label::Exit, &[]),
            ],
            [
                (Label::Entry, [Label::Point(2)].as_slice()),
                (Label::Point(2), &[Label::Point(7), Label::Point(29)]),
                (Label::Point(7), &[Label::Point(13), Label::Point(18)]),
                (Label::Point(13), &[Label::Point(24)]),
                (Label::Point(18), &[Label::Point(24)]),
                (Label::Point(24), &[Label::Point(2)]),
                (Label::Point(29), &[Label::Exit]),
            ],
        );
        assert_eq!(cfg, expected);
    }

    fn build_expected_cfg<'a, 'b, B, E, T>(blocks: B, edges: E) -> Cfg<'a>
    where
        B: IntoIterator<Item = (Label, &'a [Bytecode])>,
        E: IntoIterator<Item = (Label, T)>,
        T: IntoIterator<Item = &'b Label>,
    {
        let expected_blocks = blocks
            .into_iter()
            .map(|(l, code)| (l, Block::new(code)))
            .collect();
        let expected_edges = edges
            .into_iter()
            .map(|(l, ls)| (l, ls.into_iter().copied().collect()))
            .collect();
        Cfg {
            blocks: expected_blocks,
            edges: expected_edges,
        }
    }
}
