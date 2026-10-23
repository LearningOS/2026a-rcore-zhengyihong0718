//! Mutex (spin-like and blocking(sleep))

use super::{ResourceState, UPSafeCell};
use crate::task::TaskControlBlock;
use crate::task::{block_current_and_run_next, suspend_current_and_run_next};
use crate::task::{current_task, wakeup_task};
use alloc::{collections::VecDeque, sync::Arc};

/// Mutex trait
pub trait Mutex: Sync + Send {
    /// Snapshot the actual holdings and pending requests for detection.
    fn snapshot(&self) -> ResourceState;
    /// Remove an exited thread from the detector without unlocking its mutex.
    fn retire(&self, tid: usize);
    /// Lock the mutex
    fn lock(&self);
    /// Unlock the mutex
    fn unlock(&self);
}

/// Spinlock Mutex struct
pub struct MutexSpin {
    state: UPSafeCell<ResourceState>,
    owner: UPSafeCell<Option<usize>>,
}

impl MutexSpin {
    /// Create a new spinlock mutex
    pub fn new() -> Self {
        Self {
            state: unsafe { UPSafeCell::new(ResourceState::new(1)) },
            owner: unsafe { UPSafeCell::new(None) },
        }
    }
}

impl Mutex for MutexSpin {
    fn retire(&self, tid: usize) {
        self.state.exclusive_access().retire(tid);
    }
    fn snapshot(&self) -> ResourceState {
        self.state.exclusive_access().clone()
    }
    /// Lock the spinlock mutex
    fn lock(&self) {
        trace!("kernel: MutexSpin::lock");
        let tid = current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid;
        self.state.exclusive_access().request(tid);
        loop {
            let mut state = self.state.exclusive_access();
            if state.available == 0 {
                drop(state);
                suspend_current_and_run_next();
                continue;
            } else {
                state.acquire(tid);
                *self.owner.exclusive_access() = Some(tid);
                return;
            }
        }
    }

    fn unlock(&self) {
        trace!("kernel: MutexSpin::unlock");
        if let Some(tid) = self.owner.exclusive_access().take() {
            self.state.exclusive_access().release(tid);
        }
    }
}

/// Blocking Mutex struct
pub struct MutexBlocking {
    inner: UPSafeCell<MutexBlockingInner>,
}

pub struct MutexBlockingInner {
    locked: bool,
    wait_queue: VecDeque<Arc<TaskControlBlock>>,
    state: ResourceState,
    owner: Option<usize>,
}

impl MutexBlocking {
    /// Create a new blocking mutex
    pub fn new() -> Self {
        trace!("kernel: MutexBlocking::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(MutexBlockingInner {
                    locked: false,
                    wait_queue: VecDeque::new(),
                    state: ResourceState::new(1),
                    owner: None,
                })
            },
        }
    }
}

impl Mutex for MutexBlocking {
    fn retire(&self, tid: usize) {
        self.inner.exclusive_access().state.retire(tid);
    }
    fn snapshot(&self) -> ResourceState {
        self.inner.exclusive_access().state.clone()
    }
    /// lock the blocking mutex
    fn lock(&self) {
        trace!("kernel: MutexBlocking::lock");
        let mut mutex_inner = self.inner.exclusive_access();
        let tid = current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid;
        mutex_inner.state.request(tid);
        if mutex_inner.locked {
            mutex_inner.wait_queue.push_back(current_task().unwrap());
            drop(mutex_inner);
            block_current_and_run_next();
        } else {
            mutex_inner.locked = true;
            mutex_inner.state.acquire(tid);
            mutex_inner.owner = Some(tid);
        }
    }

    /// unlock the blocking mutex
    fn unlock(&self) {
        trace!("kernel: MutexBlocking::unlock");
        let mut mutex_inner = self.inner.exclusive_access();
        assert!(mutex_inner.locked);
        let owner = mutex_inner.owner.take().unwrap();
        mutex_inner.state.release(owner);
        if let Some(waking_task) = mutex_inner.wait_queue.pop_front() {
            let tid = waking_task
                .inner_exclusive_access()
                .res
                .as_ref()
                .unwrap()
                .tid;
            mutex_inner.state.acquire(tid);
            mutex_inner.owner = Some(tid);
            wakeup_task(waking_task);
        } else {
            mutex_inner.locked = false;
        }
    }
}
