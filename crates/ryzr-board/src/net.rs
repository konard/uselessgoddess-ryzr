//! Net extraction: collapse the grid's conductors into electrical nets.
//!
//! Conductors that touch share a net; a [`Tile::Cross`] keeps its two axes
//! apart; a [`Tile::Via`] also fuses with the via directly above or below it, so
//! a net can span layers. Drivers (gates, switches, …) attach to whatever net
//! sits on each of their sides without merging their own sides together — a gate
//! must *not* short its inputs to its output.
//!
//! The whole thing is one union-find pass over "side nodes". Two helpers,
//! [`port_on_side`] and [`Nets::neighbour_net`], express every adjacency rule,
//! and both the merge pass and the later input-resolution pass reuse them, so
//! the connectivity model is defined in exactly one place.

use std::collections::HashMap;

use crate::board::{Board, Cell};
use crate::tile::{Direction, Tile};

/// Identifies an electrical net within a single [`compile`](crate::compile)
/// run. Dense and zero-based, so it doubles as a `Vec` index.
pub type NetId = usize;

/// Which conductor of a cell a side belongs to. Every tile that conducts or
/// drives has a single [`Port::Single`] node, except [`Tile::Cross`], whose two
/// axes are deliberately distinct nodes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Port {
    Single,
    CrossVertical,
    CrossHorizontal,
}

/// The net-node a tile exposes on `side`, or `None` if that side does not
/// connect.
///
/// For a gate this is the crux: it only exposes its *output* side, so the merge
/// pass attaches the gate's output to a neighbouring net while leaving its
/// inputs free to be read separately.
fn port_on_side(tile: Tile, side: Direction) -> Option<Port> {
    match tile {
        Tile::Wire | Tile::Via | Tile::Switch | Tile::Source { .. } | Tile::Clock => {
            Some(Port::Single)
        }
        Tile::Cross => Some(match side {
            Direction::North | Direction::South => Port::CrossVertical,
            Direction::East | Direction::West => Port::CrossHorizontal,
        }),
        Tile::Gate { facing, .. } => (facing == side).then_some(Port::Single),
        Tile::Led | Tile::Empty => None,
    }
}

/// The net-node(s) a tile owns regardless of orientation — what the merge pass
/// must intern up front so even an unconnected wire still gets a net.
fn nodes_of(tile: Tile) -> &'static [Port] {
    match tile {
        Tile::Wire
        | Tile::Via
        | Tile::Switch
        | Tile::Source { .. }
        | Tile::Clock
        | Tile::Gate { .. } => &[Port::Single],
        Tile::Cross => &[Port::CrossVertical, Port::CrossHorizontal],
        Tile::Led | Tile::Empty => &[],
    }
}

/// A minimal union-find over interned side nodes.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self { parent: (0..n).collect() }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// The result of net extraction: a dense net id for every side node on the
/// board.
#[derive(Clone, Debug)]
pub struct Nets {
    count: usize,
    node_net: HashMap<(Cell, Port), NetId>,
}

impl Nets {
    /// Number of distinct nets.
    pub fn count(&self) -> usize {
        self.count
    }

    fn net_of(&self, cell: Cell, port: Port) -> Option<NetId> {
        self.node_net.get(&(cell, port)).copied()
    }

    /// The net a driver cell pushes its value onto.
    pub fn driver_net(&self, cell: Cell) -> Option<NetId> {
        self.net_of(cell, Port::Single)
    }

    /// The net presented *toward* `cell` by its neighbour in `dir` — i.e. what a
    /// gate or LED at `cell` reads from that side. Uses the same port rules as
    /// the merge pass, so reads and merges can never disagree.
    pub fn neighbour_net(&self, board: &Board, cell: Cell, dir: Direction) -> Option<NetId> {
        let neighbour = board.neighbour(cell, dir)?;
        let port = port_on_side(board.get(neighbour), dir.opposite())?;
        self.net_of(neighbour, port)
    }

    /// The nets whose value should colour `cell` when rendered: the cell's own
    /// conductor/driver net, both axes of a cross, or — for a sink LED — every
    /// net feeding it.
    pub fn display_nets(&self, board: &Board, cell: Cell) -> Vec<NetId> {
        match board.get(cell) {
            Tile::Cross => [Port::CrossVertical, Port::CrossHorizontal]
                .into_iter()
                .filter_map(|p| self.net_of(cell, p))
                .collect(),
            Tile::Led => {
                let mut nets: Vec<NetId> = Direction::ALL
                    .into_iter()
                    .filter_map(|d| self.neighbour_net(board, cell, d))
                    .collect();
                nets.sort_unstable();
                nets.dedup();
                nets
            }
            Tile::Empty => Vec::new(),
            _ => self.net_of(cell, Port::Single).into_iter().collect(),
        }
    }
}

/// Run net extraction over `board`.
pub fn extract(board: &Board) -> Nets {
    // Intern every side node so isolated tiles still receive a net.
    let mut index: HashMap<(Cell, Port), usize> = HashMap::new();
    let mut nodes: Vec<(Cell, Port)> = Vec::new();
    for (cell, tile) in board.iter() {
        for &port in nodes_of(tile) {
            let key = (cell, port);
            index.entry(key).or_insert_with(|| {
                nodes.push(key);
                nodes.len() - 1
            });
        }
    }

    // Merge touching nodes.
    let mut uf = UnionFind::new(nodes.len());
    for (cell, tile) in board.iter() {
        for side in Direction::ALL {
            let Some(port) = port_on_side(tile, side) else { continue };
            let Some(neighbour) = board.neighbour(cell, side) else { continue };
            let Some(neighbour_port) = port_on_side(board.get(neighbour), side.opposite()) else {
                continue;
            };
            uf.union(index[&(cell, port)], index[&(neighbour, neighbour_port)]);
        }

        // Vias additionally fuse with a via straight above or below.
        if matches!(tile, Tile::Via) {
            for other in board.vertical_neighbours(cell) {
                if matches!(board.get(other), Tile::Via) {
                    uf.union(index[&(cell, Port::Single)], index[&(other, Port::Single)]);
                }
            }
        }
    }

    // Assign a dense net id per union-find root.
    let mut root_net: HashMap<usize, NetId> = HashMap::new();
    let mut node_net: HashMap<(Cell, Port), NetId> = HashMap::new();
    let mut count = 0;
    for (i, &key) in nodes.iter().enumerate() {
        let root = uf.find(i);
        let net = *root_net.entry(root).or_insert_with(|| {
            let id = count;
            count += 1;
            id
        });
        node_net.insert(key, net);
    }

    Nets { count, node_net }
}
