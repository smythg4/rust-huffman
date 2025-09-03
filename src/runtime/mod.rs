//! Simple async runtime - learning step by step

use std::future::Future;
use std::sync::atomic::AtomicBool;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::pin::Pin;
use std::sync::mpsc;
use std::collections::VecDeque;

pub mod channel;
pub mod task;
pub mod executor;


#[cfg(test)]
mod tests {
    use super::*;
    use executor::SmarterExecutor;
        
    #[test] 
    fn test_async_fn() {
        let mut executor = SmarterExecutor::new();
        
        // Test with a real async function
        async fn simple_async() -> i32 {
            42
        }
        
        let result = executor.block_on(simple_async());
        assert_eq!(result, 42);
    }
    
}