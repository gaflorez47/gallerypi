/// Gesture state for pinch-to-zoom and pan.
/// Slint handles basic touch in .slint files; this is used for multi-touch
/// pinch detection if needed from the Rust side via pointer events.
#[derive(Debug, Default)]
pub struct ZoomPanState {
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl ZoomPanState {
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }

    pub fn apply_zoom(&mut self, delta: f32) {
        self.scale = (self.scale * delta).clamp(1.0, 8.0);
        if self.scale <= 1.0 {
            self.offset_x = 0.0;
            self.offset_y = 0.0;
        }
    }

    pub fn reset(&mut self) {
        self.scale = 1.0;
        self.offset_x = 0.0;
        self.offset_y = 0.0;
    }
}
