//! Быстрые клавиши Spotlight: короткий запрос (две-три буквы), который сразу даёт нужное действие или открывает
//! файл, сайт или приложение. Свои задаются в конфиге строками `quick_key = "запрос=цель"`, где цель это
//! короткое имя действия (`lock`, `dark`, ...) или что угодно, что умеет открыть Windows: путь, адрес сайта,
//! `shell:AppsFolder\...`. Своя клавиша важнее встроенной с тем же запросом.

use crate::actions::{self, ActionId};
use crate::search::swap_layout;

/// Встроенные быстрые клавиши (действия по короткому имени).
pub const DEFAULTS: &[(&str, &str)] = &[
    ("lk", "lock"),
    ("sl", "sleep"),
    ("dm", "dark"),
    ("bt", "bluetooth"),
    ("mu", "mute"),
    ("ss", "screenshot"),
    ("tm", "taskmgr"),
    ("rb", "trash"),
    ("nl", "nightlight"),
    ("tr", "terminal"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickTarget {
    Action(ActionId),
    /// Путь, адрес сайта или `shell:`-адрес: открывается как двойным щелчком.
    Open(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickKey {
    /// Запрос в нижнем регистре.
    pub trigger: String,
    pub target: QuickTarget,
}

/// Запрос в виде, в котором он сравнивается с быстрыми клавишами: без пробелов по краям, в нижнем регистре.
pub fn trigger_form(s: &str) -> String {
    s.trim().to_lowercase()
}

fn target_of(text: &str) -> QuickTarget {
    match actions::by_name(text) {
        Some(a) => QuickTarget::Action(a.id),
        None => QuickTarget::Open(text.trim().to_string()),
    }
}

/// Все быстрые клавиши: сначала свои из конфига (при повторе запроса действует первая), затем встроенные,
/// которые они не перекрыли.
pub fn resolve(user: &[(String, String)]) -> Vec<QuickKey> {
    let mut keys: Vec<QuickKey> = Vec::new();
    let all = user.iter().map(|(k, t)| (k.as_str(), t.as_str())).chain(DEFAULTS.iter().copied());
    for (trigger, target) in all {
        let trigger = trigger_form(trigger);
        if trigger.is_empty() || target.trim().is_empty() || keys.iter().any(|k| k.trigger == trigger) {
            continue;
        }
        keys.push(QuickKey { trigger, target: target_of(target) });
    }
    keys
}

/// Быстрая клавиша, запрос которой набран целиком (запрос в чужой раскладке тоже подходит: «дл» вместо «lk»).
pub fn find<'a>(keys: &'a [QuickKey], query: &str) -> Option<&'a QuickKey> {
    let q = trigger_form(query);
    if q.is_empty() {
        return None;
    }
    let swapped = trigger_form(&swap_layout(&q));
    keys.iter().find(|k| k.trigger == q || k.trigger == swapped)
}

/// Запрос, которым вызывается действие: для подписи в списке действий.
pub fn trigger_for(keys: &[QuickKey], id: ActionId) -> Option<&str> {
    keys.iter().find(|k| k.target == QuickTarget::Action(id)).map(|k| k.trigger.as_str())
}

/// Название для строки результата: имя действия, имя файла или адрес сайта без схемы.
pub fn label(key: &QuickKey) -> String {
    match &key.target {
        QuickTarget::Action(id) => actions::def_of(*id).map_or("Действие", |d| d.title).to_string(),
        QuickTarget::Open(t) => {
            if let Some((_, rest)) = t.split_once("://") {
                return rest.trim_end_matches('/').to_string();
            }
            let name = t.rsplit(['\\', '/']).next().unwrap_or(t);
            name.rsplit_once('.').filter(|(stem, ext)| !stem.is_empty() && ext.len() <= 4).map_or(name, |(stem, _)| stem).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, t)| (k.to_string(), t.to_string())).collect()
    }

    #[test]
    fn defaults_point_at_real_actions_and_have_unique_triggers() {
        let keys = resolve(&[]);
        assert_eq!(keys.len(), DEFAULTS.len());
        assert!(keys.iter().all(|k| matches!(k.target, QuickTarget::Action(_))), "every default names a catalog action");
        let mut triggers: Vec<&str> = keys.iter().map(|k| k.trigger.as_str()).collect();
        triggers.sort();
        triggers.dedup();
        assert_eq!(triggers.len(), keys.len());
    }

    #[test]
    fn own_keys_come_first_and_override_defaults_with_the_same_trigger() {
        let keys = resolve(&user(&[("LK", "sleep"), ("gh", "https://github.com"), ("gh", "dark")]));
        assert_eq!(find(&keys, "lk").map(|k| &k.target), Some(&QuickTarget::Action(ActionId::Sleep)), "own key beats the default");
        assert_eq!(find(&keys, "gh").map(|k| &k.target), Some(&QuickTarget::Open("https://github.com".into())), "first duplicate wins");
        assert_eq!(keys[0].trigger, "lk");
    }

    #[test]
    fn find_needs_the_whole_trigger_and_tolerates_case_spaces_and_layout() {
        let keys = resolve(&[]);
        assert!(find(&keys, "l").is_none() && find(&keys, "lkx").is_none() && find(&keys, "").is_none());
        assert!(find(&keys, "  LK ").is_some());
        // «дл» набрано вместо «lk» в русской раскладке.
        assert_eq!(find(&keys, "дл").map(|k| &k.target), Some(&QuickTarget::Action(ActionId::Lock)));
    }

    #[test]
    fn blank_entries_are_ignored() {
        let keys = resolve(&user(&[("", "lock"), ("x", "  ")]));
        assert_eq!(keys.len(), DEFAULTS.len());
    }

    #[test]
    fn unknown_targets_are_opened_not_run() {
        let keys = resolve(&user(&[("notes", r"C:\Users\me\notes.txt")]));
        assert_eq!(find(&keys, "notes").map(|k| &k.target), Some(&QuickTarget::Open(r"C:\Users\me\notes.txt".into())));
    }

    #[test]
    fn actions_report_their_trigger() {
        let keys = resolve(&user(&[("zz", "lock")]));
        assert_eq!(trigger_for(&keys, ActionId::Lock), Some("zz"), "own key listed before the default one");
        assert_eq!(trigger_for(&keys, ActionId::Restart), None);
    }

    #[test]
    fn labels_are_readable() {
        let key = |t: &str| QuickKey { trigger: "x".into(), target: target_of(t) };
        assert_eq!(label(&key("lock")), "Заблокировать экран");
        assert_eq!(label(&key("https://github.com/")), "github.com");
        assert_eq!(label(&key(r"C:\Users\me\notes.txt")), "notes");
        assert_eq!(label(&key(r"C:\Tools\Folder")), "Folder");
    }
}
