//! Whether the right mouse button is held, asked of the OS.
//!
//! Rune sends the right button only to `on_secondary_press`, when it is released, so a
//! right-button drag can't come through `on_drag`. While the pointer moves over the canvas the
//! editor asks the OS instead and pans while the button is down.

#[cfg(windows)]
pub fn right_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
    // GetAsyncKeyState reads physical buttons: with them swapped, the right one is VK_LBUTTON.
    let key = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 { VK_LBUTTON } else { VK_RBUTTON };
    (unsafe { GetAsyncKeyState(key as i32) } as u16 & 0x8000) != 0
}

#[cfg(target_os = "macos")]
pub fn right_button_down() -> bool {
    #[allow(unused_unsafe)]
    let buttons = unsafe { objc2_app_kit::NSEvent::pressedMouseButtons() };
    buttons & 0b10 != 0
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn right_button_down() -> bool {
    use std::sync::OnceLock;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, KeyButMask};
    use x11rb::rust_connection::RustConnection;

    // X11 (and XWayland) only; a pure Wayland session answers false.
    static X: OnceLock<Option<(RustConnection, u32)>> = OnceLock::new();
    let Some((conn, root)) = X.get_or_init(|| {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        Some((conn, root))
    }) else {
        return false;
    };
    conn.query_pointer(*root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some_and(|reply| u16::from(reply.mask) & u16::from(KeyButMask::BUTTON3) != 0)
}

#[cfg(not(any(windows, unix)))]
pub fn right_button_down() -> bool {
    false
}
