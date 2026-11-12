//! Разбор `WM_COPYDATA` от `Shell_NotifyIcon` (`dwData = 1`). Раскладка снята пробой на Windows 11
//! 10.0.28000: подпись, команда, затем `NOTIFYICONDATAW` из 956 байт с 32-битными дескрипторами.

pub const COPYDATA_TRAY: usize = 1;
const SIGNATURE: u32 = 0x3475_3423;
/// Нужны байты до конца `guidItem` (944 + 16).
const MIN_LEN: usize = 960;

pub const NIF_MESSAGE: u32 = 0x01;
pub const NIF_ICON: u32 = 0x02;
pub const NIF_TIP: u32 = 0x04;
pub const NIF_STATE: u32 = 0x08;
pub const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 0x01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyCmd {
    Add,
    Modify,
    Delete,
    SetFocus,
    SetVersion,
}

/// Одно сообщение трея. Поля, которых приложение не передало (нет флага), надо смотреть по `flags`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyData {
    pub hwnd: isize,
    pub uid: u32,
    pub guid: Option<[u8; 16]>,
    pub flags: u32,
    pub callback_msg: u32,
    pub hicon: isize,
    pub tip: String,
    /// `Some`, если приложение задало состояние «скрыт» (`NIF_STATE` и маска).
    pub hidden: Option<bool>,
    pub version: u32,
}

fn u32_at(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

pub fn parse_copydata(d: &[u8]) -> Option<(NotifyCmd, NotifyData)> {
    if d.len() < MIN_LEN || u32_at(d, 0) != SIGNATURE {
        return None;
    }
    let cmd = match u32_at(d, 4) {
        0 => NotifyCmd::Add,
        1 => NotifyCmd::Modify,
        2 => NotifyCmd::Delete,
        3 => NotifyCmd::SetFocus,
        4 => NotifyCmd::SetVersion,
        _ => return None,
    };
    let flags = u32_at(d, 20);
    let tip_units: Vec<u16> = d[32..288]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&c| c != 0)
        .collect();
    let (state, mask) = (u32_at(d, 288), u32_at(d, 292));
    let hidden = (flags & NIF_STATE != 0 && mask & NIS_HIDDEN != 0).then_some(state & NIS_HIDDEN != 0);
    let guid = (flags & NIF_GUID != 0).then(|| {
        let mut g = [0u8; 16];
        g.copy_from_slice(&d[944..960]);
        g
    });
    Some((
        cmd,
        NotifyData {
            // Дескрипторы окон и значков 32-битные и со знаком: расширяем как система.
            hwnd: u32_at(d, 12) as i32 as isize,
            uid: u32_at(d, 16),
            guid,
            flags,
            callback_msg: u32_at(d, 24),
            hicon: u32_at(d, 28) as i32 as isize,
            tip: String::from_utf16_lossy(&tip_units),
            hidden,
            version: u32_at(d, 808),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Собирает `WM_COPYDATA` так, как его шлёт `Shell_NotifyIcon` (раскладка из плана этапа 1б).
    fn build(cmd: i32, hwnd: i32, uid: u32, flags: u32, cb: u32, hicon: i32, tip: &str) -> Vec<u8> {
        let mut b = vec![0u8; 1484];
        let put = |b: &mut Vec<u8>, off: usize, v: u32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 0, 0x34753423);
        put(&mut b, 4, cmd as u32);
        put(&mut b, 8, 956);
        put(&mut b, 12, hwnd as u32);
        put(&mut b, 16, uid);
        put(&mut b, 20, flags);
        put(&mut b, 24, cb);
        put(&mut b, 28, hicon as u32);
        for (i, c) in tip.encode_utf16().enumerate() {
            b[32 + i * 2..34 + i * 2].copy_from_slice(&c.to_le_bytes());
        }
        b
    }

    #[test]
    fn parses_add_with_tip_and_callback() {
        let data = build(0, 0x3606D8, 7, 0x7, 0x800, 0x1234, "Steam — синхронизация");
        let (cmd, n) = parse_copydata(&data).unwrap();
        assert_eq!(cmd, NotifyCmd::Add);
        assert_eq!((n.hwnd, n.uid, n.callback_msg, n.hicon), (0x3606D8, 7, 0x800, 0x1234));
        assert_eq!(n.tip, "Steam — синхронизация");
        assert_eq!(n.flags & NIF_ICON, NIF_ICON);
        assert_eq!(n.hidden, None);
    }

    #[test]
    fn parses_delete() {
        let (cmd, n) = parse_copydata(&build(2, 0x10, 1, 0x1, 0x400, 0, "")).unwrap();
        assert_eq!(cmd, NotifyCmd::Delete);
        assert_eq!(n.uid, 1);
    }

    #[test]
    fn hidden_state_bit() {
        let mut data = build(1, 0x10, 1, NIF_STATE, 0, 0, "");
        data[288..292].copy_from_slice(&1u32.to_le_bytes()); // dwState = NIS_HIDDEN
        data[292..296].copy_from_slice(&1u32.to_le_bytes()); // dwStateMask
        assert_eq!(parse_copydata(&data).unwrap().1.hidden, Some(true));
        data[288..292].copy_from_slice(&0u32.to_le_bytes()); // показать
        assert_eq!(parse_copydata(&data).unwrap().1.hidden, Some(false));
    }

    #[test]
    fn guid_only_with_flag() {
        let mut data = build(0, 0x10, 1, NIF_GUID, 0, 0, "");
        data[944..960].copy_from_slice(&[7u8; 16]);
        assert_eq!(parse_copydata(&data).unwrap().1.guid, Some([7u8; 16]));
        data[20..24].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse_copydata(&data).unwrap().1.guid, None);
    }

    #[test]
    fn rejects_wrong_signature() {
        let mut data = build(0, 0x10, 1, 0x7, 0x800, 0, "x");
        data[0] = 0;
        assert!(parse_copydata(&data).is_none());
    }

    #[test]
    fn rejects_short_buffer() {
        assert!(parse_copydata(&build(0, 0x10, 1, 0x7, 0x800, 0, "x")[..100]).is_none());
    }
}
