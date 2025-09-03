use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct Task {
    future: Pin<Box <dyn Future<Output = ()> + Send>>,
    id: usize,
}

impl Task {
    pub fn new (future: Pin<Box<dyn Future<Output = ()> + Send>>, id: usize) -> Self {
        Task {
            future,
            id
        }
    }

    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.future.as_mut().poll(cx)
    }
}