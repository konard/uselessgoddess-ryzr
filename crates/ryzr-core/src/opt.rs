//! Observable-equivalence netlist optimizer: constant folding, algebraic
//! simplification, copy propagation, common-subexpression elimination and
//! dead code elimination over a finished [`Circuit`].
//!
//! Declared outputs and register next-state functions are bit-for-bit
//! preserved on every tick; internal nodes may be rewritten, merged or
//! removed. This is the same "as-if" contract a compiling RTL simulator
//! (e.g. Verilator) operates under, and the differential test suite checks
//! optimized engines against the naive interpreter on the original netlist.
//!
//! Rewrites keep creation order: node ids are assigned in original signal
//! order (plus locally inserted inverters), so word-shaped structures stay
//! contiguous for the backends' packed layouts.

use alloc::vec::Vec;

use cranelift_entity::{EntityList, EntityRef, ListPool, PrimaryMap};

use crate::HashMap;
use crate::circuit::{Circuit, GateOp, InstData, Instruction, Reg, Register, Signal};

/// Node in the rewritten graph; ids are topological by construction.
#[derive(Clone, Copy)]
enum Node {
    Const(bool),
    Input(u32),
    RegOut(u32),
    Gate { op: GateOp, ins: [u32; 3], arity: u8 },
}

#[derive(Default)]
struct Rewriter {
    nodes: Vec<Node>,
    cse: HashMap<(u8, [u32; 3]), u32>,
    consts: [Option<u32>; 2],
}

impl Rewriter {
    fn push(&mut self, node: Node) -> u32 {
        self.nodes.push(node);
        self.nodes.len() as u32 - 1
    }

    fn cval(&self, id: u32) -> Option<bool> {
        match self.nodes[id as usize] {
            Node::Const(v) => Some(v),
            _ => None,
        }
    }

    fn konst(&mut self, value: bool) -> u32 {
        match self.consts[value as usize] {
            Some(id) => id,
            None => {
                let id = self.push(Node::Const(value));
                self.consts[value as usize] = Some(id);
                id
            }
        }
    }

    /// Emit a gate with structural hashing; commutative operands are
    /// canonicalized so `and(a, b)` and `and(b, a)` share one node.
    fn gate(&mut self, op: GateOp, mut ins: [u32; 3], arity: u8) -> u32 {
        let commutative = matches!(
            op,
            GateOp::And | GateOp::Or | GateOp::Xor | GateOp::Nand | GateOp::Nor | GateOp::Xnor
        );
        if commutative && ins[0] > ins[1] {
            ins.swap(0, 1);
        }
        if let Some(&id) = self.cse.get(&(op as u8, ins)) {
            return id;
        }
        let id = self.push(Node::Gate { op, ins, arity });
        self.cse.insert((op as u8, ins), id);
        id
    }

    /// True iff one operand is the inversion of the other.
    fn complements(&self, a: u32, b: u32) -> bool {
        let inverts = |x: u32, y: u32| matches!(self.nodes[x as usize], Node::Gate { op: GateOp::Not, ins, .. } if ins[0] == y);
        inverts(a, b) || inverts(b, a)
    }

    fn not(&mut self, x: u32) -> u32 {
        if let Some(v) = self.cval(x) {
            return self.konst(!v);
        }
        if let Node::Gate { op: GateOp::Not, ins, .. } = self.nodes[x as usize] {
            return ins[0];
        }
        self.gate(GateOp::Not, [x, 0, 0], 1)
    }

    fn and(&mut self, a: u32, b: u32) -> u32 {
        match (self.cval(a), self.cval(b)) {
            (Some(false), _) | (_, Some(false)) => self.konst(false),
            (Some(true), _) => b,
            (_, Some(true)) => a,
            _ if a == b => a,
            _ if self.complements(a, b) => self.konst(false),
            _ => self.gate(GateOp::And, [a, b, 0], 2),
        }
    }

