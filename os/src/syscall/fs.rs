//! File and filesystem-related syscalls
// Codex assisted fstat/linkat/unlinkat according to the official chapter 6
// exercise and inspected the pinned official ch6_file0..3 tests for ABI behavior.
use crate::fs::{open_file, OpenFlags, Stat, ROOT_INODE};
use crate::mm::{translated_byte_buffer, translated_str, PageTable, UserBuffer, VirtAddr};
use crate::task::{current_task, current_user_token};

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let path = translated_str(token, path);
    let flags = match OpenFlags::from_bits(flags) {
        Some(flags) => flags,
        None => return -1,
    };
    if let Some(inode) = open_file(path.as_str(), flags) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

/// Copy metadata across page boundaries only after checking the whole buffer.
pub fn sys_fstat(fd: usize, st: *mut Stat) -> isize {
    let task = current_task().unwrap();
    let file = {
        let inner = task.inner_exclusive_access();
        match inner.fd_table.get(fd).and_then(|entry| entry.clone()) {
            Some(file) => file,
            None => return -1,
        }
    };
    let stat = match file.stat() {
        Some(stat) => stat,
        None => return -1,
    };
    let token = current_user_token();
    let len = core::mem::size_of::<Stat>();
    if !user_range(token, st as usize, len, true) {
        return -1;
    }
    // repr(C) Stat consists of u64/u32 fields with every byte initialized.
    let bytes = unsafe { core::slice::from_raw_parts(&stat as *const Stat as *const u8, len) };
    let mut offset = 0;
    for buffer in translated_byte_buffer(token, st as *const u8, len) {
        let end = offset + buffer.len();
        buffer.copy_from_slice(&bytes[offset..end]);
        offset = end;
    }
    0
}

/// Add a hard link in the filesystem's flat root namespace.
pub fn sys_linkat(old_name: *const u8, new_name: *const u8) -> isize {
    let token = current_user_token();
    let old = match file_name(token, old_name) {
        Some(name) => name,
        None => return -1,
    };
    let new = match file_name(token, new_name) {
        Some(name) => name,
        None => return -1,
    };
    if ROOT_INODE.link(&old, &new) {
        0
    } else {
        -1
    }
}

/// Remove a directory entry; open file descriptions retain the inode.
pub fn sys_unlinkat(name: *const u8) -> isize {
    let name = match file_name(current_user_token(), name) {
        Some(name) => name,
        None => return -1,
    };
    if ROOT_INODE.unlink(&name) {
        0
    } else {
        -1
    }
}

fn user_range(token: usize, start: usize, len: usize, write: bool) -> bool {
    // Sv39 user addresses occupy the lower canonical half.
    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return false,
    };
    if start == 0 || end > (1usize << 38) {
        return false;
    }
    let table = PageTable::from_token(token);
    let mut address = start;
    while address < end {
        match table.translate(VirtAddr::from(address).floor()) {
            Some(pte)
                if pte.is_valid()
                    && pte.flags().bits() & (1 << 4) != 0
                    && (!pte.writable() || pte.readable())
                    && if write {
                        pte.writable()
                    } else {
                        pte.readable()
                    } => {}
            _ => return false,
        }
        address = ((address / 4096 + 1) * 4096).min(end);
    }
    true
}

// Limit the walk to a single flat-directory name plus its terminating NUL.
fn file_name(token: usize, ptr: *const u8) -> Option<alloc::string::String> {
    let table = PageTable::from_token(token);
    let mut bytes = alloc::vec::Vec::new();
    for offset in 0..28 {
        let address = (ptr as usize).checked_add(offset)?;
        if !user_range(token, address, 1, false) {
            return None;
        }
        let byte = *table.translate_va(VirtAddr::from(address))?.get_ref::<u8>();
        if byte == 0 {
            return alloc::string::String::from_utf8(bytes).ok();
        }
        bytes.push(byte);
    }
    None
}
