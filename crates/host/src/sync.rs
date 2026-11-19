//! 锁中毒照常拿锁。实例是常驻进程：一个任务持锁时 panic，`lock().unwrap()` 会让之后每个碰这把锁的
//! 任务都跟着 panic，整个实例连锁崩掉。锁里的状态最多是那一次改了一半，比实例没了好。

use std::sync::{Mutex, MutexGuard, PoisonError};

pub(crate) trait Locked<T> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> Locked<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_poisoned_lock_is_still_taken() {
        let m = Arc::new(Mutex::new(1));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("holding the lock");
        })
        .join();
        assert!(m.lock().is_err(), "锁中毒了");
        *m.locked() += 1;
        assert_eq!(*m.locked(), 2);
    }
}
