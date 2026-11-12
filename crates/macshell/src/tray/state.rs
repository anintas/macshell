//! Список значков трея: что приложения добавили, изменили и удалили.

use super::proto::{NIF_ICON, NIF_MESSAGE, NIF_TIP, NotifyCmd, NotifyData};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon {
    pub hwnd: isize,
    pub uid: u32,
    pub guid: Option<[u8; 16]>,
    pub callback_msg: u32,
    pub hicon: isize,
    pub tip: String,
    pub hidden: bool,
    pub version: u32,
}

#[derive(Default)]
pub struct TrayState {
    icons: Vec<TrayIcon>,
}

impl TrayState {
    /// Все значки в порядке добавления, в том числе скрытые.
    pub fn icons(&self) -> &[TrayIcon] {
        &self.icons
    }

    /// Что видно снаружи: окно, номер, дескриптор значка и «скрыт». Подсказки сюда не входят: у многих
    /// приложений они меняются каждую секунду, будить из-за этого панель незачем.
    pub fn signature(&self) -> Vec<(isize, u32, isize, bool)> {
        self.icons.iter().map(|i| (i.hwnd, i.uid, i.hicon, i.hidden)).collect()
    }

    /// То, что надо показывать.
    pub fn visible(&self) -> impl Iterator<Item = &TrayIcon> {
        self.icons.iter().filter(|i| !i.hidden)
    }

    fn position(&self, d: &NotifyData) -> Option<usize> {
        match d.guid {
            Some(g) => self.icons.iter().position(|i| i.guid == Some(g)),
            None => self.icons.iter().position(|i| i.guid.is_none() && i.hwnd == d.hwnd && i.uid == d.uid),
        }
    }

    /// Применяет сообщение. `true`, если изменилось то, что видно пользователю.
    pub fn apply(&mut self, cmd: NotifyCmd, d: NotifyData) -> bool {
        let found = self.position(&d);
        match (cmd, found) {
            (NotifyCmd::Add, None) => {
                self.icons.push(TrayIcon {
                    hwnd: d.hwnd,
                    uid: d.uid,
                    guid: d.guid,
                    callback_msg: if d.flags & NIF_MESSAGE != 0 { d.callback_msg } else { 0 },
                    hicon: if d.flags & NIF_ICON != 0 { d.hicon } else { 0 },
                    tip: if d.flags & NIF_TIP != 0 { d.tip } else { String::new() },
                    hidden: d.hidden.unwrap_or(false),
                    version: 0,
                });
                true
            }
            // Повторное добавление того же значка работает как изменение (приложение перезапустилось).
            (NotifyCmd::Add | NotifyCmd::Modify, Some(i)) => {
                let before = self.icons[i].clone();
                let icon = &mut self.icons[i];
                icon.hwnd = d.hwnd;
                icon.uid = d.uid;
                if d.flags & NIF_MESSAGE != 0 {
                    icon.callback_msg = d.callback_msg;
                }
                if d.flags & NIF_ICON != 0 {
                    icon.hicon = d.hicon;
                }
                if d.flags & NIF_TIP != 0 {
                    icon.tip = d.tip;
                }
                if let Some(h) = d.hidden {
                    icon.hidden = h;
                }
                *icon != before
            }
            (NotifyCmd::Delete, Some(i)) => {
                self.icons.remove(i);
                true
            }
            (NotifyCmd::SetVersion, Some(i)) => {
                self.icons[i].version = d.version;
                false
            }
            _ => false,
        }
    }

