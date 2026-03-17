#[cfg(windows)]
pub fn bring_iris_to_front() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE};
    use widestring::U16CString;

    // Window title we expect the app to use
    let title = "Iris";
    if let Ok(u16_title) = U16CString::from_str(title) {
        unsafe {
            let h = FindWindowW(std::ptr::null(), u16_title.as_ptr());
            if h != 0 {
                // Restore if minimized
                ShowWindow(h, SW_RESTORE);
                // Try to set foreground
                SetForegroundWindow(h);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn bring_iris_to_front() {
    // no-op on non-windows platforms
}
