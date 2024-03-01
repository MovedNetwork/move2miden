//! Module for creating control flow graphs for Move functions.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    fmt, iter,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingEdge {
    If { true_case: Label, false_case: Label },
    Pass { next: Label },
    LoopBack { header: Label },
    WhileTrue { body_start: Label, after: Label },
    // Miden does not have while false, but it is
    // possible in Move because the loop structure is less restrictive.
    // We will convert to `WhileTrue` by adding an extra `Not` instruction
    // during the compilation step.
    WhileFalse { body_start: Label, after: Label },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cfg<'a> {
    blocks: BTreeMap<Label, Block<'a>>,
    // Edges are directed as start -> end.
    edges: BTreeMap<Label, OutgoingEdge>,
}

impl<'a> Cfg<'a> {
    pub fn new(bytecode: &'a [Bytecode]) -> Result<Self, CfgError> {
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
                    validate_conditional_jump(x, i, bytecode)?;
                    branch_origins.insert(i);
                    // Both x and i + 1 are branch destinations because we jump to x
                    // if the condition is met and simply go to the next bytecode otherwise.
                    branch_dests.insert(x);
                    branch_dests.insert(i + 1);
                }
                Bytecode::Branch(x) => {
                    let x = *x as usize;
                    validate_unconditional_jump(x, i, bytecode)?;
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
                    edges.insert(l, OutgoingEdge::Pass { next: maybe_label });
                }
                current_label = Some(maybe_label);
            }
            let l = match current_label {
                Some(l) => l,
                None => continue,
            };
            match b {
                Bytecode::BrTrue(x) => {
                    let x = *x as usize;
                    let true_case = Label::new(x);
                    let false_case = match bytecode.get(i + 1) {
                        Some(Bytecode::Branch(x)) => Label::new(*x as usize),
                        _ => Label::new(i + 1),
                    };
                    edges.insert(
                        l,
                        OutgoingEdge::If {
                            true_case,
                            false_case,
                        },
                    );
                    current_label = None;
                }
                Bytecode::BrFalse(x) => {
                    let x = *x as usize;
                    let false_case = Label::new(x);
                    let true_case = match bytecode.get(i + 1) {
                        Some(Bytecode::Branch(x)) => Label::new(*x as usize),
                        _ => Label::new(i + 1),
                    };
                    edges.insert(
                        l,
                        OutgoingEdge::If {
                            true_case,
                            false_case,
                        },
                    );
                    current_label = None;
                }
                Bytecode::Branch(x) => {
                    let x = *x as usize;
                    let dest_label = Label::new(x);
                    let edge = if x < i {
                        // In the loop-back case we convert the if-else into a while loop
                        let Some(OutgoingEdge::If {
                            true_case,
                            false_case,
                        }) = edges.remove(&dest_label)
                        else {
                            return Err(CfgError::InvalidLoopHeader);
                        };
                        // Need to figure out if the true case or false case is the
                        // body of the loop. The body is the path which leads to
                        // the current label (since it is branching back up to the header).
                        match (
                            has_path(&edges, &true_case, &l),
                            has_path(&edges, &false_case, &l),
                        ) {
                            // Exactly one path should get to this node; if none or both do then there is a problem
                            (true, true) | (false, false) => {
                                return Err(CfgError::InvalidLoopHeader)
                            }
                            (true, false) => edges.insert(
                                dest_label,
                                OutgoingEdge::WhileTrue {
                                    body_start: true_case,
                                    after: false_case,
                                },
                            ),
                            (false, true) => edges.insert(
                                dest_label,
                                OutgoingEdge::WhileFalse {
                                    body_start: false_case,
                                    after: true_case,
                                },
                            ),
                        };
                        OutgoingEdge::LoopBack { header: dest_label }
                    } else {
                        OutgoingEdge::Pass { next: dest_label }
                    };
                    edges.insert(l, edge);
                    current_label = None;
                }
                Bytecode::Abort | Bytecode::Ret => {
                    // Abort and Ret signify the end of the function
                    edges.insert(l, OutgoingEdge::Pass { next: Label::Exit });
                    current_label = None;
                }
                _ => continue,
            }
        }
        // The last block exits the function
        if let Some(l) = current_label {
            edges.insert(l, OutgoingEdge::Pass { next: Label::Exit });
        }

        Ok(Self { blocks, edges })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfgError {
    // BrTrue, BrFalse and Branch are not allowed to jump to another such instruction.
    BranchToBranch,
    // BrTrue and BrFalse are not allowed to jump to an earlier index in the bytecode.
    ConditionalJumpBack,
    // BrTrue, BrFalse and Branch must branch to a value in [0, bytecode.len() - 1].
    BranchOutOfBounds,
    // BrTrue, BrFalse and Branch cannot target itself as the jump point.
    SelfBranch,
    // It is not allowed to have multiple BrTrue/BrFalse in a row.
    RepeatConditionalBranch,
    // This error is returned if we try to set and edge when not in a block
    UnexpectedBlockEnd,
    // Loop headers are expected to have two branch options: loop body or post-loop code
    InvalidLoopHeader,
}

