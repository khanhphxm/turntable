// Modified by Turntable: release regenerable CPU pixels after atlas upload; see ../TURNTABLE.md.
use crate::{DevicePixels, Pixels, Result, SharedString, Size, size};
use smallvec::SmallVec;

use image::{Delay, Frame, RgbaImage};
use parking_lot::Mutex;
use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

/// A source of assets for this app to use.
pub trait AssetSource: 'static + Send + Sync {
    /// Load the given asset from the source path.
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;

    /// List the assets at the given path.
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}

impl AssetSource for () {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

/// A unique identifier for the image cache
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

#[derive(PartialEq, Eq, Hash, Clone)]
#[expect(missing_docs)]
pub struct RenderImageParams {
    pub image_id: ImageId,
    pub frame_index: usize,
}

/// A cached and processed image, in BGRA format
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    data: RenderImageData,
}

enum RenderImageData {
    Frames(SmallVec<[Frame; 1]>),
    Reloadable {
        dimensions: (u32, u32),
        delay: Delay,
        byte_len: usize,
        pixels: Mutex<Option<RgbaImage>>,
        reload: Box<dyn Fn() -> Option<RgbaImage> + Send + Sync>,
    },
}

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RenderImage {}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        Self::with_data(RenderImageData::Frames(data.into()))
    }

    /// Create a single-frame image whose CPU pixels can be regenerated.
    ///
    /// The first call to [`Self::as_bytes`] transfers the supplied frame's buffer
    /// without copying it. Later calls invoke `reload`, which must return identical
    /// BGRA pixels with the same dimensions. Layout and cached atlas hits never
    /// invoke it. Keep expensive initial decoding off the foreground thread;
    /// `reload` runs synchronously only when pixels are needed again, for example
    /// after atlas eviction or when painting into another window.
    pub fn new_with_reloader(
        frame: Frame,
        reload: impl Fn() -> Option<RgbaImage> + Send + Sync + 'static,
    ) -> Self {
        let delay = frame.delay();
        let pixels = frame.into_buffer();
        Self::with_data(RenderImageData::Reloadable {
            dimensions: pixels.dimensions(),
            delay,
            byte_len: pixels.as_raw().len(),
            pixels: Mutex::new(Some(pixels)),
            reload: Box::new(reload),
        })
    }

    fn with_data(data: RenderImageData) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: ImageId(NEXT_ID.fetch_add(1, SeqCst)),
            scale_factor: 1.0,
            data,
        }
    }

    /// Get BGRA bytes, borrowing retained frames or transferring regenerable pixels.
    /// Returns `None` for an invalid frame index or a failed/mismatched reload.
    pub fn as_bytes(&self, frame_index: usize) -> Option<Cow<'_, [u8]>> {
        match &self.data {
            RenderImageData::Frames(frames) => frames
                .get(frame_index)
                .map(|frame| Cow::Borrowed(frame.buffer().as_raw().as_slice())),
            RenderImageData::Reloadable {
                dimensions,
                pixels,
                reload,
                ..
            } if frame_index == 0 => {
                let initial = pixels.lock().take();
                let pixels = initial.or_else(reload)?;
                if pixels.dimensions() != *dimensions {
                    return None;
                }
                Some(Cow::Owned(pixels.into_raw()))
            }
            RenderImageData::Reloadable { .. } => None,
        }
    }

    /// Get the uncompressed frame's byte length without loading or taking pixels.
    pub fn byte_len(&self, frame_index: usize) -> usize {
        match &self.data {
            RenderImageData::Frames(frames) => frames
                .get(frame_index)
                .map_or(0, |frame| frame.buffer().as_raw().len()),
            RenderImageData::Reloadable { byte_len, .. } if frame_index == 0 => *byte_len,
            RenderImageData::Reloadable { .. } => 0,
        }
    }

    /// Get the size of this image, in pixels.
    pub fn size(&self, frame_index: usize) -> Size<DevicePixels> {
        let dimensions = match &self.data {
            RenderImageData::Frames(frames) => frames
                .get(frame_index)
                .map(|frame| frame.buffer().dimensions()),
            RenderImageData::Reloadable { dimensions, .. } if frame_index == 0 => Some(*dimensions),
            RenderImageData::Reloadable { .. } => None,
        };
        dimensions
            .map(|(width, height)| size(width.into(), height.into()))
            .unwrap_or_default()
    }

    /// Get the size of this image, in pixels for display, adjusted for the scale factor.
    pub(crate) fn render_size(&self, frame_index: usize) -> Size<Pixels> {
        self.size(frame_index)
            .map(|v| (v.0 as f32 / self.scale_factor).into())
    }

    /// Get the delay of this frame from the previous
    pub fn delay(&self, frame_index: usize) -> Delay {
        match &self.data {
            RenderImageData::Frames(frames) => frames.get(frame_index).map(|frame| frame.delay()),
            RenderImageData::Reloadable { delay, .. } if frame_index == 0 => Some(*delay),
            RenderImageData::Reloadable { .. } => None,
        }
        .unwrap_or(Delay::from_numer_denom_ms(100, 1))
    }

    /// Get the number of frames for this image.
    pub fn frame_count(&self) -> usize {
        match &self.data {
            RenderImageData::Frames(frames) => frames.len(),
            RenderImageData::Reloadable { .. } => 1,
        }
    }
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageData")
            .field("id", &self.id)
            .field("size", &(self.frame_count() > 0).then(|| self.size(0)))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IntoElement as _, Styled as _, TestAppContext, img, point, px};
    use smallvec::SmallVec;
    use std::sync::Arc;

    #[test]
    fn empty_render_image_does_not_panic() {
        let image = RenderImage::new(SmallVec::new());
        assert_eq!(image.frame_count(), 0);
        assert_eq!(image.size(0), Size::default());
        assert_eq!(image.as_bytes(0), None);
        assert_eq!(image.render_size(0), Size::default());
        assert_eq!(image.delay(0), Delay::from_numer_denom_ms(100, 1));
        let _ = format!("{image:?}");
    }

    fn pixels() -> RgbaImage {
        RgbaImage::from_pixel(2, 3, image::Rgba([30, 80, 210, 170]))
    }

    #[test]
    fn reloadable_frames_transfer_initial_pixels_and_keep_metadata() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reload_calls = calls.clone();
        let buffer = pixels();
        let original = buffer.as_ptr();
        let delay = Delay::from_numer_denom_ms(42, 1);
        let image =
            RenderImage::new_with_reloader(Frame::from_parts(buffer, 0, 0, delay), move || {
                reload_calls.fetch_add(1, SeqCst);
                Some(pixels())
            });
        assert_eq!(image.byte_len(0), 24);
        assert_eq!(image.size(0), size(2.into(), 3.into()));
        assert_eq!(image.frame_count(), 1);
        assert_eq!(image.delay(0), delay);
        assert!(image.as_bytes(1).is_none());
        let first = image.as_bytes(0).unwrap();
        assert!(matches!(&first, Cow::Owned(_)));
        assert_eq!(first.as_ptr(), original);
        assert_eq!(calls.load(SeqCst), 0);
        let second = image.as_bytes(0).unwrap();
        assert_eq!(first, second);
        assert_eq!(calls.load(SeqCst), 1);
        assert_eq!(image.byte_len(0), 24);
        assert_eq!(image.byte_len(1), 0);
        assert_eq!(image.size(1), Size::default());
        assert_eq!(image.delay(0), delay);
    }

    #[test]
    fn retained_frames_borrow_pixels_and_reloads_reject_mismatched_dimensions() {
        let retained = RenderImage::new([Frame::new(pixels())]);
        assert!(matches!(retained.as_bytes(0), Some(Cow::Borrowed(_))));
        assert_eq!(retained.byte_len(0), 24);
        let mismatched =
            RenderImage::new_with_reloader(Frame::new(pixels()), || Some(RgbaImage::new(1, 1)));
        assert!(mismatched.as_bytes(0).is_some());
        assert!(mismatched.as_bytes(0).is_none());
        let failed = RenderImage::new_with_reloader(Frame::new(pixels()), || None);
        assert!(failed.as_bytes(0).is_some());
        assert!(failed.as_bytes(0).is_none());
    }

    #[gpui::test]
    fn reloadable_images_decode_only_after_atlas_removal(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let calls = Arc::new(AtomicUsize::new(0));
        let reload_calls = calls.clone();
        let image = Arc::new(RenderImage::new_with_reloader(
            Frame::new(pixels()),
            move || {
                reload_calls.fetch_add(1, SeqCst);
                Some(pixels())
            },
        ));
        for pass in 0..3 {
            window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
                img(image.clone()).size_full().into_any_element()
            });
            window.update(|window, _| {
                assert!(window.has_image_atlas_entry(&image));
                assert_eq!(calls.load(SeqCst), usize::from(pass == 2));
                if pass == 1 {
                    window.drop_image(image.clone()).unwrap();
                    assert!(!window.has_image_atlas_entry(&image));
                }
            });
        }
    }
}