    /// Убирает значки, чьё окно-владелец закрыто (приложение упало без `NIM_DELETE`).
    pub fn prune(&mut self, alive: impl Fn(isize) -> bool) -> bool {
        let before = self.icons.len();
        self.icons.retain(|i| alive(i.hwnd));
        self.icons.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::proto::{NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_STATE, NIF_TIP};

    fn data(hwnd: isize, uid: u32, flags: u32, tip: &str, hicon: isize) -> NotifyData {
        NotifyData { hwnd, uid, guid: None, flags, callback_msg: 0x800, hicon, tip: tip.into(), hidden: None, version: 0 }
    }

    const ALL: u32 = NIF_MESSAGE | NIF_ICON | NIF_TIP;

    #[test]
    fn add_then_modify_updates_tip() {
        let mut s = TrayState::default();
        assert!(s.apply(NotifyCmd::Add, data(1, 1, ALL, "было", 10)));
        assert!(s.apply(NotifyCmd::Modify, data(1, 1, NIF_TIP, "стало", 0)));
        assert_eq!(s.icons()[0].tip, "стало");
    }

    #[test]
    fn modify_unknown_is_ignored() {
        let mut s = TrayState::default();
        assert!(!s.apply(NotifyCmd::Modify, data(1, 1, ALL, "x", 10)));
        assert!(s.icons().is_empty());
    }

    #[test]
    fn delete_removes() {
        let mut s = TrayState::default();
        s.apply(NotifyCmd::Add, data(1, 1, ALL, "x", 10));
        assert!(s.apply(NotifyCmd::Delete, data(1, 1, 0, "", 0)));
        assert!(s.icons().is_empty());
        assert!(!s.apply(NotifyCmd::Delete, data(1, 1, 0, "", 0)));
    }

    #[test]
    fn same_hwnd_and_uid_not_duplicated() {
        let mut s = TrayState::default();
        s.apply(NotifyCmd::Add, data(1, 1, ALL, "a", 10));
        s.apply(NotifyCmd::Add, data(1, 1, ALL, "b", 11));
        assert_eq!(s.icons().len(), 1);
        assert_eq!(s.icons()[0].tip, "b");
        s.apply(NotifyCmd::Add, data(1, 2, ALL, "c", 12));
        assert_eq!(s.icons().len(), 2);
    }

    #[test]
    fn same_guid_not_duplicated() {
        let mut s = TrayState::default();
        let with_guid = |hwnd| NotifyData { guid: Some([5; 16]), ..data(hwnd, 1, ALL | NIF_GUID, "g", 10) };
        s.apply(NotifyCmd::Add, with_guid(1));
        s.apply(NotifyCmd::Add, with_guid(2)); // приложение перезапустилось: новое окно, тот же значок
        assert_eq!(s.icons().len(), 1);
        assert_eq!(s.icons()[0].hwnd, 2);
    }

    #[test]
    fn modify_keeps_old_icon_when_flag_missing() {
        let mut s = TrayState::default();
        s.apply(NotifyCmd::Add, data(1, 1, ALL, "x", 10));
        s.apply(NotifyCmd::Modify, data(1, 1, NIF_TIP, "y", 99));
        assert_eq!(s.icons()[0].hicon, 10);
        s.apply(NotifyCmd::Modify, data(1, 1, NIF_ICON, "", 77));
        assert_eq!(s.icons()[0].hicon, 77);
        assert_eq!(s.icons()[0].tip, "y");
    }

    #[test]
    fn prune_drops_dead_windows() {
        let mut s = TrayState::default();
        s.apply(NotifyCmd::Add, data(1, 1, ALL, "жив", 10));
        s.apply(NotifyCmd::Add, data(2, 1, ALL, "мёртв", 11));
        assert!(s.prune(|hwnd| hwnd == 1));
        assert_eq!(s.icons().len(), 1);
        assert!(!s.prune(|hwnd| hwnd == 1));
    }

    #[test]
    fn hidden_icons_are_not_visible() {
        let mut s = TrayState::default();
        s.apply(NotifyCmd::Add, data(1, 1, ALL, "a", 10));
        s.apply(NotifyCmd::Add, NotifyData { hidden: Some(true), ..data(2, 1, ALL | NIF_STATE, "b", 11) });
        assert_eq!(s.icons().len(), 2);
        assert_eq!(s.visible().map(|i| i.hwnd).collect::<Vec<_>>(), vec![1]);
        s.apply(NotifyCmd::Modify, NotifyData { hidden: Some(false), ..data(2, 1, NIF_STATE, "", 0) });
        assert_eq!(s.visible().count(), 2);
    }
}

#[cfg(test)]
mod signature_tests {
    use super::*;
    use crate::tray::proto::{NIF_ICON, NIF_MESSAGE, NIF_STATE, NIF_TIP};

    fn data(tip: &str, hicon: isize, flags: u32) -> NotifyData {
        NotifyData { hwnd: 1, uid: 1, guid: None, flags, callback_msg: 0x800, hicon, tip: tip.into(), hidden: None, version: 0 }
    }

    #[test]
    fn signature_changes_on_icon_swap_and_hiding_but_not_on_tooltip() {
        let mut s = TrayState::default();
        s.apply(NotifyCmd::Add, data("a", 10, NIF_MESSAGE | NIF_ICON | NIF_TIP));
        let before = s.signature();
        s.apply(NotifyCmd::Modify, data("b", 0, NIF_TIP));
        assert_eq!(s.signature(), before, "tooltip only");
        s.apply(NotifyCmd::Modify, data("b", 11, NIF_ICON));
        assert_ne!(s.signature(), before, "new icon handle");
        let swapped = s.signature();
        s.apply(NotifyCmd::Modify, NotifyData { hidden: Some(true), ..data("", 0, NIF_STATE) });
        assert_ne!(s.signature(), swapped, "hidden");
    }
}
