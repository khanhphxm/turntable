// Added by Turntable: coalesce frame demand without waking a running timer.
// See TURNTABLE.md. This policy is independent of X11 and the event loop.

use std::cell::Cell;

#[derive(Default)]
pub(crate) struct FrameRequest {
    requested: Cell<bool>,
    scheduled: Cell<bool>,
}

impl FrameRequest {
    /// Returns whether the event loop needs a ping. An armed timer already owns
    /// the next wake; demand during a frame is latched for its completion instead.
    pub fn request(&self) -> bool {
        !self.requested.replace(true) && !self.scheduled.get()
    }

    pub fn is_requested(&self) -> bool {
        self.requested.get()
    }

    pub fn arm(&self) {
        self.scheduled.set(true);
    }

    pub fn begin_frame(&self) {
        self.requested.set(false);
    }

    /// Hiding a window cancels its timer but preserves unserved animation demand.
    pub fn park(&self) {
        self.scheduled.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_requests_coalesce_until_a_frame_consumes_them() {
        let request = FrameRequest::default();
        assert!(!request.is_requested());
        assert!(request.request());
        assert!(!request.request());
        request.arm();
        request.begin_frame();
        assert!(!request.is_requested());
        request.park();
        assert!(request.request());
    }

    #[test]
    fn animation_demand_latches_without_extra_event_loop_pings() {
        let request = FrameRequest::default();
        request.request();
        request.arm();
        for _ in 0..120 {
            request.begin_frame();
            assert!(!request.request());
            assert!(request.is_requested());
        }
        request.begin_frame();
        assert!(!request.is_requested());
        request.park();
        assert!(request.request());
    }

    #[test]
    fn hiding_preserves_pending_demand_and_revealing_rearms_it() {
        let request = FrameRequest::default();
        request.request();
        request.arm();
        request.park();
        assert!(request.is_requested());
        request.arm();
        request.begin_frame();
        assert!(!request.is_requested());
        assert!(!request.request());
    }
}
