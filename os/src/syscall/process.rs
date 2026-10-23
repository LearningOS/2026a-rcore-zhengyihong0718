//! Process management syscalls
//!
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::{
    fs::{open_file, OpenFlags},
    mm::{translated_refmut, translated_str, PageTable},
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next,
    },
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    //trace!("kernel: sys_waitpid");
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
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

/// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    let task = current_task().unwrap();
    let result = task
        .inner_exclusive_access()
        .memory_set
        .map_user_range(start, len, prot);
    result
}

/// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    let task = current_task().unwrap();
    let result = task
        .inner_exclusive_access()
        .memory_set
        .unmap_user_range(start, len);
    result
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// YOUR JOB: Implement spawn.
/// HINT: fork + exec =/= spawn
pub fn sys_spawn(path: *const u8) -> isize {
    // AI assistance: Codex; create the target ELF directly, without fork's copy.
    let table = PageTable::from_token(current_user_token());
    let mut bytes = Vec::new();
    let mut name = None;
    for offset in 0..4096 {
        let addr = match (path as usize).checked_add(offset) {
            Some(addr) => addr,
            None => return -1,
        };
        let pa = match table.translate_user_va(addr, false) {
            Some(pa) => pa,
            None => return -1,
        };
        let byte = unsafe { (pa.0 as *const u8).read_volatile() };
        if byte == 0 {
            name = String::from_utf8(bytes).ok();
            break;
        }
        bytes.push(byte);
    }
    let name = match name {
        Some(name) => name,
        None => return -1,
    };
    let inode = match open_file(name.as_str(), OpenFlags::RDONLY) {
        Some(inode) => inode,
        None => return -1,
    };
    let data = inode.read_all();
    let parent = current_task().unwrap();
    let child = Arc::new(crate::task::TaskControlBlock::new(data.as_slice()));
    let pid = child.getpid();
    {
        let files = parent.inner_exclusive_access().fd_table.clone();
        let mut inner = child.inner_exclusive_access();
        inner.parent = Some(Arc::downgrade(&parent));
        inner.fd_table = files;
    }
    parent.inner_exclusive_access().children.push(child.clone());
    add_task(child);
    pid as isize
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(prio: isize) -> isize {
    if prio < 2 {
        return -1;
    }
    current_task().unwrap().inner_exclusive_access().priority = prio as usize;
    prio
}
