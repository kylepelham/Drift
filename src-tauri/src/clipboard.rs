//! Clipboard writes. Windows goes through Win32 directly so the text can be marked
//! non-sensitive for clipboard history; other platforms fall back to the webview.

/// Windows clipboard format for UTF-16 text.
#[cfg(windows)]
const CF_UNICODETEXT: u32 = 13;

#[cfg(windows)]
#[tauri::command]
pub(crate) fn clipboard_write_text(window: tauri::WebviewWindow, text: String) -> Result<(), String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    if text.is_empty() {
        return Ok(());
    }
    let value = clipboard_utf16(&text);
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, value.len() * std::mem::size_of::<u16>());
        if memory.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let target = GlobalLock(memory).cast::<u16>();
        if target.is_null() {
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }
        std::ptr::copy_nonoverlapping(value.as_ptr(), target, value.len());
        GlobalUnlock(memory);

        let history_memory = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>());
        if history_memory.is_null() {
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }
        let history_target = GlobalLock(history_memory).cast::<u32>();
        if history_target.is_null() {
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }
        history_target.write(1);
        GlobalUnlock(history_memory);
        let history_name = clipboard_utf16("CanIncludeInClipboardHistory");
        let history_format = RegisterClipboardFormatW(history_name.as_ptr());
        if history_format == 0 {
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }

        if OpenClipboard(hwnd) == 0 {
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(std::io::Error::last_os_error().to_string());
        }
        if EmptyClipboard() == 0 {
            let error = std::io::Error::last_os_error().to_string();
            CloseClipboard();
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(error);
        }
        if SetClipboardData(CF_UNICODETEXT, memory).is_null() {
            let error = std::io::Error::last_os_error().to_string();
            CloseClipboard();
            GlobalFree(history_memory);
            GlobalFree(memory);
            return Err(error);
        }
        if SetClipboardData(history_format, history_memory).is_null() {
            let error = std::io::Error::last_os_error().to_string();
            CloseClipboard();
            GlobalFree(history_memory);
            return Err(error);
        }
        CloseClipboard();
    }
    Ok(())
}

#[cfg(not(windows))]
#[tauri::command]
pub(crate) fn clipboard_write_text(_window: tauri::WebviewWindow, _text: String) -> Result<(), String> {
    Ok(())
}

pub(crate) fn clipboard_utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
