//! Time syscall isolated for actual-source host verification.
use crate::{mm::PageTable, task::current_user_token};
use alloc::vec::Vec;
#[repr(C)]
#[derive(Debug)]
/// User ABI time value in seconds and microseconds.
pub struct TimeVal {
    /// Whole seconds since the platform clock origin.
    pub sec: usize,
    /// Microseconds within the current second.
    pub usec: usize,
}

/// Write the current time to user memory.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    // AI assistance: Codex. Validate every destination before writing any byte.
    // Each byte is translated independently, so TimeVal may straddle two pages.
    let table = PageTable::from_token(current_user_token());
    let mut destinations = Vec::new();
    for offset in 0..core::mem::size_of::<TimeVal>() {
        let addr = match (ts as usize).checked_add(offset) {
            Some(addr) => addr,
            None => return -1,
        };
        match table.translate_user_va(addr, true) {
            Some(pa) => destinations.push(pa.0 as *mut u8),
            None => return -1,
        }
    }
    let us = crate::timer::get_time_us();
    let time = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &time as *const TimeVal as *const u8,
            core::mem::size_of::<TimeVal>(),
        )
    };
    for (ptr, &byte) in destinations.into_iter().zip(bytes) {
        unsafe {
            ptr.write_volatile(byte);
        }
    }
    0
}
