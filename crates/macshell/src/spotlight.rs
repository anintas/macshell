//! Окно Spotlight: стеклянная панель с полем ввода и результатами. Создаётся при открытии и уничтожается при
//! закрытии, в покое не занимает памяти. Фон под панелью снимается и размывается один раз при открытии,
//! стекло для текущей высоты панели пересчитывается, только когда высота меняется.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use capture::{RectI, capture_region};
use glass::{Bitmap, GlassParams, cpu};
use slint::{ModelRc, VecModel};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, IsWindow, SM_CXSCREEN, SM_CYSCREEN, SetForegroundWindow, WM_ACTIVATE,
};

use crate::actions::{self, ActionDef, ActionId};
use crate::anim::{ease_out_cubic, progress};
use crate::bar_style::{SoftBackdrop, padded_rect};
use crate::clipboard::{self, ClipItem};
use crate::files::{self, FileEntry};
use crate::host::{InputEvent, LayeredWindow, primary_scale};
use crate::icons::load_icon;
use crate::index::{Entry, Kind, Usage, rank};
use crate::quick::{self, QuickKey, QuickTarget};
use crate::quicklook;
use crate::search::calc;
use crate::currency;
use crate::units;
use crate::slint_view::{ContentView, Spotlight, SpotlightLine, TabButton, image_from_pixels};

const WIDTH_PT: f32 = 680.0;
/// Окно рассчитано на самый длинный список: 7 строк и 3 заголовка.
const MAX_HEIGHT_PT: f32 = 560.0;
const TOP_FRACTION: f32 = 0.20;
pub const MAX_ROWS: usize = 7;
const APPS_MAX: usize = 4;
const SETTINGS_MAX: usize = 3;
const APPEAR_MS: u32 = 130;
const APPEAR_DY_PT: f32 = 10.0;
const BLINK_MS: u64 = 530;
const ICON_PT: f32 = 30.0;
const FOCUS_RETRY_MS: u64 = 40;

const VK_BACK: u32 = 0x08;
const VK_TAB: u32 = 0x09;
const VK_RETURN: u32 = 0x0D;
const VK_ESCAPE: u32 = 0x1B;
const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;
const VK_V: u32 = 0x56;
const VK_Y: u32 = 0x59;
/// Сторона плиты предпросмотра в логических пикселях (миниатюра чуть меньше).
const PREVIEW_PT: f32 = 252.0;
const VK_0: u32 = 0x30;
const VK_4: u32 = 0x34;

/// Время шагов открытия под `MACSHELL_TRACE=1`: показывает, куда уходят миллисекунды до первого кадра.
fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn lap(label: &str, since: &mut Instant) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("MACSHELL_TRACE").is_some()) {
        eprintln!("    spotlight {label}: {:.1} ms", since.elapsed().as_secs_f64() * 1000.0);
    }
    *since = Instant::now();
}

/// Тёмное дымчатое стекло под белый текст. Радиус 30 = радиус строки 18 + отступ 12 (концентрично).
fn spotlight_glass() -> GlassParams {
    GlassParams {
        radius: 30.0,
        bevel: 24.0,
        refraction: 16.0,
        blur_radius: 12,
        saturation: 1.5,
        tint: [0.05, 0.06, 0.09, 0.36],
        dispersion: 2.0,
        rim: 0.6,
        ..GlassParams::default()
    }
}

/// Строка списка: результат калькулятора, запись индекса, файл, действие или копия из буфера обмена.
#[derive(Clone, Debug)]
pub enum Hit {
    Calc(String),
    Entry(Entry),
    File(FileEntry),
    Action(&'static ActionDef),
    /// Текст копии и время копирования (секунды Unix).
    Clip(String, u64),
    /// Сработала быстрая клавиша: запрос набран целиком.
    Quick(QuickKey),
    /// Результат конвертации («6,21371 mi», «925,37 RUB») и подпись источника; результат копируется в буфер.
    Convert(String, String),
}

/// Вкладка Spotlight. «Приложения» своей вкладки не имеют: Ctrl+1 открывает окно «Приложения».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    All,
    Files,
    Actions,
    Clipboard,
}

/// Подписи кнопок вкладок и их сочетания клавиш; порядок совпадает с Ctrl+1..Ctrl+4.
pub const TAB_BUTTONS: [(&str, &str); 4] = [("Приложения", "Ctrl+1"), ("Файлы", "Ctrl+2"), ("Действия", "Ctrl+3"), ("Буфер", "Ctrl+4")];

/// Вкладка по номеру кнопки или сочетания (0 это «Приложения», их окно открывает владелец).
pub fn tab_of_button(n: usize) -> Option<Tab> {
    match n {
        1 => Some(Tab::Files),
        2 => Some(Tab::Actions),
        3 => Some(Tab::Clipboard),
        _ => None,
    }
}

/// Номер подсвеченной кнопки: у обычного поиска ни одной.
pub fn active_button(tab: Tab) -> i32 {
    match tab {
        Tab::All => -1,
        Tab::Files => 1,
        Tab::Actions => 2,
        Tab::Clipboard => 3,
    }
}

/// Секция списка с заголовком.
#[derive(Clone, Debug)]
pub struct Group {
    pub title: &'static str,
    pub hits: Vec<Hit>,
}

/// Что сделать с выбранной строкой.
#[derive(Clone, Debug)]
pub enum Launch {
    Entry(Entry),
    /// Результат калькулятора или выбранная копия: кладётся в буфер обмена.
    Copy(String),
    /// Файл или папка: открыть, а с Ctrl показать в Проводнике.
    File { path: String, reveal: bool },
    Action(ActionId),
    /// Путь, сайт или `shell:`-адрес из быстрой клавиши.
    Open(String),
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Stay,
    Close,
    Launch(Launch),
    /// Ctrl+1: вместо вкладки открывается окно «Приложения».
    OpenApps,
}

