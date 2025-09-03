use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Mutex::new(ChannelState {
        buffer: VecDeque::with_capacity(capacity),
        capacity,
        receiver_waker: None,
        sender_wakers: VecDeque::new(),
        closed: false,
        sender_count: 1,
    }));

    let sender = Sender {
        shared: shared.clone(),
    };

    let receiver = Receiver {
        shared,
    };

    (sender, receiver)
}

// shared state between senders and receivers
struct ChannelState<T> {
    // fifo buffer for messages
    buffer: VecDeque<T>,
    // max size
    capacity: usize,
    // waker for the blocked receiver
    receiver_waker: Option<Waker>,
    // queue of wakers for blocked senders (FIFO order)
    sender_wakers: VecDeque<Waker>,
    // channel closed flag
    closed: bool,
    // number of active senders
    sender_count: usize,
}

// sender half of channel
pub struct Sender<T> {
    shared: Arc<Mutex<ChannelState<T>>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        {
            let mut state = self.shared.lock().unwrap();
            state.sender_count += 1;
        }
        Sender {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut state = self.shared.lock().unwrap();
        state.sender_count -= 1;
        
        // If this was the last sender, close the channel and wake receiver
        if state.sender_count == 0 {
            state.closed = true;
            if let Some(waker) = state.receiver_waker.take() {
                waker.wake();
            }
        }
    }
}

impl<T> Sender<T> {
    // send a value, wait if channel is full
    pub fn send(&self, value: T) -> SendFuture<T> {
        SendFuture {
            sender: self,
            value: Some(value),
            registered: false,
        }
    }
}

// receiver half of channel
pub struct Receiver<T> {
    shared: Arc<Mutex<ChannelState<T>>>,
}

impl<T> Receiver<T> {
    // receive a value, waiting if channel is empty
    pub fn recv(&self) -> RecvFuture<T> {
        RecvFuture {
            receiver: self,
        }
    }
}

// future for send ops
pub struct SendFuture<'a, T> {
    sender: &'a Sender<T>,
    value: Option<T>,
    registered: bool,
}

impl<T> Unpin for SendFuture<'_, T> {} 

impl<T> Future for SendFuture<'_, T> {
    type Output = Result<(), SendError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.sender.shared.lock().unwrap();

        // check if channel closed
        if state.closed {
            return Poll::Ready( Err(SendError::Closed) );
        }

        // try to send immediately if space available
        if state.buffer.len() < state.capacity {
            let value = this.value.take().unwrap();
            state.buffer.push_back(value);

            // wake up receiver if it was waiting
            if let Some(waker) = state.receiver_waker.take() {
                waker.wake();
            }

            return Poll::Ready(Ok(()));
        }

        // if channel is full, register our waker for when space opens up
        if !this.registered {
            state.sender_wakers.push_back(cx.waker().clone());
            this.registered = true;
        }

        Poll::Pending
    }
}

// future for receive ops
pub struct RecvFuture<'a, T> {
    receiver: &'a Receiver<T>,
}

impl<T> Unpin for RecvFuture<'_, T> {}

impl<T> Future for RecvFuture<'_, T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.receiver.shared.lock().unwrap();

        // try to receive immediately if there's data
        if let Some(value) = state.buffer.pop_front() {
            // wake up the first waiting sender (FIFO order)
            if let Some(sender_waker) = state.sender_wakers.pop_front() {
                sender_waker.wake();
            }

            return Poll::Ready(Ok(value));
        }

        // check if channel is closed and empty
        if state.closed {
            return Poll::Ready(Err(RecvError::Closed));
        }

        // no data available - register our waker
        state.receiver_waker = Some(cx.waker().clone());

        Poll::Pending
    }
}

#[derive(Debug)]
pub enum SendError {
    Closed,
}

#[derive(Debug)]
pub enum RecvError {
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::executor::SmarterExecutor;
    
    #[test]
    fn test_channel_fifo_ordering() {
        let mut executor = SmarterExecutor::new();
        let (tx, rx) = channel::<i32>(2);
        
        // Test FIFO ordering - sender will block after 2 items due to capacity
        executor.spawn(async move {
            println!("Sending 1");
            tx.send(1).await.unwrap();
            println!("Sending 2");
            tx.send(2).await.unwrap();
            println!("Sending 3 (will block until receiver reads)");
            tx.send(3).await.unwrap();
            println!("All values sent");
        });
        
        executor.spawn(async move {
            let val1 = rx.recv().await.unwrap();
            println!("Received: {}", val1);
            assert_eq!(val1, 1);
            
            let val2 = rx.recv().await.unwrap();
            println!("Received: {}", val2);
            assert_eq!(val2, 2);
            
            let val3 = rx.recv().await.unwrap();
            println!("Received: {}", val3);
            assert_eq!(val3, 3);
            
            println!("All values received in correct FIFO order!");
        });
        
        executor.run();
    }
    
    #[test]
    fn test_backpressure() {
        let mut executor = SmarterExecutor::new();
        let (tx, rx) = channel::<String>(1); // Tiny capacity to test backpressure
        
        executor.spawn(async move {
            println!("Sending 'first'");
            tx.send("first".to_string()).await.unwrap();
            println!("Sending 'second' (should block)");
            tx.send("second".to_string()).await.unwrap();
            println!("Backpressure worked - second send completed after recv");
        });
        
        executor.spawn(async move {
            // The backpressure test works by having the sender fill the buffer first,
            // then the receiver drains it, allowing the second send to complete.
            let val1 = rx.recv().await.unwrap();
            println!("Received: {}", val1);
            assert_eq!(val1, "first");
            
            let val2 = rx.recv().await.unwrap();
            println!("Received: {}", val2);
            assert_eq!(val2, "second");
        });
        
        executor.run();
    }
}

