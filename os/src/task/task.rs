//! Types related to task management

use super::TaskContext;

/// The task control block (TCB) of a task.
#[derive(Copy, Clone)]
pub struct TaskControlBlock {
    /// The task status in it's lifecycle
    pub task_status: TaskStatus,
    /// The task context
    pub task_cx: TaskContext,
    /// Per-task counts for the syscalls supported by this chapter.
    pub syscall_times: [usize; 5],
}

impl TaskControlBlock {
    // AI assistance: Codex. Contract: rCore-Tutorial-Guide, chapter3/5exercise.
    // Only supported calls can return to userspace; other IDs have count zero.
    pub fn record_syscall(&mut self, id: usize) {
        if let Some(index) = crate::syscall::SYSCALL_IDS
            .iter()
            .position(|&value| value == id)
        {
            self.syscall_times[index] = self.syscall_times[index].saturating_add(1);
        }
    }

    pub fn syscall_count(&self, id: usize) -> usize {
        crate::syscall::SYSCALL_IDS
            .iter()
            .position(|&value| value == id)
            .map_or(0, |index| self.syscall_times[index])
    }
}

/// The status of a task
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Exited,
}