/// Раскладывает найденное по секциям: калькулятор или лучшее совпадение, приложения, системные настройки.
/// Всего не больше `MAX_ROWS` строк. `ranked` отсортирован от лучшего к худшему.
pub fn group_results(calc: Option<String>, ranked: &[Entry]) -> Vec<Group> {
    let mut groups = Vec::new();
    let others: &[Entry] = match (calc, ranked.first()) {
        (Some(result), _) => {
            groups.push(Group { title: "Лучшее совпадение", hits: vec![Hit::Calc(result)] });
            ranked
        }
        (None, Some(best)) => {
            groups.push(Group { title: "Лучшее совпадение", hits: vec![Hit::Entry(best.clone())] });
            &ranked[1..]
        }
        (None, None) => return groups,
    };
    let room = MAX_ROWS - 1;
    let apps: Vec<Hit> =
        others.iter().filter(|e| e.kind == Kind::App).take(APPS_MAX.min(room)).cloned().map(Hit::Entry).collect();
    let room = room - apps.len();
    let settings: Vec<Hit> =
        others.iter().filter(|e| e.kind == Kind::Setting).take(SETTINGS_MAX.min(room)).cloned().map(Hit::Entry).collect();
    if !apps.is_empty() {
        groups.push(Group { title: "Приложения", hits: apps });
    }
    if !settings.is_empty() {
        groups.push(Group { title: "Системные настройки", hits: settings });
    }
    groups
}

/// Вкладка «Файлы»: недавние файлы при пустом запросе, иначе совпадения по имени.
pub fn file_groups(files: &[FileEntry], query: &str) -> Vec<Group> {
    let (title, found) =
        if query.trim().is_empty() { ("Недавние файлы", files::recent(files, MAX_ROWS)) } else { ("Файлы", files::search(files, query, MAX_ROWS)) };
    if found.is_empty() {
        return Vec::new();
    }
    vec![Group { title, hits: found.into_iter().cloned().map(Hit::File).collect() }]
}

/// Вкладка «Действия».
pub fn action_groups(query: &str) -> Vec<Group> {
    let found = actions::search(query, MAX_ROWS);
    if found.is_empty() {
        return Vec::new();
    }
    vec![Group { title: "Действия", hits: found.into_iter().map(Hit::Action).collect() }]
}

/// Ставит перед `rest` результат конвертации единиц или валют, если запрос вида «10 km in mi» или
/// «100 usd in eur»; всего не больше `MAX_ROWS`. Курсы валют приходят с задержкой: пока их нет, строки нет.
pub fn with_conversion(query: &str, rest: Vec<Group>) -> Vec<Group> {
    let found = units::convert(query).map(|r| (r, "Конвертация".to_string())).or_else(|| match currency::convert(query) {
        currency::Outcome::Ready { text, date } => Some((text, format!("Курс ЦБ {date}"))),
        _ => None,
    });
    let Some((result, note)) = found else { return rest };
    let mut out = vec![Group { title: "Конвертация", hits: vec![Hit::Convert(result, note)] }];
    out.extend(rest);
    trim_rows(&mut out, MAX_ROWS);
    out
}

/// Оставляет в списке не больше `max` строк, отбрасывая с конца; пустые секции убираются.
pub fn trim_rows(groups: &mut Vec<Group>, max: usize) {
    let mut left = max;
    for g in groups.iter_mut() {
        g.hits.truncate(left);
        left -= g.hits.len();
    }
    groups.retain(|g| !g.hits.is_empty());
}

/// Ставит перед `rest` строку быстрой клавиши, если запрос набран целиком. Действие, которое она запускает,
/// из остального списка убирается, чтобы не показываться дважды; всего строк не больше `MAX_ROWS`.
pub fn with_quick_key(keys: &[QuickKey], query: &str, mut rest: Vec<Group>) -> Vec<Group> {
    let Some(key) = quick::find(keys, query) else { return rest };
    if let QuickTarget::Action(id) = key.target {
        for g in rest.iter_mut() {
            g.hits.retain(|h| !matches!(h, Hit::Action(a) if a.id == id));
        }
        rest.retain(|g| !g.hits.is_empty());
    }
    let mut out = vec![Group { title: "Быстрая клавиша", hits: vec![Hit::Quick(key.clone())] }];
    out.extend(rest);
    trim_rows(&mut out, MAX_ROWS);
    out
}

/// Вкладка «Буфер»: копии от новых к старым, при запросе только те, где он встречается (без учёта регистра).
pub fn clip_groups(items: &[ClipItem], query: &str) -> Vec<Group> {
    let q = query.trim().to_lowercase();
    let hits: Vec<Hit> = items
        .iter()
        .filter(|i| q.is_empty() || i.text.to_lowercase().contains(&q))
        .take(MAX_ROWS)
        .map(|i| Hit::Clip(i.text.clone(), i.at))
        .collect();
    if hits.is_empty() {
        return Vec::new();
    }
    vec![Group { title: "Буфер обмена", hits }]
}

/// Пояснение к запросу про валюты, у которого пока нет ответа: курсы грузятся или недоступны.
pub fn currency_note(query: &str) -> &'static str {
    match currency::convert(query) {
        currency::Outcome::Loading => "Загружаю курсы валют…",
        currency::Outcome::Unavailable => "Курсы валют недоступны: нет связи с cbr.ru",
        _ => "",
    }
}

