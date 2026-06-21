//! Ready-made boards: the editor's starter content and the test suite's
//! fixtures in one place. Each one is small enough to read by hand and is
//! exercised by a behavioural test in `tests/`.

use crate::board::{Board, Cell};
use crate::tile::{Direction, GateKind, Tile};

/// Set a tile on layer 0.
fn put(board: &mut Board, x: u32, y: u32, tile: Tile) {
    board.set(Cell::new(0, x, y), tile);
}

/// Lay a run of plain wire on layer 0.
fn wires(board: &mut Board, cells: &[(u32, u32)]) {
    for &(x, y) in cells {
        put(board, x, y, Tile::Wire);
    }
}

/// Two switches into an AND gate into an LED — the canonical first circuit.
///
/// ```text
///   B↓
///   [&]→─◧     (& = AND facing east, ◧ = LED)
///   A↑
/// ```
pub fn and_gate() -> Board {
    two_input(GateKind::And)
}

/// Same shape as [`and_gate`] but any gate kind, so one layout drives the whole
/// truth-table test matrix.
pub fn two_input(kind: GateKind) -> Board {
    let mut board = Board::new(1, 5, 3);
    put(&mut board, 2, 1, Tile::Gate { kind, facing: Direction::East });
    put(&mut board, 2, 2, Tile::Switch); // input A, north side
    put(&mut board, 2, 0, Tile::Switch); // input B, south side
    put(&mut board, 3, 1, Tile::Wire); // output stub
    put(&mut board, 4, 1, Tile::Led);
    board
}

/// The two switch cells of a [`two_input`] board, in `(A, B)` order.
pub fn two_input_switches() -> (Cell, Cell) {
    (Cell::new(0, 2, 2), Cell::new(0, 2, 0))
}

/// The LED cell of a [`two_input`] board.
pub fn two_input_led() -> Cell {
    Cell::new(0, 4, 1)
}

/// A single inverter feeding its own input through a wire loop: the output
/// flips every tick, the smallest possible oscillator and a direct test that
/// feedback is legal under the register lowering.
pub fn ring_oscillator() -> Board {
    let mut board = Board::new(1, 3, 3);
    put(&mut board, 1, 1, Tile::Gate { kind: GateKind::Not, facing: Direction::East });
    // Output at (2,1) loops back around the bottom into the gate's west input.
    wires(&mut board, &[(2, 1), (2, 0), (1, 0), (0, 0), (0, 1)]);
    board
}

/// A free-running clock straight into an LED — the simplest "something moves"
/// board, and a test that [`Tile::Clock`] toggles.
pub fn clock_blinker() -> Board {
    let mut board = Board::new(1, 3, 1);
    put(&mut board, 0, 0, Tile::Clock);
    put(&mut board, 1, 0, Tile::Wire);
    put(&mut board, 2, 0, Tile::Led);
    board
}

/// Switch cell of [`clock_blinker`]'s neighbour LED.
pub fn clock_blinker_led() -> Cell {
    Cell::new(0, 2, 0)
}

/// A cross-coupled NOR pair: the textbook set/reset latch, and the demo that
/// shows the editor handling true bistable feedback. `S` at (0,1) sets, `R` at
/// (0,7) resets; the `Q`/`Qn` LEDs sit at (0,3)/(0,5). The two feedback buses
/// cross exactly once, through the [`Tile::Cross`] at (3,5).
pub fn sr_latch() -> Board {
    let mut board = Board::new(1, 9, 9);

    // Qn = NOR(S, Q): gate at (1,1) facing east, set switch to its west.
    put(&mut board, 0, 1, Tile::Switch); // S
    put(&mut board, 1, 1, Tile::Gate { kind: GateKind::Nor, facing: Direction::East });
    // Q = NOR(R, Qn): gate at (1,7) facing east, reset switch to its west.
    put(&mut board, 0, 7, Tile::Switch); // R
    put(&mut board, 1, 7, Tile::Gate { kind: GateKind::Nor, facing: Direction::East });

    // Q bus: from the (1,7) gate's output down column 3 and into the (1,1)
    // gate's north input at (1,2).
    wires(&mut board, &[(2, 7), (3, 7), (3, 6), (3, 4), (3, 3), (2, 3), (1, 3), (1, 2)]);
    // Qn bus: from the (1,1) gate's output up and around column 5 into the
    // (1,7) gate's south input at (1,6).
    wires(
        &mut board,
        &[
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 1),
            (5, 2),
            (5, 3),
            (5, 4),
            (5, 5),
            (4, 5),
            (2, 5),
            (1, 5),
            (1, 6),
        ],
    );
    // The single crossing of the two buses: Q runs vertically, Qn horizontally.
    put(&mut board, 3, 5, Tile::Cross);

    // Indicator LEDs tapping each bus.
    put(&mut board, 0, 3, Tile::Led); // Q
    put(&mut board, 0, 5, Tile::Led); // Qn
    board
}

/// Cells the [`sr_latch`] test pokes: `(set, reset, q_led, qn_led)`.
pub fn sr_latch_ports() -> (Cell, Cell, Cell, Cell) {
    (Cell::new(0, 0, 1), Cell::new(0, 0, 7), Cell::new(0, 0, 3), Cell::new(0, 0, 5))
}

/// The headline feature in miniature: a signal leaves a switch on layer 0,
/// climbs a via stack to layer 1, and lights an LED there. Proof that nets span
/// layers — something VCB's flat board cannot express.
pub fn via_bridge() -> Board {
    let mut board = Board::new(2, 3, 1);
    board.set(Cell::new(0, 0, 0), Tile::Switch);
    board.set(Cell::new(0, 1, 0), Tile::Wire);
    board.set(Cell::new(0, 2, 0), Tile::Via);
    board.set(Cell::new(1, 2, 0), Tile::Via);
    board.set(Cell::new(1, 1, 0), Tile::Wire);
    board.set(Cell::new(1, 0, 0), Tile::Led);
    board
}

/// Cells the [`via_bridge`] test pokes: `(switch_layer0, led_layer1)`.
pub fn via_bridge_ports() -> (Cell, Cell) {
    (Cell::new(0, 0, 0), Cell::new(1, 0, 0))
}

/// Every demo paired with a display name, for the editor's "load example" cycle.
pub fn gallery() -> Vec<(&'static str, Board)> {
    vec![
        ("AND gate", and_gate()),
        ("Ring oscillator", ring_oscillator()),
        ("Clock blinker", clock_blinker()),
        ("SR latch", sr_latch()),
        ("Multi-layer via bridge", via_bridge()),
    ]
}
