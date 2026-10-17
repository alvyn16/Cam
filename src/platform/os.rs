//! Native OS integration and window chrome abstractions.

use anyhow::Result;
use winit::window::Window;

pub trait OsIntegrationBackend: Send + Sync {
    /// Configure window appearance (titlebar, dark mode, Mica/Acrylic).
    fn style_window(&self, window: &Window);
    /// Configure a floating overlay/pill window (borderless, topmost, tool window).
    fn style_floating_pill(&self, window: &Window);
    /// Bring the application to the front.
    fn activate_app(&self);
    /// Check if the window is currently in fullscreen.
    fn is_fullscreen(&self, window: &Window) -> bool;
    /// Check/request screen recording permission.
    fn ensure_screen_permission(&self) -> Result<bool>;
}