/// Что написать, когда на вкладке нет строк; пусто, если строки есть или это обычный поиск.
pub fn empty_text(tab: Tab, query_empty: bool, files_ready: bool, has_hits: bool, currency_note: &'static str) -> &'static str {
    if has_hits {
        return "";
    }
    match tab {
        Tab::All => currency_note,
        Tab::Files if !files_ready => "Индексирую файлы…",
        Tab::Files => "Ничего не найдено",
        Tab::Actions => "Нет такого действия",
        Tab::Clipboard if query_empty => "История буфера обмена пуста",
        Tab::Clipboard => "Ничего не найдено",
    }
}

/// Номера первых строк каждой секции в общем порядке результатов.
fn section_starts(groups: &[Group]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut n = 0;
    for g in groups {
        starts.push(n);
        n += g.hits.len();
    }
    starts
}

/// Табуляция прыгает по секциям: к началу следующей (с Shift предыдущей), по кругу.
pub fn next_section(starts: &[usize], selected: usize, backwards: bool) -> usize {
    if starts.is_empty() {
        return 0;
    }
    if backwards {
        starts.iter().rev().find(|&&s| s < selected).or(starts.last()).copied().unwrap_or(0)
    } else {
        starts.iter().find(|&&s| s > selected).or(starts.first()).copied().unwrap_or(0)
    }
}

/// Убирает последнее слово (Ctrl+Backspace) вместе с пробелами перед ним.
pub fn delete_word(s: &mut String) {
    while s.ends_with(' ') {
        s.pop();
    }
    while s.chars().last().is_some_and(|c| c != ' ') {
        s.pop();
    }
}

/// Вставленный текст: первая строка без управляющих символов, не длиннее 200 символов.
pub fn sanitize_paste(text: &str) -> String {
    text.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).take(200).collect::<String>()
}

/// Текст из буфера обмена, если там текст.
pub fn clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    unsafe {
        OpenClipboard(None).ok()?;
        let text = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let global = HGLOBAL(handle.0);
            let ptr = GlobalLock(global) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(global);
            Some(s)
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Кладёт текст в буфер обмена (результат калькулятора).
pub fn set_clipboard_text(text: &str) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let ok = (|| {
            EmptyClipboard().ok()?;
            let global = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).ok()?;
            let ptr = GlobalLock(global) as *mut u16;
            if ptr.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(global);
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(global.0))).ok()?;
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}

pub struct SpotlightWin {
    win: LayeredWindow,
    view: ContentView<Spotlight>,
    scale: f32,
    params: GlassParams,
    /// Размытый снимок экрана вокруг окна и положение окна внутри него.
    blurred: SoftBackdrop,
    origin: (u32, u32),
    /// Стекло для одной высоты панели (пиксели): пересчитывается, когда высота меняется.
    glass: Option<(u32, Bitmap)>,
    frame: Bitmap,
    index: Arc<Vec<Entry>>,
    usage: Usage,
    query: String,
    groups: Vec<Group>,
    hits: Vec<Hit>,
    selected: usize,
    /// Открыт предпросмотр (Ctrl+Y): список скрыт, на его месте миниатюра и сведения о выбранном.
    preview_open: bool,
    tab: Tab,
    files: Arc<Vec<FileEntry>>,
    /// Быстрые клавиши: встроенные и из конфига.
    quick: Arc<Vec<QuickKey>>,
    files_ready: bool,
    /// Вкладка «Файлы» открыта, а индекса ещё нет: владелец окна запускает его построение.
    wants_files: bool,
    icons: HashMap<String, Option<slint::Image>>,
    clicked: Rc<Cell<i32>>,
    tab_clicked: Rc<Cell<i32>>,
    hovered: Rc<Cell<i32>>,
    appear: Option<Instant>,
    caret_on: bool,
    blink_at: Instant,
    /// Окно, которое было впереди до нас: возвращаем ему фокус при закрытии без запуска.
    previous: isize,
    restore_focus: bool,
    /// Сколько раз ещё повторить попытку взять фокус: пока Win и Alt зажаты, система бывает против.
    focus_retries: u32,
    next_focus_at: Instant,
    /// Окно уже было активным: только тогда потеря активности закрывает его (до этого она случайна).
    had_focus: bool,
}

impl SpotlightWin {
    /// `files`: индекс файлов, если он уже построен.
    pub fn open(index: Arc<Vec<Entry>>, usage: &Usage, files: Option<Arc<Vec<FileEntry>>>, quick: Arc<Vec<QuickKey>>) -> Result<SpotlightWin> {
        let scale = primary_scale();
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let (w, h) = ((WIDTH_PT * scale).round() as i32, (MAX_HEIGHT_PT * scale).round() as i32);
        let rect = RectI { x: (sw - w) / 2, y: (sh as f32 * TOP_FRACTION).round() as i32, w, h };
        let previous = unsafe { GetForegroundWindow() }.0 as isize;
        let mut t = Instant::now();

        let mut win = LayeredWindow::new_focusable(rect)?;
        let motion = crate::anim::enabled();
        win.animate_overlay(false, -(APPEAR_DY_PT * scale).round() as i32);
        if motion { win.set_blend(0, -(APPEAR_DY_PT * scale).round() as i32)?; }
        lap("window", &mut t);
        let view = ContentView::new_with(w as u32, h as u32, Spotlight::new);
        lap("slint view", &mut t);
        view.set_scale_factor(scale);
        let clicked = Rc::new(Cell::new(-1));
        let hovered = Rc::new(Cell::new(-1));
        let c = clicked.clone();
        view.ui.on_activated(move |i| c.set(i));
        let hv = hovered.clone();
        view.ui.on_hovered(move |i| hv.set(i));
        let tab_clicked = Rc::new(Cell::new(-1));
        let tc = tab_clicked.clone();
        view.ui.on_tab_selected(move |i| tc.set(i));
        let buttons: Vec<TabButton> =
            TAB_BUTTONS.iter().map(|(name, hint)| TabButton { name: (*name).into(), hint: (*hint).into() }).collect();
        view.ui.set_tab_buttons(ModelRc::new(VecModel::from(buttons)));

        let params = spotlight_glass().scaled(scale);
        let padded = padded_rect(rect, 2 * params.blur_radius as i32, sw, sh);
        let shot = capture_region(padded)?;
        lap("capture", &mut t);
        let blurred = SoftBackdrop::new(&shot, params.blur_radius);
        lap("blur", &mut t);
        let origin = ((rect.x - padded.x) as u32, (rect.y - padded.y) as u32);

        let mut spot = SpotlightWin {
            win,
            view,
            scale,
            params,
            blurred,
            origin,
            glass: None,
            frame: Bitmap::new(w as u32, h as u32),
            index,
            usage: usage.clone(),
            query: String::new(),
            groups: Vec::new(),
            hits: Vec::new(),
            selected: 0,
            preview_open: false,
            tab: Tab::All,
            files_ready: files.is_some(),
            files: files.unwrap_or_default(),
            quick,
            wants_files: false,
            icons: HashMap::new(),
            clicked,
            tab_clicked,
            hovered,
            appear: motion.then(Instant::now),
            caret_on: true,
            blink_at: Instant::now() + Duration::from_millis(BLINK_MS),
            previous,
            restore_focus: true,
            focus_retries: 12,
            next_focus_at: Instant::now() + Duration::from_millis(FOCUS_RETRY_MS),
            had_focus: false,
        };
        spot.rebuild();
        lap("rebuild", &mut t);
        spot.present()?;
        lap("first present", &mut t);
        spot.win.focus();
        spot.win.capture_mouse(true);
        lap("focus", &mut t);
        Ok(spot)
    }

