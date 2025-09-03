use std::future::Future;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::pin::Pin;
use std::sync::mpsc;
use std::collections::VecDeque;

use crate::runtime::task::Task;

/// A very simple executor
pub struct SmarterExecutor {
    wake_receiver: mpsc::Receiver<()>,
    wake_sender: mpsc::Sender<()>,
    task_queue: VecDeque<Task>,
    next_task_id: usize,
}

impl SmarterExecutor {
    pub fn new() -> Self {
        let (wake_sender, wake_receiver) = mpsc::channel();
        SmarterExecutor {
            wake_receiver,
            wake_sender,
            task_queue: VecDeque::new(),
            next_task_id: 0,
        }
    }
    
    /// Run a future to completion, sleeping until woken up
    pub fn block_on<F: Future>(&mut self, mut future: F) -> F::Output {
        // Pin the future to memory so we can poll it
        let mut future = unsafe { Pin::new_unchecked(&mut future) };
        
        loop {
            // create a waker that sends a message to wake us up
            let waker = self.create_waker();
            let mut context = Context::from_waker(&waker);

            match future.as_mut().poll(&mut context) {
                Poll::Ready(result) => return result,
                Poll::Pending => {
                    // instead of busy polling, sleep until something wake us
                    let _ = self.wake_receiver.recv();
                }
            }
        }
    }

    pub fn spawn<F>(&mut self, future: F)
    where 
    F: Future<Output = ()> + Send + 'static
    {
        let task = Task::new(Box::pin(future), self.next_task_id);
        self.task_queue.push_back(task);
        self.next_task_id += 1;
    }

    // run all tasks to completion
    pub fn run(&mut self) {
        while !self.task_queue.is_empty() {
            self.poll_tasks();

            if !self.task_queue.is_empty() {
                let _ = self.wake_receiver.recv();
            }
        }
    }

    fn poll_tasks(&mut self) {
        let mut remaining_tasks = VecDeque::new();

        while let Some(mut task) = self.task_queue.pop_front() {
            let waker = self.create_waker();
            let mut cx = Context::from_waker(&waker);

            match task.poll(&mut cx) {
                Poll::Ready(()) => {
                    // Task completed
                },
                Poll::Pending => {
                    remaining_tasks.push_back(task);
                }
            }
        }
        self.task_queue = remaining_tasks;
    }
    // create a waker that will wake up this executor
    fn create_waker(&self) -> Waker {
        let sender = self.wake_sender.clone();
        let raw_waker = create_simple_waker(sender);
        unsafe { Waker::from_raw(raw_waker) }
    }
}

// Simple working waker based on async-rust-book-server
static SIMPLE_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    simple_clone,
    simple_wake,
    simple_wake_by_ref,
    simple_drop,
);

unsafe fn simple_clone(data: *const ()) -> RawWaker {
    unsafe {
        // Get the sender, clone it, but don't consume the original
        let sender = &*(data as *const mpsc::Sender<()>);
        let cloned_sender = sender.clone();
        
        // Create new Box with the cloned sender
        let new_data = Box::into_raw(Box::new(cloned_sender));
        RawWaker::new(new_data as *const (), &SIMPLE_WAKER_VTABLE)
    }
}

unsafe fn simple_wake(data: *const ()) {
    unsafe {
        // Get the channel sender and send wake signal
        let sender = Box::from_raw(data as *mut mpsc::Sender<()>);
        let _ = sender.send(());
    }
}

unsafe fn simple_wake_by_ref(data: *const ()) {
    unsafe {
        // Get the channel sender, send signal, but don't consume the Box
        let sender = &*(data as *const mpsc::Sender<()>);
        let _ = sender.send(());
        // Box stays alive - we're just borrowing it
    }
}

unsafe fn simple_drop(data: *const ()) {
    unsafe {
        // Clean up the sender
        drop(Box::from_raw(data as *mut mpsc::Sender<()>));
    }
}

fn create_simple_waker(sender: mpsc::Sender<()>) -> RawWaker {
    let data = Box::into_raw(Box::new(sender));
    RawWaker::new(data as *const (), &SIMPLE_WAKER_VTABLE)
}