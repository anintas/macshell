//! «Сейчас играет» в Центре управления: что воспроизводится в системе (браузер, плеер, Spotify) и кнопки
//! «назад», «пауза», «вперёд». Читается через системные медиаэлементы управления Windows. Запросы к ним идут в
//! фоновом потоке, окно получает ответ по каналу и не ждёт.

use std::sync::mpsc::{Receiver, channel};

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Now {
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
}

fn session() -> windows::core::Result<GlobalSystemMediaTransportControlsSession> {
    GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?.GetCurrentSession()
}

fn read() -> Option<Now> {
    let s = session().ok()?;
    let props = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let title = props.Title().ok()?.to_string();
    if title.trim().is_empty() {
        return None;
    }
    let playing = s
        .GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .map(|st| st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
        .unwrap_or(false);
    Some(Now { title, artist: props.Artist().map(|a| a.to_string()).unwrap_or_default(), playing })
}

fn spawn<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let _ = tx.send(work());
    });
    rx
}

/// Запрашивает, что сейчас играет; ответ приходит в канал (`None`, если ничего не играет или нет доступа).
pub fn request() -> Receiver<Option<Now>> {
    spawn("media-read", read)
}

/// Отправляет команду активному плееру; по окончании в канал приходит свежее состояние.
pub fn send(cmd: Command) -> Receiver<Option<Now>> {
    spawn("media-cmd", move || {
        if let Ok(s) = session() {
            let _ = match cmd {
                Command::PlayPause => s.TryTogglePlayPauseAsync().and_then(|a| a.join()),
                Command::Next => s.TrySkipNextAsync().and_then(|a| a.join()),
                Command::Previous => s.TrySkipPreviousAsync().and_then(|a| a.join()),
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(300)); // плееру нужно время сменить трек
        read()
    })
}

/// Вторая строка карточки: исполнитель или подсказка.
pub fn subtitle(now: &Option<Now>) -> String {
    match now {
        Some(n) if !n.artist.trim().is_empty() => n.artist.clone(),
        Some(_) => String::new(),
        None => "Ничего не играет".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_prefers_the_artist_and_explains_silence() {
        assert_eq!(subtitle(&None), "Ничего не играет");
        assert_eq!(subtitle(&Some(Now { title: "t".into(), artist: "Muse".into(), playing: true })), "Muse");
        assert_eq!(subtitle(&Some(Now { title: "t".into(), artist: "  ".into(), playing: false })), "");
    }

    #[test]
    fn real_session_read_does_not_panic() {
        let rx = request();
        let _ = rx.recv_timeout(std::time::Duration::from_secs(5));
    }
}