    fn or(&mut self, a: u32, b: u32) -> u32 {
        match (self.cval(a), self.cval(b)) {
            (Some(true), _) | (_, Some(true)) => self.konst(true),
            (Some(false), _) => b,
            (_, Some(false)) => a,
            _ if a == b => a,
            _ if self.complements(a, b) => self.konst(true),
            _ => self.gate(GateOp::Or, [a, b, 0], 2),
        }
    }

    fn xor(&mut self, a: u32, b: u32) -> u32 {
        match (self.cval(a), self.cval(b)) {
            (Some(va), Some(vb)) => self.konst(va ^ vb),
            (Some(false), _) => b,
            (_, Some(false)) => a,
            (Some(true), _) => self.not(b),
            (_, Some(true)) => self.not(a),
            _ if a == b => self.konst(false),
            _ if self.complements(a, b) => self.konst(true),
            _ => self.gate(GateOp::Xor, [a, b, 0], 2),
        }
    }

    fn nand(&mut self, a: u32, b: u32) -> u32 {
        match (self.cval(a), self.cval(b)) {
            (Some(false), _) | (_, Some(false)) => self.konst(true),
            (Some(true), _) => self.not(b),
            (_, Some(true)) => self.not(a),
            _ if a == b => self.not(a),
            _ if self.complements(a, b) => self.konst(true),
            _ => self.gate(GateOp::Nand, [a, b, 0], 2),
        }
    }

    fn nor(&mut self, a: u32, b: u32) -> u32 {
        match (self.cval(a), self.cval(b)) {
            (Some(true), _) | (_, Some(true)) => self.konst(false),
            (Some(false), _) => self.not(b),
            (_, Some(false)) => self.not(a),
            _ if a == b => self.not(a),
            _ if self.complements(a, b) => self.konst(false),
            _ => self.gate(GateOp::Nor, [a, b, 0], 2),
        }
    }

    fn xnor(&mut self, a: u32, b: u32) -> u32 {
        match (self.cval(a), self.cval(b)) {
            (Some(va), Some(vb)) => self.konst(va == vb),
            (Some(true), _) => b,
            (_, Some(true)) => a,
            (Some(false), _) => self.not(b),
            (_, Some(false)) => self.not(a),
            _ if a == b => self.konst(true),
            _ if self.complements(a, b) => self.konst(false),
            _ => self.gate(GateOp::Xnor, [a, b, 0], 2),
        }
    }

    fn mux(&mut self, s: u32, t: u32, e: u32) -> u32 {
        if let Some(vs) = self.cval(s) {
            return if vs { t } else { e };
        }
        if t == e {
            return t;
        }
        match (self.cval(t), self.cval(e)) {
            (Some(true), Some(false)) => s,
            (Some(false), Some(true)) => self.not(s),
            (Some(true), _) => self.or(s, e),
            (Some(false), _) => {
                let ns = self.not(s);
                self.and(ns, e)
            }
            (_, Some(false)) => self.and(s, t),
            (_, Some(true)) => {
                let ns = self.not(s);
                self.or(ns, t)
            }
            _ if s == t => self.or(s, e),
            _ if s == e => self.and(s, t),
            _ => self.gate(GateOp::Mux, [s, t, e], 3),
        }
    }
}

