//! Windows input adapter shares the recorder's tested event logger.
use crate::engine::geom::Rect;
use crate::platform::input::*;
use crate::project::recording::CursorImage;
use anyhow::Result;
pub struct WindowsInputBackend;
impl WindowsInputBackend {
    pub fn new() -> Self {
        Self
    }
}
struct Session(Option<crate::recorder::input::InputLogger>);
impl InputSession for Session {
    fn finish(mut self: Box<Self>) -> InputLog {
        let log = self.0.take().unwrap().stop();
        InputLog {
            mouse: log
                .mouse
                .into_iter()
                .map(|m| RawMouse {
                    host: m.host,
                    x: m.x,
                    y: m.y,
                    kind: m.kind,
                    button: m.button,
                    cursor: m.cursor,
                })
                .collect(),
            keys: log
                .keys
                .into_iter()
                .map(|k| RawKey {
                    host: k.host,
                    code: k.code,
                    key: k.key,
                    mods: k.mods,
                    down: k.down,
                })
                .collect(),
            cursors: log.cursors,
            bounds: log.bounds,
            keys_enabled: log.keys_enabled,
        }
    }
}
impl InputBackend for WindowsInputBackend {
    fn start_monitoring(&self, window: Option<u32>, keys: bool) -> Result<Box<dyn InputSession>> {
        let dir = std::env::temp_dir().join("Cam").join("input-cursors");
        Ok(Box::new(Session(Some(
            crate::recorder::input::InputLogger::start(dir, window, keys),
        ))))
    }
    fn current_cursor_image(&self) -> Option<CursorImage> {
        crate::recorder::cursor::current(&std::env::temp_dir().join("Cam/input-cursors"), 0)
            .map(|(_, c)| c)
    }
    fn display_bounds(&self, id: u32) -> Rect {
        crate::recorder::input::display_bounds(id)
    }
}
