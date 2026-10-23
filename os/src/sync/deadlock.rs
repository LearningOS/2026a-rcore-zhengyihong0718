//! Resource snapshots for the chapter's deadlock detection algorithm.
//!
//! AI-assisted implementation based on the LearningOS chapter 8 exercise.
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

/// Actual free units, held units and outstanding requests of one resource.
#[derive(Clone)]
pub struct ResourceState {
    /// Units immediately available; queued requests never make this negative.
    pub available: usize,
    allocation: BTreeMap<usize, usize>,
    need: BTreeMap<usize, usize>,
}

impl ResourceState {
    /// Create an initially unallocated resource.
    pub fn new(available: usize) -> Self {
        Self {
            available,
            allocation: BTreeMap::new(),
            need: BTreeMap::new(),
        }
    }

    /// Record a pending single-unit request.
    pub fn request(&mut self, tid: usize) {
        self.need.insert(tid, 1);
    }

    /// Forget an exited thread without making its unreleased units available.
    pub fn retire(&mut self, tid: usize) {
        self.need.remove(&tid);
        self.allocation.remove(&tid);
    }

    /// Commit acquisition, including direct handoff to a woken thread.
    pub fn acquire(&mut self, tid: usize) {
        assert!(self.available > 0);
        self.available -= 1;
        self.need.remove(&tid);
        *self.allocation.entry(tid).or_insert(0) += 1;
    }

    /// Release a unit. Semaphore producers may post without owning a unit.
    pub fn release(&mut self, tid: usize) {
        if let Some(count) = self.allocation.get_mut(&tid) {
            *count -= 1;
            if *count == 0 {
                self.allocation.remove(&tid);
            }
        }
        self.available += 1;
    }
}

/// Test a hypothetical request without changing any live synchronization state.
pub fn would_deadlock(mut resources: Vec<ResourceState>, tid: usize, id: usize) -> bool {
    resources[id].request(tid);
    let mut unfinished = BTreeSet::new();
    for resource in &resources {
        unfinished.extend(resource.allocation.keys().copied());
        unfinished.extend(resource.need.keys().copied());
    }
    let mut work: Vec<usize> = resources.iter().map(|r| r.available).collect();
    while let Some(finished) = unfinished.iter().copied().find(|t| {
        resources
            .iter()
            .zip(&work)
            .all(|(r, w)| r.need.get(t).copied().unwrap_or(0) <= *w)
    }) {
        unfinished.remove(&finished);
        for (r, w) in resources.iter().zip(&mut work) {
            *w += r.allocation.get(&finished).copied().unwrap_or(0);
        }
    }
    !unfinished.is_empty()
}
