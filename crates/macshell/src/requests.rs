//! Просьбы главному циклу панели от кода, который не держит окон панели: действия Spotlight («Mission Control»,
//! «Настройки macshell», «Центр уведомлений») и горячие углы. Флаг ставится из любого места, цикл забирает его
//! на ближайшем шаге.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Mission = 1,
    Settings = 2,
    Panel = 4,
    Stage = 8,
    Calendar = 16,
}

static PENDING: AtomicU8 = AtomicU8::new(0);

pub fn set(r: Request) {
    PENDING.fetch_or(r as u8, Ordering::SeqCst);
}

/// Была ли такая просьба; забирает её.
pub fn take(r: Request) -> bool {
    PENDING.fetch_and(!(r as u8), Ordering::SeqCst) & r as u8 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_taken_once_and_requests_do_not_mix() {
        set(Request::Mission);
        set(Request::Panel);
        assert!(take(Request::Mission));
        assert!(!take(Request::Mission), "taken only once");
        assert!(!take(Request::Settings));
        assert!(take(Request::Panel));
    }
}