impl fmt::Display for CfgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for CfgError {}

fn validate_conditional_jump(
    dest: usize,
    index: usize,
    bytecode: &[Bytecode],
) -> Result<(), CfgError> {
    if dest < index {
        return Err(CfgError::ConditionalJumpBack);
    }
    if bytecode
        .get(index + 1)
        .filter(|b| Bytecode::is_conditional_branch(b))
        .is_some()
    {
        return Err(CfgError::RepeatConditionalBranch);
    }
    validate_unconditional_jump(dest, index, bytecode)
}

fn validate_unconditional_jump(
    dest: usize,
    index: usize,
    bytecode: &[Bytecode],
) -> Result<(), CfgError> {
    if dest == index {
        return Err(CfgError::SelfBranch);
    }
    let dest_code = bytecode.get(dest).ok_or(CfgError::BranchOutOfBounds)?;
    match dest_code {
        Bytecode::BrTrue(_) | Bytecode::BrFalse(_) | Bytecode::Branch(_) => {
            Err(CfgError::BranchToBranch)
        }
        _ => Ok(()),
    }
}

// Use BFS to see if there is a path from `start` to `target` using `edges`
fn has_path(edges: &BTreeMap<Label, OutgoingEdge>, start: &Label, target: &Label) -> bool {
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(start);
    while let Some(label) = queue.pop_front() {
        visited.insert(label);
        if label == target {
            return true;
        }
        match edges.get(label) {
            Some(OutgoingEdge::If {
                true_case,
                false_case,
            }) => {
                if !visited.contains(true_case) {
                    queue.push_back(true_case);
                }
                if !visited.contains(false_case) {
                    queue.push_back(false_case);
                }
            }
            Some(OutgoingEdge::LoopBack { header }) => {
                if !visited.contains(header) {
                    queue.push_back(header);
                }
            }
            Some(OutgoingEdge::Pass { next }) => {
                if !visited.contains(next) {
                    queue.push_back(next);
                }
            }
            Some(OutgoingEdge::WhileTrue { body_start, after }) => {
                if !visited.contains(body_start) {
                    queue.push_back(body_start);
                }
                if !visited.contains(after) {
                    queue.push_back(after);
                }
            }
            Some(OutgoingEdge::WhileFalse { body_start, after }) => {
                if !visited.contains(body_start) {
                    queue.push_back(body_start);
                }
                if !visited.contains(after) {
                    queue.push_back(after);
                }
            }
            None => (),
        }
    }
    false
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
        let cfg = Cfg::new(&bytecode).unwrap();
        let expected = build_expected_cfg(
            [(Label::Entry, bytecode.as_slice()), (Label::Exit, &[])],
            [(Label::Entry, OutgoingEdge::Pass { next: Label::Exit })],
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
        let cfg = Cfg::new(&bytecode).unwrap();
        let expected = build_expected_cfg(
            [
                (Label::Entry, &bytecode[0..5]),
                (Label::Point(7), &bytecode[7..9]),
                (Label::Point(9), &bytecode[9..]),
                (Label::Exit, &[]),
            ],
            [
                (
                    Label::Entry,
                    OutgoingEdge::If {
                        true_case: Label::Point(9),
                        false_case: Label::Point(7),
                    },
                ),
                (Label::Point(7), OutgoingEdge::Pass { next: Label::Exit }),
                (Label::Point(9), OutgoingEdge::Pass { next: Label::Exit }),
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
        let cfg = Cfg::new(&bytecode).unwrap();
        let expected = build_expected_cfg(
            [
                (Label::Entry, &bytecode[0..4]),
                (Label::Point(4), &bytecode[4..7]),
                (Label::Point(9), &bytecode[9..17]),
                (Label::Point(18), &bytecode[18..20]),
                (Label::Exit, &[]),
            ],
            [
                (
                    Label::Entry,
                    OutgoingEdge::Pass {
                        next: Label::Point(4),
                    },
                ),
                (
                    Label::Point(4),
                    OutgoingEdge::WhileTrue {
                        body_start: Label::Point(9),
                        after: Label::Point(18),
                    },
                ),
                (
                    Label::Point(9),
                    OutgoingEdge::LoopBack {
                        header: Label::Point(4),
                    },
                ),
                (Label::Point(18), OutgoingEdge::Pass { next: Label::Exit }),
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
        let cfg = Cfg::new(&bytecode).unwrap();
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
                (
                    Label::Entry,
                    OutgoingEdge::If {
                        true_case: Label::Point(8),
                        false_case: Label::Point(10),
                    },
                ),
                (Label::Point(8), OutgoingEdge::Pass { next: Label::Exit }),
                (
                    Label::Point(10),
                    OutgoingEdge::If {
                        true_case: Label::Point(14),
                        false_case: Label::Point(16),
                    },
                ),
                (Label::Point(14), OutgoingEdge::Pass { next: Label::Exit }),
                (
                    Label::Point(16),
                    OutgoingEdge::WhileFalse {
                        body_start: Label::Point(29),
                        after: Label::Point(34),
                    },
                ),
                (
                    Label::Point(29),
                    OutgoingEdge::LoopBack {
                        header: Label::Point(16),
                    },
                ),
                (Label::Point(34), OutgoingEdge::Pass { next: Label::Exit }),
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
        let cfg = Cfg::new(&bytecode).unwrap();
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
                (
                    Label::Entry,
                    OutgoingEdge::Pass {
                        next: Label::Point(2),
                    },
                ),
                (
                    Label::Point(2),
                    OutgoingEdge::WhileTrue {
                        body_start: Label::Point(7),
                        after: Label::Point(29),
                    },
                ),
                (
                    Label::Point(7),
                    OutgoingEdge::If {
                        true_case: Label::Point(13),
                        false_case: Label::Point(18),
                    },
                ),
                (
                    Label::Point(13),
                    OutgoingEdge::Pass {
                        next: Label::Point(24),
                    },
                ),
                (
                    Label::Point(18),
                    OutgoingEdge::Pass {
                        next: Label::Point(24),
                    },
                ),
                (
                    Label::Point(24),
                    OutgoingEdge::LoopBack {
                        header: Label::Point(2),
                    },
                ),
                (Label::Point(29), OutgoingEdge::Pass { next: Label::Exit }),
            ],
        );
        assert_eq!(cfg, expected);
    }

    fn build_expected_cfg<'a, B, E>(blocks: B, edges: E) -> Cfg<'a>
    where
        B: IntoIterator<Item = (Label, &'a [Bytecode])>,
        E: IntoIterator<Item = (Label, OutgoingEdge)>,
    {
        let expected_blocks = blocks
            .into_iter()
            .map(|(l, code)| (l, Block::new(code)))
            .collect();
        let expected_edges = edges.into_iter().collect();
        Cfg {
            blocks: expected_blocks,
            edges: expected_edges,
        }
    }
}
