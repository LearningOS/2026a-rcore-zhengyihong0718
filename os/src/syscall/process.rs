//! Process management syscalls
use crate::mm::PageTable;
use crate::task::{change_program_brk, exit_current_and_run_next, suspend_current_and_run_next};
use crate::task::{current_syscall_count, current_user_token, with_current_task};
use alloc::vec::Vec;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// Write seconds and microseconds to a checked, possibly cross-page user buffer.
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

/// Read/write a permitted user byte or query the current task's syscall count.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    match trace_request {
        0 | 1 => {
            let table = PageTable::from_token(current_user_token());
            match table.translate_user_va(id, trace_request == 1) {
                Some(pa) => {
                    let ptr = pa.0 as *mut u8;
                    if trace_request == 0 {
                        unsafe { ptr.read_volatile() as isize }
                    } else {
                        unsafe { ptr.write_volatile(data as u8) };
                        0
                    }
                }
                None => -1,
            }
        }
        2 => current_syscall_count(id) as isize,
        _ => -1,
    }
}

/// Map zeroed anonymous pages in the current task's user address space.
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    with_current_task(|task| task.memory_set.map_user_range(start, len, prot))
}

/// Unmap the current task's pages and release their physical frames.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    with_current_task(|task| task.memory_set.unmap_user_range(start, len))
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
