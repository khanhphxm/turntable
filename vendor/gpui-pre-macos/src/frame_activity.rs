// Keep a short tail of vsyncs after the last draw or frame request. GPUI's
// invalidator wakes us both for dirty windows and pending next-frame callbacks,
// including callbacks whose frame was deferred by the inactive-window throttle.
const IDLE_FRAME_LIMIT: u8 = 3;

#[derive(Default)]
pub(crate) struct FrameActivity {
    requested: bool,
    idle_frames: u8,
}

impl FrameActivity {
    pub(crate) fn request(&mut self) {
        self.requested = true;
    }

    pub(crate) fn should_park(&mut self) -> bool {
        if std::mem::take(&mut self.requested) {
            self.idle_frames = 0;
        } else {
            self.idle_frames = self.idle_frames.saturating_add(1);
        }
        self.idle_frames >= IDLE_FRAME_LIMIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_source_parks_after_three_unused_frames() {
        let mut activity = FrameActivity::default();
        activity.request();
        assert!(!activity.should_park());
        assert!(!activity.should_park());
        assert!(!activity.should_park());
        assert!(activity.should_park());
        assert!(activity.should_park());
    }

    #[test]
    fn animation_demand_keeps_source_running_and_then_parks() {
        let mut activity = FrameActivity::default();
        for _ in 0..240 {
            activity.request();
            assert!(!activity.should_park());
        }
        for _ in 1..IDLE_FRAME_LIMIT {
            assert!(!activity.should_park());
        }
        assert!(activity.should_park());
    }

    #[test]
    fn demand_restarts_parked_source_and_resets_idle_tail() {
        let mut activity = FrameActivity::default();
        for _ in 0..IDLE_FRAME_LIMIT {
            activity.should_park();
        }
        assert!(activity.should_park());
        activity.request();
        assert!(!activity.should_park());
        assert!(!activity.should_park());
        activity.request();
        assert!(!activity.should_park());
        for _ in 1..IDLE_FRAME_LIMIT {
            assert!(!activity.should_park());
        }
        assert!(activity.should_park());
    }
}
