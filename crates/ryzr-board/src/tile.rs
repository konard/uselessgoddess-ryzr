//! What a single grid cell can hold.
//!
//! Tiles split cleanly into three roles, which the compiler treats very
//! differently:
//!
//! * **Conductors** ([`Tile::Wire`], [`Tile::Cross`], [`Tile::Via`]) carry a
//!   signal with no delay. Adjacent conductors merge into one *net* and a net
//!   takes the wired-OR of everything driving it.
//! * **Drivers** ([`Tile::Switch`], [`Tile::Source`], [`Tile::Clock`],
//!   [`Tile::Gate`]) push a value onto the nets they touch. Every driver is
//!   backed by a register, so a signal advances exactly one driver per tick —
//!   the same unit-delay model that makes Virtual Circuit Board's latches and
//!   oscillators tick, and the reason arbitrary feedback topologies stay legal
//!   under ryzr's "no combinational cycles" rule.
//! * **Sinks** ([`Tile::Led`]) only read. They light when any net they touch is
//!   high and exist purely to make a board observable.

/// One of the four in-plane neighbours of a cell.
///
/// `North` is `+y` (up on screen), `East` is `+x`. The mapping is arbitrary but
/// fixed; everything downstream only relies on it being consistent.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Direction {
    North,
    East,
    South,
    West,
}

impl Direction {
    /// All four directions, in clockwise order from north.
    pub const ALL: [Direction; 4] =
        [Direction::North, Direction::East, Direction::South, Direction::West];

    /// In-plane `(dx, dy)` step for this direction.
    pub fn delta(self) -> (i32, i32) {
        match self {
            Direction::North => (0, 1),
            Direction::East => (1, 0),
            Direction::South => (0, -1),
            Direction::West => (-1, 0),
        }
    }

    /// The direction pointing the opposite way.
    pub fn opposite(self) -> Direction {
        match self {
            Direction::North => Direction::South,
            Direction::East => Direction::West,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
        }
    }

    /// The next direction turning clockwise (used when the player rotates a
    /// device under the cursor).
    pub fn rotate_cw(self) -> Direction {
        match self {
            Direction::North => Direction::East,
            Direction::East => Direction::South,
            Direction::South => Direction::West,
            Direction::West => Direction::North,
        }
    }
}

/// The logic a [`Tile::Gate`] performs over the nets on its input sides.
///
/// A gate folds its op across *every* net touching a non-output side, so a
/// three-input AND is as natural as a two-input one. Empty input sets resolve
/// to a defined constant (see [`crate::compile`]): the positive ops fold to low,
/// their negations to high, matching how a real inverter with a floating input
/// still reads high.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GateKind {
    And,
    Or,
    Xor,
    Nand,
    Nor,
    Xnor,
    Buf,
    Not,
}

/// The contents of one cell on one layer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Tile {
    /// Nothing here.
    #[default]
    Empty,
    /// A conductor connecting all four in-plane sides.
    Wire,
    /// Two independent conductors that pass over each other without touching:
    /// north–south is isolated from east–west. The classic wire crossing.
    Cross,
    /// A conductor like [`Tile::Wire`] that *additionally* bridges to a `Via`
    /// directly above or below it, threading a net between layers. This is the
    /// editor's headline feature: real multi-layer routing instead of VCB's
    /// single flat crossing component.
    Via,
    /// A logic gate. It drives the net on its `facing` side from the nets on the
    /// other three sides.
    Gate { kind: GateKind, facing: Direction },
    /// A player-toggleable input. Drives every net it touches with its state.
    Switch,
    /// A constant high or low source. Drives every net it touches.
    Source { on: bool },
    /// A free-running oscillator: drives every net it touches, flipping each
    /// tick (a two-tick period).
    Clock,
    /// An indicator that lights when any net it touches is high. Read-only.
    Led,
}

impl Tile {
    /// Whether this tile merges with neighbouring conductors into a net.
    pub fn is_conductor(self) -> bool {
        matches!(self, Tile::Wire | Tile::Cross | Tile::Via)
    }

    /// Whether this tile pushes a value onto the nets around it.
    pub fn is_driver(self) -> bool {
        matches!(self, Tile::Switch | Tile::Source { .. } | Tile::Clock | Tile::Gate { .. })
    }
}
