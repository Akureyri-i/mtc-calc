//! The text players read, by key, in the language chosen in the settings: English, or Chinese.
//! Each language is a JSON file of key to text (`lang/en.json`, `lang/zh-CN.json`) built into the
//! program; `language.name` is the language's own name, shown in the settings. A key a translation
//! lacks shows in English. `{0}`, `{1}`… in a text are filled in by `tf`. The game's own words stay
//! as the game writes them in every language (charge names, shell types and names, weapon names,
//! key letters), since the player reads them in the game in English.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Language {
    #[default]
    English,
    Chinese,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::English, Language::Chinese];

    /// The language's own name, as its speakers write it.
    pub fn name(self) -> &'static str {
        self.table().get("language.name").copied().unwrap_or("?")
    }

    fn table(self) -> &'static HashMap<&'static str, &'static str> {
        match self {
            Language::English => &ENGLISH,
            Language::Chinese => &CHINESE,
        }
    }

    /// The language Windows shows its own text in: Chinese for any Chinese, else English.
    pub fn of_windows() -> Self {
        let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        // The primary language of a LANGID: its low 10 bits (Chinese is 0x04).
        if id & 0x3ff == 0x04 { Language::Chinese } else { Language::English }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

fn table(json: &'static str) -> HashMap<&'static str, &'static str> {
    let map: HashMap<String, String> = serde_json::from_str(json).expect("language file");
    map.into_iter().map(|(k, v)| (&*Box::leak(k.into_boxed_str()), &*Box::leak(v.into_boxed_str()))).collect()
}

static ENGLISH: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| table(include_str!("../lang/en.json")));
static CHINESE: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| table(include_str!("../lang/zh-CN.json")));

pub fn set_language(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

pub fn language() -> Language {
    if CURRENT.load(Ordering::Relaxed) == Language::Chinese as u8 { Language::Chinese } else { Language::English }
}

/// The text for `key` in the current language (English where the translation lacks it; the key
/// itself where English does too, which the tests rule out).
pub fn t(key: &str) -> &'static str {
    let english = || ENGLISH.get(key).copied();
    let text = match language() {
        Language::Chinese => CHINESE.get(key).copied().or_else(english),
        Language::English => english(),
    };
    text.unwrap_or_else(|| Box::leak(key.to_owned().into_boxed_str()))
}

/// As `t`, with `{0}`, `{1}`… filled in from `args`.
pub fn tf(key: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut text = t(key).to_owned();
    for (i, a) in args.iter().enumerate() {
        text = text.replace(&format!("{{{i}}}"), &a.to_string());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key used in the code is in the English file, the Chinese file has no key English
    /// lacks, and each Chinese text has the placeholders of its English one.
    #[test]
    fn language_files_agree() {
        let src: String = ["aim_strip.rs", "map_marks.rs", "panel.rs", "plan.rs", "settings.rs", "lib.rs"]
            .iter()
            .map(|f| std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(f)).expect("source"))
            .chain(["main.rs", "plan.rs", "loading.rs", "tray.rs", "marks.rs"].iter().filter_map(|f| std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mtc-hud/src").join(f)).ok()))
            .collect();
        let mut used = Vec::new();
        for call in ["t(\"", "tf(\""] {
            let mut from = 0;
            while let Some(i) = src[from..].find(call).map(|i| i + from) {
                from = i + call.len();
                // A call of `t` or `tf` itself, not of a name ending so (`split("`).
                if src[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let end = src[from..].find('"').expect("key end") + from;
                used.push(src[from..end].to_owned());
            }
        }
        let missing: Vec<&String> = used.iter().filter(|k| !ENGLISH.contains_key(k.as_str())).collect();
        assert!(missing.is_empty(), "keys missing from en.json: {missing:?}");
        let extra: Vec<&&str> = CHINESE.keys().filter(|k| !ENGLISH.contains_key(*k)).collect();
        assert!(extra.is_empty(), "zh-CN keys English lacks: {extra:?}");
        let untranslated: Vec<&&str> = ENGLISH.keys().filter(|k| !CHINESE.contains_key(*k)).collect();
        assert!(untranslated.is_empty(), "keys without a Chinese text: {untranslated:?}");
        for (k, en) in ENGLISH.iter() {
            let zh = CHINESE[k];
            for i in 0..4 {
                let p = format!("{{{i}}}");
                assert_eq!(en.contains(&p), zh.contains(&p), "{k}: placeholder {p}");
            }
        }
        let unused: Vec<&&str> = ENGLISH.keys().filter(|k| **k != "language.name" && !used.iter().any(|u| u == *k)).collect();
        assert!(unused.is_empty(), "keys no code uses: {unused:?}");
    }

    #[test]
    fn languages_are_named_in_themselves() {
        assert_eq!(Language::English.name(), "English");
        assert_eq!(Language::Chinese.name(), "\u{4e2d}\u{6587}");
    }

    #[test]
    fn placeholders_fill_in() {
        set_language(Language::English);
        assert_eq!(tf("panel.use", &[&"Medium"]), "Use Medium");
        set_language(Language::Chinese);
        assert!(tf("panel.use", &[&"Medium"]).contains("Medium"));
        set_language(Language::English);
    }
}
