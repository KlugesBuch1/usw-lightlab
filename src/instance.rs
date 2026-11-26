use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, FindWindowW, MessageBoxW, SetForegroundWindow, SetWindowPos, ShowWindow,
    HWND_NOTOPMOST, HWND_TOPMOST, MB_ICONERROR, MB_OK, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SW_RESTORE,
};

const MUTEX_NAME: &str = "Local\\lightlab-single-instance";

pub struct Instance {
    handle: HANDLE,
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub fn acquire(window_title: &str) -> Result<Option<Instance>, String> {
    let name = wide(MUTEX_NAME);
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() {
            return Err(format!(
                "cannot create the lightlab single-instance mutex (Windows error {})",
                GetLastError()
            ));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            focus_existing(window_title);
            return Ok(None);
        }
        Ok(Some(Instance { handle }))
    }
}

fn focus_existing(window_title: &str) {
    let title = wide(window_title);
    for _ in 0..30 {
        unsafe {
            let window = FindWindowW(std::ptr::null(), title.as_ptr());
            if !window.is_null() {
                ShowWindow(window, SW_RESTORE);
                SetWindowPos(
                    window,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                SetWindowPos(
                    window,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                BringWindowToTop(window);
                SetForegroundWindow(window);
                return;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub fn show_error(message: &str) {
    let message = wide(message);
    let title = wide("lightlab");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
