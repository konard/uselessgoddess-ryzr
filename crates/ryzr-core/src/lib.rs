#![no_std]

#[cfg_attr(not(feature = "std"), macro_use)]
extern crate alloc;

#[cfg(feature = "std")]
#[macro_use]
extern crate std;

#[allow(unused_imports)]
#[cfg(feature = "std")]
use std::collections::{HashMap, HashSet, hash_map};

#[cfg(not(feature = "std"))]
use hashbrown::{HashMap, HashSet, hash_map};

mod circuit;

pub use circuit::{Circuit, CircuitBuilder, Reg, Register, Signal};
