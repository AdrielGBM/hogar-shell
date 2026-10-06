//! Serials for telling the latest of something from the ones before it.

use std::cell::Cell;

/// Moves `serial` on and answers with the new value; it wraps instead of overflowing.
pub fn next_serial(serial: &Cell<u64>) -> u64 {
    let next = serial.get().wrapping_add(1);
    serial.set(next);
    next
}