    /// Новый индекс (фоновая сборка закончилась, пока окно открыто).
    pub fn set_index(&mut self, index: Arc<Vec<Entry>>) {
        self.index = index;
        self.rebuild();
    }

    /// Индекс файлов построен (или обновлён): вкладка «Файлы» перерисовывается.
    pub fn set_files(&mut self, files: Arc<Vec<FileEntry>>) {
        self.files = files;
        self.files_ready = true;
        self.wants_files = false;
        if self.tab == Tab::Files {
            self.rebuild();
        }
    }

    /// Вкладка «Файлы» просила индекс, которого нет; забирается один раз.
    pub fn take_wants_files(&mut self) -> bool {
        std::mem::take(&mut self.wants_files)
    }

    pub fn window_hwnd(&self) -> isize {
        self.win.hwnd()
    }

    pub fn has_focus(&self) -> bool {
        self.win.is_foreground()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    /// Пересчитывает результаты по запросу и обновляет интерфейс.
    fn rebuild(&mut self) {
        self.groups = match self.tab {
            Tab::All => {
                let ranked: Vec<Entry> =
                    rank(&self.query, &self.index, &self.usage, 24).into_iter().map(|(i, _)| self.index[i].clone()).collect();
                with_quick_key(&self.quick, &self.query, with_conversion(&self.query, group_results(calc(&self.query), &ranked)))
            }
            Tab::Files => file_groups(&self.files, &self.query),
            Tab::Actions => with_quick_key(&self.quick, &self.query, action_groups(&self.query)),
            Tab::Clipboard => clip_groups(&clipboard::snapshot(), &self.query),
        };
        self.hits = self.groups.iter().flat_map(|g| g.hits.iter().cloned()).collect();
        self.selected = 0;
        let mut lines = Vec::new();
        let mut n = 0i32;
        let groups = self.groups.clone();
        for (gi, g) in groups.iter().enumerate() {
            lines.push(SpotlightLine {
                header: true,
                gap: gi > 0,
                title: g.title.into(),
                kind: "".into(),
                glyph: "".into(),
                icon: Default::default(),
                has_icon: false,
                index: -1,
            });
            for hit in &g.hits {
                lines.push(self.line_for(hit, n));
                n += 1;
            }
        }
        self.preview_open = false;
        self.view.ui.set_preview_open(false);
        self.view.ui.set_query(self.query.as_str().into());
        self.view.ui.set_lines(ModelRc::new(VecModel::from(lines)));
        self.view.ui.set_selected(0);
        // Кнопки вкладок видны, пока запрос пуст или открыта вкладка: при обычном поиске их место занимают результаты.
        self.view.ui.set_tabs_visible(self.tab != Tab::All || self.query.is_empty());
        self.view.ui.set_active_tab(active_button(self.tab));
        let note = if self.tab == Tab::All && self.hits.is_empty() { currency_note(&self.query) } else { "" };
        self.view.ui.set_empty_text(empty_text(self.tab, self.query.is_empty(), self.files_ready, !self.hits.is_empty(), note).into());
        self.reset_caret();
    }

    fn line_for(&mut self, hit: &Hit, index: i32) -> SpotlightLine {
        match hit {
            Hit::Calc(result) => SpotlightLine {
                header: false,
                gap: false,
                title: format!("= {result}").into(),
                kind: "Калькулятор".into(),
                glyph: "calculator".into(),
                icon: Default::default(),
                has_icon: false,
                index,
            },
            Hit::File(f) => {
                let px = (ICON_PT * self.scale).round() as i32;
                let image = self
                    .icons
                    .entry(f.path.clone())
                    .or_insert_with(|| load_icon(&f.path, px).map(|p| image_from_pixels(&p)))
                    .clone();
                SpotlightLine {
                    header: false,
                    gap: false,
                    title: f.name.as_str().into(),
                    kind: files::kind_label(f).into(),
                    glyph: if image.is_some() { "" } else { "folder" }.into(),
                    has_icon: image.is_some(),
                    icon: image.unwrap_or_default(),
                    index,
                }
            }
            Hit::Convert(result, note) => SpotlightLine {
                header: false,
                gap: false,
                title: format!("= {result}").into(),
                kind: note.as_str().into(),
                glyph: "calculator".into(),
                icon: Default::default(),
                has_icon: false,
                index,
            },
            Hit::Quick(key) => {
                let (glyph, icon) = match &key.target {
                    QuickTarget::Action(id) => (actions::def_of(*id).map_or("gear", |d| d.glyph), None),
                    QuickTarget::Open(t) => {
                        let px = (ICON_PT * self.scale).round() as i32;
                        let image = self.icons.entry(t.clone()).or_insert_with(|| load_icon(t, px).map(|p| image_from_pixels(&p))).clone();
                        ("", image)
                    }
                };
                SpotlightLine {
                    header: false,
                    gap: false,
                    title: quick::label(key).into(),
                    kind: format!("Клавиша {}", key.trigger).into(),
                    glyph: glyph.into(),
                    has_icon: icon.is_some(),
                    icon: icon.unwrap_or_default(),
                    index,
                }
            }
            Hit::Action(a) => SpotlightLine {
                header: false,
                gap: false,
                title: a.title.into(),
                kind: match quick::trigger_for(&self.quick, a.id) {
                    Some(trigger) => format!("Запрос: {trigger}"),
                    None => String::new(),
                }
                .into(),
                glyph: a.glyph.into(),
                icon: Default::default(),
                has_icon: false,
                index,
            },
            Hit::Clip(text, at) => SpotlightLine {
                header: false,
                gap: false,
                title: clipboard::preview(text, 70).into(),
                kind: clipboard::age_label(now_secs().saturating_sub(*at)).into(),
                glyph: "clipboard".into(),
                icon: Default::default(),
                has_icon: false,
                index,
            },
            Hit::Entry(e) => {
                let (icon, has_icon) = if e.kind == Kind::App {
                    let px = (ICON_PT * self.scale).round() as i32;
                    let image = self
                        .icons
                        .entry(e.target.clone())
                        .or_insert_with(|| load_icon(&e.target, px).map(|p| image_from_pixels(&p)))
                        .clone();
                    (image.clone().unwrap_or_default(), image.is_some())
                } else {
                    (Default::default(), false)
                };
                SpotlightLine {
                    header: false,
                    gap: false,
                    title: e.title.as_str().into(),
                    kind: if e.kind == Kind::App { "Приложение" } else { "Параметры Windows" }.into(),
                    glyph: e.glyph.into(),
                    icon,
                    has_icon,
                    index,
                }
            }
        }
    }

    fn reset_caret(&mut self) {
        self.caret_on = true;
        self.blink_at = Instant::now() + Duration::from_millis(BLINK_MS);
        self.view.ui.set_caret_on(true);
    }

    fn select(&mut self, n: usize) {
        if self.hits.is_empty() {
            return;
        }
        self.selected = n.min(self.hits.len() - 1);
        self.view.ui.set_selected(self.selected as i32);
        if self.preview_open {
            self.show_preview();
        }
    }

    /// Ctrl+Y: открывает предпросмотр выбранного файла или копии из буфера, повторное нажатие закрывает.
    fn toggle_preview(&mut self) {
        if self.preview_open {
            self.preview_open = false;
            self.view.ui.set_preview_open(false);
        } else {
            self.preview_open = true;
            self.show_preview();
        }
    }

    /// Заполняет панель предпросмотра по выбранному результату; для остальных результатов просмотр не открывается.
    fn show_preview(&mut self) {
        let (preview, image) = match self.hits.get(self.selected) {
            Some(Hit::File(f)) => {
                let px = ((PREVIEW_PT - 16.0) * self.scale).round() as i32;
                let image = load_icon(&f.path, px).map(|p| image_from_pixels(&p));
                (quicklook::describe(f, &files::kind_label(f), now_secs()), image)
            }
            Some(Hit::Clip(text, at)) => (
                quicklook::Preview {
                    title: "Из буфера обмена".to_string(),
                    meta: format!("{} · {}", clipboard::age_label(now_secs().saturating_sub(*at)), quicklook::human_size(text.len() as u64)),
                    text: quicklook::head_lines(text.as_bytes(), quicklook::TEXT_LINES, quicklook::LINE_CHARS),
                },
                None,
            ),
            _ => {
                self.preview_open = false;
                self.view.ui.set_preview_open(false);
                return;
            }
        };
        self.view.ui.set_preview_title(preview.title.into());
        self.view.ui.set_preview_meta(preview.meta.into());
        self.view.ui.set_preview_text(preview.text.into());
        self.view.ui.set_preview_has_image(image.is_some());
        self.view.ui.set_preview_image(image.unwrap_or_default());
        self.view.ui.set_preview_open(true);
    }

    /// `reveal`: Ctrl+Enter, у файла «показать в Проводнике» вместо «открыть».
    fn launch_of(&self, n: usize, reveal: bool) -> Option<Launch> {
        Some(match self.hits.get(n)? {
            Hit::Calc(result) => Launch::Copy(result.clone()),
            Hit::Entry(e) => Launch::Entry(e.clone()),
            Hit::File(f) => Launch::File { path: f.path.clone(), reveal },
            Hit::Action(a) => Launch::Action(a.id),
            Hit::Clip(text, _) => Launch::Copy(text.clone()),
            Hit::Convert(result, _) => Launch::Copy(result.clone()),
            Hit::Quick(key) => match &key.target {
                QuickTarget::Action(id) => Launch::Action(*id),
                QuickTarget::Open(target) => Launch::Open(target.clone()),
            },
        })
    }

    /// Переключает вкладку: запрос остаётся, список строится заново. Без индекса файлов просит его у владельца.
    fn switch_tab(&mut self, tab: Tab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.wants_files = tab == Tab::Files && !self.files_ready;
        self.rebuild();
    }

    /// Высота видимой панели в пикселях окна.
    fn panel_px(&self) -> u32 {
        (self.view.ui.get_panel_height() * self.scale).ceil() as u32
    }

    /// Мышь, клавиатура и потеря активности. Закрытие и запуск возвращаются, исполняет их владелец окна.
    pub fn handle_input(&mut self) -> Outcome {
        let mut query_changed = false;
        let (w, panel) = (self.win.rect().w, self.panel_px() as i32);
        for ev in self.win.take_input() {
            match ev {
                InputEvent::Char(c) => {
                    self.query.push(c);
                    query_changed = true;
                }
                InputEvent::Key { vk, ctrl, shift } => match vk {
                    // Esc сначала закрывает предпросмотр, второй раз сам Spotlight.
                    VK_ESCAPE if self.preview_open => self.toggle_preview(),
                    VK_ESCAPE => return Outcome::Close,
                    VK_Y if ctrl => self.toggle_preview(),
                    VK_RETURN => {
                        if let Some(l) = self.launch_of(self.selected, ctrl) {
                            return Outcome::Launch(l);
                        }
                    }
                    // Ctrl+1 «Приложения» (окном), Ctrl+2..4 вкладки, Ctrl+0 обычный поиск.
                    VK_0..=VK_4 if ctrl => match vk - VK_0 {
                        0 => self.switch_tab(Tab::All),
                        1 => return Outcome::OpenApps,
                        // Ctrl+2 это кнопка 1 («Файлы»), Ctrl+3 кнопка 2, Ctrl+4 кнопка 3.
                        n => self.switch_tab(tab_of_button(n as usize - 1).unwrap_or(Tab::All)),
                    },
                    // Backspace в пустом поле выводит из вкладки, как «назад».
                    VK_BACK if self.query.is_empty() && self.tab != Tab::All => self.switch_tab(Tab::All),
                    VK_BACK => {
                        if ctrl {
                            delete_word(&mut self.query);
                        } else {
                            self.query.pop();
                        }
                        query_changed = true;
                    }
                    VK_UP | VK_DOWN if !self.hits.is_empty() => {
                        let n = self.hits.len();
                        let next = if vk == VK_DOWN { (self.selected + 1) % n } else { (self.selected + n - 1) % n };
                        self.select(next);
                        self.reset_caret();
                    }
                    VK_TAB if !self.hits.is_empty() => {
                        self.select(next_section(&section_starts(&self.groups), self.selected, shift));
                    }
                    VK_V if ctrl => {
                        if let Some(text) = clipboard_text() {
                            self.query.push_str(&sanitize_paste(&text));
                            query_changed = true;
                        }
                    }
                    _ => {}
                },
                InputEvent::Down { x, y } => {
                    if x < 0 || y < 0 || x >= w || y >= panel {
                        return Outcome::Close;
                    }
                    self.view.dispatch(&ev);
                }
                InputEvent::Message { msg, .. } if msg == WM_ACTIVATE => {
                    if self.win.is_foreground() {
                        self.had_focus = true;
                    } else if self.had_focus {
                        return Outcome::Close;
                    }
                }
                other => self.view.dispatch(&other),
            }
        }
        if query_changed {
            self.rebuild();
        }
        let tab_click = self.tab_clicked.replace(-1);
        if tab_click == 0 {
            return Outcome::OpenApps;
        }
        if let Some(tab) = usize::try_from(tab_click).ok().and_then(tab_of_button) {
            // Повторный клик по той же вкладке возвращает обычный поиск.
            self.switch_tab(if self.tab == tab { Tab::All } else { tab });
        }
        let clicked = self.clicked.replace(-1);
        if clicked >= 0 {
            if let Some(l) = self.launch_of(clicked as usize, false) {
                return Outcome::Launch(l);
            }
        }
        let hovered = self.hovered.replace(-1);
        if hovered >= 0 && hovered as usize != self.selected {
            self.select(hovered as usize);
        }
        Outcome::Stay
    }

    /// Кадр интерфейса поверх стекла. Стекло пересчитывается только при смене высоты панели.
    pub fn present(&mut self) -> Result<()> {
        let Some(content) = self.view.render_if_changed() else { return Ok(()) };
        let (w, ph) = (self.frame.width, self.panel_px().min(self.frame.height));
        if self.glass.as_ref().map(|(h, _)| *h) != Some(ph) {
            let inner = self.blurred.region(self.origin.0, self.origin.1, w, ph);
            let glass = cpu::render(&inner, &GlassParams { blur_radius: 0, ..self.params });
            self.glass = Some((ph, glass));
        }
        let Some((_, glass)) = &self.glass else { return Ok(()) };
        self.frame.pixels.fill(0);
        self.frame.pixels[..glass.pixels.len()].copy_from_slice(&glass.pixels);
        cpu::composite_over(&mut self.frame, &content);
        self.win.present(&self.frame)
    }

    /// Мигание курсора и появление окна. `true`, если нужен ещё кадр.
    pub fn tick(&mut self) -> Result<bool> {
        let mut animating = false;
        if self.win.is_foreground() {
            self.had_focus = true;
        }
        if self.focus_retries > 0 {
            if self.had_focus {
                self.focus_retries = 0;
            } else if Instant::now() >= self.next_focus_at {
                self.win.focus();
                self.focus_retries -= 1;
                self.next_focus_at = Instant::now() + Duration::from_millis(FOCUS_RETRY_MS);
            }
        }
        if let Some(start) = self.appear {
            let t = ease_out_cubic(progress(start, APPEAR_MS));
            let dy = (-(APPEAR_DY_PT * self.scale) * (1.0 - t)).round() as i32;
            self.win.set_blend((255.0 * t).round() as u8, dy)?;
            if t >= 1.0 {
                self.appear = None;
            } else {
                animating = true;
            }
        }
        // Курсы валют догрузились (или не вышло): запрос про валюты получает ответ или пояснение.
        if currency::take_updated() {
            self.rebuild();
        }
        if Instant::now() >= self.blink_at {
            self.caret_on = !self.caret_on;
            self.view.ui.set_caret_on(self.caret_on);
            self.blink_at = Instant::now() + Duration::from_millis(BLINK_MS);
        }
        Ok(animating || self.view.animating())
    }

    /// Через сколько миллисекунд окну нужен следующий кадр (мигание курсора).
    pub fn next_wake_ms(&self) -> u32 {
        let mut next = self.blink_at;
        if self.focus_retries > 0 {
            next = next.min(self.next_focus_at);
        }
        next.saturating_duration_since(Instant::now()).as_millis() as u32 + 1
    }

    /// Закрытие после запуска: фокус отдаёт запущенное приложение, возвращать прежнему окну не надо.
    pub fn launched(&mut self) {
        self.restore_focus = false;
    }
}

impl Drop for SpotlightWin {
    fn drop(&mut self) {
        self.win.capture_mouse(false);
        if self.restore_focus && self.win.is_foreground() && self.previous != 0 {
            unsafe {
                let prev = windows::Win32::Foundation::HWND(self.previous as *mut _);
                if IsWindow(Some(prev)).as_bool() {
                    let _ = SetForegroundWindow(prev);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, kind: Kind) -> Entry {
        Entry {
            title: title.into(),
            norm: title.to_lowercase(),
            keywords: vec![],
            target: format!("t:{title}"),
            kind,
            glyph: "gear",
        }
    }

    fn titles(g: &Group) -> Vec<String> {
        g.hits
            .iter()
            .map(|h| match h {
                Hit::Calc(r) => format!("={r}"),
                Hit::Entry(e) => e.title.clone(),
                Hit::File(f) => f.name.clone(),
                Hit::Action(a) => a.title.to_string(),
                Hit::Clip(t, _) => t.clone(),
                Hit::Quick(k) => format!("quick:{}", k.trigger),
                Hit::Convert(r, _) => format!("conv:{r}"),
            })
            .collect()
    }

    #[test]
    fn nothing_found_gives_no_groups() {
        assert!(group_results(None, &[]).is_empty());
    }

    #[test]
    fn best_match_then_apps_then_settings() {
        let ranked = vec![
            entry("Звук", Kind::Setting),
            entry("Звукозапись", Kind::App),
            entry("Звуковой микшер", Kind::Setting),
            entry("Zvuk", Kind::App),
        ];
        let g = group_results(None, &ranked);
        let names: Vec<_> = g.iter().map(|g| g.title).collect();
        assert_eq!(names, vec!["Лучшее совпадение", "Приложения", "Системные настройки"]);
        assert_eq!(titles(&g[0]), vec!["Звук"]);
        assert_eq!(titles(&g[1]), vec!["Звукозапись", "Zvuk"]);
        assert_eq!(titles(&g[2]), vec!["Звуковой микшер"]);
    }

    #[test]
    fn calculator_takes_the_best_slot_and_every_entry_follows() {
        let ranked = vec![entry("Калькулятор", Kind::App), entry("Дисплей", Kind::Setting)];
        let g = group_results(Some("8".into()), &ranked);
        assert_eq!(titles(&g[0]), vec!["=8"]);
        assert_eq!(titles(&g[1]), vec!["Калькулятор"]);
        assert_eq!(titles(&g[2]), vec!["Дисплей"]);
    }

    #[test]
    fn total_rows_never_exceed_the_limit() {
        let mut ranked = Vec::new();
        for i in 0..10 {
            ranked.push(entry(&format!("App {i}"), Kind::App));
            ranked.push(entry(&format!("Set {i}"), Kind::Setting));
        }
        for calc in [None, Some("1".to_string())] {
            let g = group_results(calc, &ranked);
            let rows: usize = g.iter().map(|g| g.hits.len()).sum();
            assert!(rows <= MAX_ROWS, "{rows} rows");
            assert!(g.iter().all(|g| !g.hits.is_empty()));
        }
    }

    fn file(name: &str, dir: bool, mtime: u64) -> FileEntry {
        FileEntry { path: format!(r"C:\x\{name}"), name: name.into(), norm: crate::search::normalize(name), dir, mtime }
    }

    #[test]
    fn files_tab_shows_recent_files_without_a_query_and_matches_with_one() {
        let files = vec![file("old.txt", false, 1), file("report.pdf", false, 9), file("Reports", true, 5)];
        let recent = file_groups(&files, "");
        assert_eq!(recent[0].title, "Недавние файлы");
        assert_eq!(titles(&recent[0]), vec!["report.pdf", "old.txt"], "newest first, folders excluded");
        let found = file_groups(&files, "report");
        assert_eq!(found[0].title, "Файлы");
        assert_eq!(found[0].hits.len(), 2);
        assert!(file_groups(&files, "zzzz").is_empty());
        assert!(file_groups(&[], "").is_empty());
    }

    #[test]
    fn clipboard_tab_filters_case_insensitively_and_keeps_order() {
        let items = vec![
            ClipItem { text: "Hello World".into(), at: 30 },
            ClipItem { text: "other".into(), at: 20 },
            ClipItem { text: "say hello".into(), at: 10 },
        ];
        assert_eq!(titles(&clip_groups(&items, "")[0]), vec!["Hello World", "other", "say hello"]);
        assert_eq!(titles(&clip_groups(&items, "  HELLO ")[0]), vec!["Hello World", "say hello"]);
        assert!(clip_groups(&items, "nothing").is_empty());
        assert!(clip_groups(&[], "").is_empty());
    }

    #[test]
    fn actions_tab_lists_the_catalog_start_and_finds_by_query() {
        let all = action_groups("");
        assert_eq!(all[0].title, "Действия");
        assert_eq!(all[0].hits.len(), MAX_ROWS);
        assert_eq!(titles(&action_groups("lock")[0])[0], "Заблокировать экран");
        assert!(action_groups("qwertyuiop").is_empty());
    }

    #[test]
    fn quick_key_goes_first_and_its_action_is_not_listed_twice() {
        let keys = quick::resolve(&[("gh".into(), "https://github.com".into())]);
        let mut ranked = vec![entry("Блокнот", Kind::App)];
        ranked.push(entry("Звук", Kind::Setting));
        // Обычный поиск: «lk» целиком запускает блокировку, найденное идёт ниже.
        let g = with_quick_key(&keys, "lk", group_results(None, &ranked));
        assert_eq!(g[0].title, "Быстрая клавиша");
        assert_eq!(titles(&g[0]), vec!["quick:lk"]);
        assert!(g.len() > 1, "usual results follow");
        // Вкладка действий: блокировка не повторяется второй раз.
        let g = with_quick_key(&keys, "lk", action_groups("lock"));
        let locks = g.iter().flat_map(|g| &g.hits).filter(|h| matches!(h, Hit::Action(a) if a.id == ActionId::Lock)).count();
        assert_eq!(locks, 0);
        assert!(matches!(&g[0].hits[0], Hit::Quick(k) if k.target == QuickTarget::Action(ActionId::Lock)));
        // Частичный запрос ничего не добавляет.
        assert_eq!(with_quick_key(&keys, "l", group_results(None, &ranked)).len(), group_results(None, &ranked).len());
        // Открывающая клавиша.
        let g = with_quick_key(&keys, "gh", Vec::new());
        assert!(matches!(&g[0].hits[0], Hit::Quick(k) if k.target == QuickTarget::Open("https://github.com".into())));
    }

    #[test]
    fn conversion_goes_first_and_plain_queries_are_left_alone() {
        let ranked = vec![entry("Километр", Kind::App)];
        let g = with_conversion("10 km in mi", group_results(None, &ranked));
        assert_eq!(g[0].title, "Конвертация");
        assert_eq!(titles(&g[0]), vec!["conv:6,21371 mi"]);
        assert!(g.len() > 1, "apps found for the same words still follow");
        let plain = with_conversion("firefox", group_results(None, &ranked));
        assert_eq!(plain.len(), group_results(None, &ranked).len());
        assert!(matches!(with_conversion("5 ft to cm", Vec::new())[0].hits[0], Hit::Convert(..)));
    }

    #[test]
    fn trimming_never_leaves_empty_groups_or_too_many_rows() {
        let mut g = vec![
            Group { title: "a", hits: (0..5).map(|i| Hit::Calc(i.to_string())).collect() },
            Group { title: "b", hits: (0..5).map(|i| Hit::Calc(i.to_string())).collect() },
            Group { title: "c", hits: vec![Hit::Calc("x".into())] },
        ];
        trim_rows(&mut g, 7);
        let rows: usize = g.iter().map(|g| g.hits.len()).sum();
        assert_eq!(rows, 7);
        assert_eq!(g.len(), 2, "the group that lost every row is dropped");
    }

    #[test]
    fn empty_tabs_explain_themselves_and_search_stays_silent() {
        assert_eq!(empty_text(Tab::All, true, true, false, ""), "");
        assert_eq!(empty_text(Tab::All, false, true, false, "Загружаю курсы валют…"), "Загружаю курсы валют…");
        assert_eq!(empty_text(Tab::Files, true, false, false, ""), "Индексирую файлы…");
        assert_eq!(empty_text(Tab::Files, false, true, false, ""), "Ничего не найдено");
        assert_eq!(empty_text(Tab::Clipboard, true, true, false, ""), "История буфера обмена пуста");
        assert_eq!(empty_text(Tab::Clipboard, false, true, false, ""), "Ничего не найдено");
        assert_eq!(empty_text(Tab::Actions, true, true, false, ""), "Нет такого действия");
        assert_eq!(empty_text(Tab::Files, true, false, true, ""), "", "rows present: no caption");
    }

    #[test]
    fn tab_buttons_map_to_tabs_and_apps_is_not_a_tab() {
        assert_eq!(tab_of_button(0), None, "the first button opens the Apps window");
        assert_eq!([tab_of_button(1), tab_of_button(2), tab_of_button(3)], [Some(Tab::Files), Some(Tab::Actions), Some(Tab::Clipboard)]);
        assert_eq!(tab_of_button(4), None);
        assert_eq!([active_button(Tab::All), active_button(Tab::Files), active_button(Tab::Clipboard)], [-1, 1, 3]);
        assert_eq!(TAB_BUTTONS.len(), 4);
    }

    #[test]
    fn section_jump_wraps_in_both_directions() {
        let starts = [0, 1, 5];
        assert_eq!(next_section(&starts, 0, false), 1);
        assert_eq!(next_section(&starts, 3, false), 5);
        assert_eq!(next_section(&starts, 5, false), 0);
        assert_eq!(next_section(&starts, 5, true), 1);
        assert_eq!(next_section(&starts, 0, true), 5);
        assert_eq!(next_section(&[], 0, false), 0);
    }

    #[test]
    fn ctrl_backspace_removes_a_word() {
        let mut s = "настройки звука  ".to_string();
        delete_word(&mut s);
        assert_eq!(s, "настройки ");
        delete_word(&mut s);
        assert_eq!(s, "");
        delete_word(&mut s);
        assert_eq!(s, "");
    }

    #[test]
    fn pasted_text_is_one_clean_line() {
        assert_eq!(sanitize_paste("first line\r\nsecond"), "first line");
        assert_eq!(sanitize_paste("tab\there"), "tabhere");
        assert_eq!(sanitize_paste(&"я".repeat(500)).chars().count(), 200);
        assert_eq!(sanitize_paste(""), "");
    }
}
