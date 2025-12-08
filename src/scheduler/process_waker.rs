use alloc::sync::Arc;
use alloc::task::Wake;
use core::task::Waker;

use crate::scheduler;
use crate::process;

pub struct ProcessWaker {
    task_id: u64,
}

impl ProcessWaker {
    pub fn new(task_id: u64) -> Waker {
	Waker::from(Arc::new(Self {
	    task_id: task_id,
	}))
    }
}

impl Wake for ProcessWaker {
    fn wake(self: Arc<Self>) {
	let process = match scheduler::get_process_by_id(self.task_id) {
	    Some(p) => p,
	    None => return, // Return on error, as the task has exited before the waker was called
	};

	match process.clone().get_state() {
	    process::TaskState::Waiting { future } => {
		process.clone().set_state(process::TaskState::AsyncSyscall {
		    future,
		});
	    },
	    process::TaskState::AsyncSyscall { future: _ } => (),  // No need to do anything, we're already in the right state
	    _ => (),  // Waker is expired, skip
	}
    }
}