/// Rewrite `circuit` into an observably equivalent, usually smaller one.
/// Inputs keep their indices and registers keep their ids and initial
/// values, so engine-facing semantics (poke/peek, state vectors) and the
/// declared outputs are unchanged.
pub fn optimize(circuit: &Circuit) -> Circuit {
    let mut rw = Rewriter::default();

    // Forward rewrite in signal order; ids are topological so every operand
    // already has a canonical representative.
    let mut repr: Vec<u32> = Vec::with_capacity(circuit.insts.len());
    for (_, inst) in circuit.insts.iter() {
        let id = match &inst.data {
            InstData::Const { value } => rw.konst(*value),
            InstData::Input { index } => rw.push(Node::Input(*index)),
            InstData::RegisterOutput { reg } => rw.push(Node::RegOut(reg.index() as u32)),
            InstData::Gate { op, inputs } => {
                let ins = inputs.as_slice(&circuit.list_pool);
                let r = |i: usize| repr[ins[i].index()];
                match op {
                    GateOp::Buf => r(0),
                    GateOp::Not => {
                        let x = r(0);
                        rw.not(x)
                    }
                    GateOp::And => {
                        let (a, b) = (r(0), r(1));
                        rw.and(a, b)
                    }
                    GateOp::Or => {
                        let (a, b) = (r(0), r(1));
                        rw.or(a, b)
                    }
                    GateOp::Xor => {
                        let (a, b) = (r(0), r(1));
                        rw.xor(a, b)
                    }
                    GateOp::Nand => {
                        let (a, b) = (r(0), r(1));
                        rw.nand(a, b)
                    }
                    GateOp::Nor => {
                        let (a, b) = (r(0), r(1));
                        rw.nor(a, b)
                    }
                    GateOp::Xnor => {
                        let (a, b) = (r(0), r(1));
                        rw.xnor(a, b)
                    }
                    GateOp::Mux => {
                        let (s, t, e) = (r(0), r(1), r(2));
                        rw.mux(s, t, e)
                    }
                }
            }
        };
        repr.push(id);
    }

    // Liveness from the observable roots: declared outputs and register
    // next-state inputs. Inputs and register outputs always survive so the
    // engine-facing interface keeps its shape.
    let mut live = vec![false; rw.nodes.len()];
    let mut stack: Vec<u32> = Vec::new();
    for &out in &circuit.output_signals {
        stack.push(repr[out.index()]);
    }
    for (_, reg) in circuit.regs.iter() {
        stack.push(repr[reg.data_input.index()]);
    }
    while let Some(id) = stack.pop() {
        if core::mem::replace(&mut live[id as usize], true) {
            continue;
        }
        if let Node::Gate { ins, arity, .. } = rw.nodes[id as usize] {
            stack.extend(&ins[..arity as usize]);
        }
    }
    for (id, node) in rw.nodes.iter().enumerate() {
        if matches!(node, Node::Input(_) | Node::RegOut(_)) {
            live[id] = true;
        }
    }

    // Emit the surviving nodes in id order (still topological, still in
    // creation order for everything that came from the original netlist).
    let mut insts = PrimaryMap::with_capacity(rw.nodes.len());
    let mut list_pool = ListPool::new();
    let mut remap = vec![Signal::new(0); rw.nodes.len()];
    for (id, node) in rw.nodes.iter().enumerate() {
        if !live[id] {
            continue;
        }
        let (data, name) = match *node {
            Node::Const(value) => (InstData::Const { value }, None),
            Node::Input(index) => (InstData::Input { index }, None),
            Node::RegOut(reg) => {
                let reg = Reg::new(reg as usize);
                (InstData::RegisterOutput { reg }, circuit.regs[reg].name_index)
            }
            Node::Gate { op, ins, arity } => {
                let mut inputs = EntityList::new();
                for &input in &ins[..arity as usize] {
                    inputs.push(remap[input as usize], &mut list_pool);
                }
                (InstData::Gate { op, inputs }, None)
            }
        };
        remap[id] = insts.push(Instruction { data, debug_name_index: name });
    }

    let mut regs = PrimaryMap::with_capacity(circuit.regs.len());
    for (_, reg) in circuit.regs.iter() {
        regs.push(Register {
            data_input: remap[repr[reg.data_input.index()] as usize],
            initial: reg.initial,
            name_index: reg.name_index,
        });
    }

    let output_signals =
        circuit.output_signals.iter().map(|s| remap[repr[s.index()] as usize]).collect();

    Circuit {
        insts,
        regs,
        input_names: circuit.input_names.clone(),
        output_names: circuit.output_names.clone(),
        output_signals,
        debug_names: circuit.debug_names.clone(),
        list_pool,
        input_count: circuit.input_count,
        register_count: circuit.register_count,
        output_count: circuit.output_count,
    }
}
