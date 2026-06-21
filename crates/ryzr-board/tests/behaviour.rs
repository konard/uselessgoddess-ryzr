//! Behavioural tests over the demo boards.
//!
//! Each one builds a [`demos`] board, runs the simulation through [`Sim`], and
//! asserts the logical behaviour — the reproducing tests that pin the net
//! extraction, the register-based unit-delay lowering, and multi-layer vias.

use ryzr_board::{Cell, GateKind, Sim, demos};

/// Enough ticks for any demo here to reach a steady state.
const SETTLE: u64 = 8;

/// Build a two-input board of `kind`, drive the switches, and read the LED.
fn two_input(kind: GateKind, a: bool, b: bool) -> bool {
    let board = demos::two_input(kind);
    let (sw_a, sw_b) = demos::two_input_switches();
    let led = demos::two_input_led();

    let mut sim = Sim::new(&board).expect("two-input board compiles");
    sim.set_switch(sw_a, a);
    sim.set_switch(sw_b, b);
    sim.run(SETTLE);
    sim.led_on(led)
}

#[test]
fn two_input_truth_tables() {
    let cases: &[(GateKind, fn(bool, bool) -> bool)] = &[
        (GateKind::And, |a, b| a && b),
        (GateKind::Or, |a, b| a || b),
        (GateKind::Xor, |a, b| a ^ b),
        (GateKind::Nand, |a, b| !(a && b)),
        (GateKind::Nor, |a, b| !(a || b)),
        (GateKind::Xnor, |a, b| !(a ^ b)),
    ];

    for &(kind, expected) in cases {
        for a in [false, true] {
            for b in [false, true] {
                assert_eq!(two_input(kind, a, b), expected(a, b), "{kind:?} with a={a}, b={b}",);
            }
        }
    }
}

#[test]
fn ring_oscillator_has_period_two() {
    let board = demos::ring_oscillator();
    let gate = Cell::new(0, 1, 1);

    let mut sim = Sim::new(&board).expect("ring compiles");
    sim.tick();
    let s0 = sim.cell_on(gate);
    sim.tick();
    let s1 = sim.cell_on(gate);
    sim.tick();
    let s2 = sim.cell_on(gate);

    assert_ne!(s0, s1, "an inverter fed from its own output must flip each tick");
    assert_eq!(s0, s2, "and return after two ticks");
}

#[test]
fn clock_blinks() {
    let board = demos::clock_blinker();
    let led = demos::clock_blinker_led();

    let mut sim = Sim::new(&board).expect("clock compiles");
    sim.tick();
    let first = sim.led_on(led);
    sim.tick();
    let second = sim.led_on(led);

    assert_ne!(first, second, "a clock must toggle its LED every tick");
}

#[test]
fn sr_latch_sets_resets_and_holds() {
    let board = demos::sr_latch();
    let (set, reset, q, qn) = demos::sr_latch_ports();
    let mut sim = Sim::new(&board).expect("latch compiles");

    // Pulse set: Q goes high.
    sim.set_switch(set, true);
    sim.run(SETTLE);
    assert!(sim.led_on(q), "S should set Q");
    assert!(!sim.led_on(qn), "Qn should be low while Q is set");

    // Release set: the latch holds.
    sim.set_switch(set, false);
    sim.run(SETTLE);
    assert!(sim.led_on(q), "Q must latch after S is released");
    assert!(!sim.led_on(qn));

    // Pulse reset: Q goes low.
    sim.set_switch(reset, true);
    sim.run(SETTLE);
    assert!(!sim.led_on(q), "R should reset Q");
    assert!(sim.led_on(qn), "Qn should be high while Q is reset");

    // Release reset: the latch holds the reset state.
    sim.set_switch(reset, false);
    sim.run(SETTLE);
    assert!(!sim.led_on(q), "Q must stay reset after R is released");
    assert!(sim.led_on(qn));
}

#[test]
fn via_bridges_layers() {
    let board = demos::via_bridge();
    let (switch, led) = demos::via_bridge_ports();
    let mut sim = Sim::new(&board).expect("via bridge compiles");

    sim.run(SETTLE);
    assert!(!sim.led_on(led), "LED on layer 1 is dark until driven");

    sim.set_switch(switch, true);
    sim.run(SETTLE);
    assert!(sim.led_on(led), "a switch on layer 0 lights an LED on layer 1 through the via stack");

    sim.set_switch(switch, false);
    sim.run(SETTLE);
    assert!(!sim.led_on(led), "and goes dark again");
}

#[test]
fn every_demo_compiles() {
    for (name, board) in demos::gallery() {
        if let Err(error) = Sim::new(&board) {
            panic!("demo {name:?} failed to compile: {error}");
        }
    }
}
