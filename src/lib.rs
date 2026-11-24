#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
extern crate alloc;

pub mod escrow;
pub mod testing;

#[cfg(test)]
mod tests;
