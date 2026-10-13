// Copyright 2026 Turntable contributors. Licensed under Apache-2.0.
// Turntable addition: demand-driven Windows frame scheduling; see ../TURNTABLE.md.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use parking_lot::{Condvar, Mutex};

/// Shared by all windows and the compositor-wait thread. No timer runs while idle.
#[derive(Default)]
pub(crate) struct FrameScheduler {
    pending: Mutex<bool>,
    ready: Condvar,
}

impl FrameScheduler {
    pub(crate) fn wake(&self) {
        let mut pending = self.pending.lock();
        if !*pending {
            *pending = true;
            self.ready.notify_one();
        }
    }

    pub(crate) fn wake_all(&self) {
        let mut pending = self.pending.lock();
        *pending = true;
        self.ready.notify_all();
    }

    pub(crate) fn wait(&self, stop: &AtomicBool) -> bool {
        let mut pending = self.pending.lock();
        while !*pending && !stop.load(Ordering::Acquire) {
            self.ready.wait(&mut pending);
        }
        if stop.load(Ordering::Acquire) {
            return false;
        }
        *pending = false;
        true
    }
}

/// Coalesce requests per window, without invalidating other clean windows.
pub(crate) struct FrameRequest {
    pending: AtomicBool,
    scheduler: Arc<FrameScheduler>,
}

impl FrameRequest {
    pub(crate) fn new(scheduler: Arc<FrameScheduler>) -> Self {
        Self {
            pending: AtomicBool::new(false),
            scheduler,
        }
    }

    pub(crate) fn request(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            self.scheduler.wake();
        }
    }

    pub(crate) fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_requests_without_losing_next_frame() {
        let scheduler = Arc::new(FrameScheduler::default());
        let request = FrameRequest::new(scheduler.clone());
        let stop = AtomicBool::new(false);
        request.request();
        request.request();
        assert!(scheduler.wait(&stop));
        assert!(request.take());
        assert!(!request.take());
        request.request();
        assert!(scheduler.wait(&stop));
        assert!(request.take());
    }

    #[test]
    fn windows_have_independent_demand() {
        let scheduler = Arc::new(FrameScheduler::default());
        let first = FrameRequest::new(scheduler.clone());
        let second = FrameRequest::new(scheduler);
        first.request();
        assert!(first.take());
        assert!(!second.take());
    }

    #[test]
    fn wakes_a_parked_thread_and_stops_without_a_frame() {
        let scheduler = Arc::new(FrameScheduler::default());
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let scheduler = scheduler.clone();
            let stop = stop.clone();
            std::thread::spawn(move || scheduler.wait(&stop))
        };
        stop.store(true, Ordering::Release);
        scheduler.wake_all();
        assert!(!worker.join().unwrap());
    }

    #[test]
    fn stopped_thread_preserves_a_new_generation_wakeup() {
        let scheduler = FrameScheduler::default();
        scheduler.wake_all();
        assert!(!scheduler.wait(&AtomicBool::new(true)));
        assert!(scheduler.wait(&AtomicBool::new(false)));
    }
}
