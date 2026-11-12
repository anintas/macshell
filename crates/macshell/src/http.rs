//! Минимальный GET по HTTPS через WinHTTP (входит в Windows, своих библиотек не добавляет). Нужен только для
//! курсов валют: один запрос в полсутки.

use std::ffi::c_void;

use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_FLAG_SECURE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::core::{PCWSTR, w};

/// Не больше столько байт читается из ответа: курсы весят около 10 КБ.
const MAX_BODY: usize = 2 * 1024 * 1024;

/// Закрывает дескриптор WinHTTP при выходе из области видимости.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Делит `https://host/path?query` на узел и путь. Другие схемы не принимаются.
pub fn split_https(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    (!host.is_empty() && !host.contains(['@', ' '])).then_some((host, path))
}

/// Тело ответа на GET `url` (только HTTPS). `None` при любой ошибке сети. Блокирует до `timeout_ms` на шаг.
pub fn get(url: &str, timeout_ms: i32) -> Option<Vec<u8>> {
    let (host, path) = split_https(url)?;
    unsafe {
        let session = Handle(WinHttpOpen(w!("macshell"), WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return None;
        }
        WinHttpSetTimeouts(session.0, timeout_ms, timeout_ms, timeout_ms, timeout_ms).ok()?;
        let host_w = wide(host);
        let connection = Handle(WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), 443, 0));
        if connection.0.is_null() {
            return None;
        }
        let path_w = wide(path);
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return None;
        }
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available).ok()?;
            if available == 0 {
                break;
            }
            let mut chunk = vec![0u8; available as usize];
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, available, &mut read).ok()?;
            body.extend_from_slice(&chunk[..read as usize]);
            if body.len() > MAX_BODY {
                return None;
            }
        }
        Some(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_splits_into_host_and_path_and_only_https_is_accepted() {
        assert_eq!(split_https("https://www.cbr.ru/scripts/XML_daily.asp"), Some(("www.cbr.ru", "/scripts/XML_daily.asp")));
        assert_eq!(split_https("https://example.com"), Some(("example.com", "/")));
        assert_eq!(split_https("http://example.com/x"), None);
        assert_eq!(split_https("https:///x"), None);
        assert_eq!(split_https("https://user@evil.com/x"), None);
    }
}
