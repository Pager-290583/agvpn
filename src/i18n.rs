//! Minimal translation layer: English source strings, per-language dictionaries.

use std::collections::HashMap;
use std::sync::OnceLock;

pub const LANGUAGES: &[(&str, &str)] = &[("system", ""), ("en", "English"), ("ru", "Русский")];

struct Catalog {
    lang: String,
    map: HashMap<&'static str, &'static str>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

pub fn system_language() -> String {
    for var in ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                let first = value.split(':').next().unwrap_or("");
                let lang = first.split('_').next().unwrap_or("").split('.').next().unwrap_or("");
                if !lang.is_empty() {
                    return lang.to_lowercase();
                }
            }
        }
    }
    "en".into()
}

/// Selects the UI language. Only the first call has an effect (a restart applies changes).
pub fn setup(language: &str) {
    let lang = if language.is_empty() || language == "system" { system_language() } else { language.to_string() };
    let map = if lang == "ru" { crate::i18n_ru::RU.iter().copied().collect() } else { HashMap::new() };
    let _ = CATALOG.set(Catalog { lang, map });
}

pub fn current() -> &'static str {
    CATALOG.get().map(|c| c.lang.as_str()).unwrap_or("en")
}

pub fn gettext(text: &str) -> String {
    CATALOG
        .get()
        .and_then(|c| c.map.get(text).copied())
        .unwrap_or(text)
        .to_string()
}

/// Plural forms; Russian uses three forms joined with '|' in the catalog.
pub fn ngettext(one: &str, many: &str, n: i64) -> String {
    let key = format!("{one}|{many}");
    let translated = CATALOG.get().and_then(|c| c.map.get(key.as_str()).copied());
    let form = match translated {
        None => if n == 1 { one } else { many },
        Some(t) => {
            let forms: Vec<&str> = t.split('|').collect();
            plural_form(&forms, n, current())
        }
    };
    form.replace("{n}", &n.to_string())
}

fn plural_form<'a>(forms: &[&'a str], n: i64, lang: &str) -> &'a str {
    if lang == "ru" && forms.len() == 3 {
        let (m10, m100) = (n % 10, n % 100);
        if m10 == 1 && m100 != 11 {
            forms[0]
        } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
            forms[1]
        } else {
            forms[2]
        }
    } else if n == 1 {
        forms[0]
    } else {
        forms[forms.len() - 1]
    }
}

/// `tr!("text")` translates; `tr!("Hi {name}", name = value)` also substitutes `{name}`.
#[macro_export]
macro_rules! tr {
    ($text:expr) => { $crate::i18n::gettext($text) };
    ($text:expr, $($key:ident = $value:expr),+ $(,)?) => {{
        let mut s = $crate::i18n::gettext($text);
        $( s = s.replace(concat!("{", stringify!($key), "}"), &$value.to_string()); )+
        s
    }};
}

#[cfg(test)]
mod tests {
    use super::plural_form;

    #[test]
    fn russian_plurals() {
        let forms = ["one", "few", "many"];
        let pick = |n| plural_form(&forms, n, "ru");
        assert_eq!(pick(1), "one");
        assert_eq!(pick(21), "one");
        assert_eq!(pick(11), "many");
        assert_eq!(pick(3), "few");
        assert_eq!(pick(13), "many");
        assert_eq!(pick(25), "many");
    }
}
