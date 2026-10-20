//! English inflections, the way ActiveSupport does them: `pluralize`,
//! `singularize`, `camelize`, `underscore`, `classify`, `tableize`.
//!
//! The default rules are ActiveSupport's. A project can add acronyms,
//! irregular words and uncountable words in
//! `config/initializers/inflections.rb`; `configure` loads them.

use std::sync::{LazyLock, RwLock};

use regex::Regex;

struct Rule {
    re: Regex,
    replacement: &'static str,
}

fn rules(list: &[(&str, &'static str)]) -> Vec<Rule> {
    // ActiveSupport applies the rule defined last first.
    list.iter()
        .rev()
        .map(|(pattern, replacement)| Rule {
            re: Regex::new(&format!("(?i){pattern}")).expect("valid rule"),
            replacement,
        })
        .collect()
}

static PLURALS: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    rules(&[
        ("$", "s"),
        ("s$", "s"),
        ("^(ax|test)is$", "${1}es"),
        ("(octop|vir)us$", "${1}i"),
        ("(octop|vir)i$", "${1}i"),
        ("(alias|status)$", "${1}es"),
        ("(bu)s$", "${1}ses"),
        ("(buffal|tomat)o$", "${1}oes"),
        ("([ti])um$", "${1}a"),
        ("([ti])a$", "${1}a"),
        ("sis$", "ses"),
        ("(?:([^f])fe|([lr])f)$", "${1}${2}ves"),
        ("(hive)$", "${1}s"),
        ("([^aeiouy]|qu)y$", "${1}ies"),
        ("(x|ch|ss|sh)$", "${1}es"),
        ("(matr|vert|ind)(?:ix|ex)$", "${1}ices"),
        ("^(m|l)ouse$", "${1}ice"),
        ("^(m|l)ice$", "${1}ice"),
        ("^(ox)$", "${1}en"),
        ("^(oxen)$", "${1}"),
        ("(quiz)$", "${1}zes"),
    ])
});

static SINGULARS: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    rules(&[
        ("s$", ""),
        ("(ss)$", "${1}"),
        ("(n)ews$", "${1}ews"),
        ("([ti])a$", "${1}um"),
        ("((a)naly|(b)a|(d)iagno|(p)arenthe|(p)rogno|(s)ynop|(t)he)(sis|ses)$", "${1}sis"),
        ("(^analy)(sis|ses)$", "${1}sis"),
        ("([^f])ves$", "${1}fe"),
        ("(hive)s$", "${1}"),
        ("(tive)s$", "${1}"),
        ("([lr])ves$", "${1}f"),
        ("([^aeiouy]|qu)ies$", "${1}y"),
        ("(s)eries$", "${1}eries"),
        ("(m)ovies$", "${1}ovie"),
        ("(x|ch|ss|sh)es$", "${1}"),
        ("^(m|l)ice$", "${1}ouse"),
        ("(bus)(es)?$", "${1}"),
        ("(o)es$", "${1}"),
        ("(shoe)s$", "${1}"),
        ("(cris|test)(is|es)$", "${1}is"),
        ("^(a)x[ie]s$", "${1}xis"),
        ("(octop|vir)(us|i)$", "${1}us"),
        ("(alias|status)(es)?$", "${1}"),
        ("^(ox)en", "${1}"),
        ("(vert|ind)ices$", "${1}ex"),
        ("(matr)ices$", "${1}ix"),
        ("(quiz)zes$", "${1}"),
        ("(database)s$", "${1}"),
    ])
});

#[derive(Debug, Clone, Default)]
pub struct Custom {
    /// `inflect.acronym "API"`
    pub acronyms: Vec<String>,
    /// `inflect.irregular "person", "people"`: (singular, plural)
    pub irregulars: Vec<(String, String)>,
    pub uncountables: Vec<String>,
}

static DEFAULT_IRREGULARS: &[(&str, &str)] = &[
    ("person", "people"),
    ("man", "men"),
    ("child", "children"),
    ("sex", "sexes"),
    ("move", "moves"),
    ("zombie", "zombies"),
];
static DEFAULT_UNCOUNTABLES: &[&str] =
    &["equipment", "information", "rice", "money", "species", "series", "fish", "sheep", "jeans", "police"];

static CUSTOM: LazyLock<RwLock<Custom>> = LazyLock::new(|| RwLock::new(Custom::default()));

/// Sets the project's own inflections.
pub fn configure(custom: Custom) {
    *CUSTOM.write().expect("inflections lock") = custom;
}

/// Reads `inflect.acronym`, `inflect.irregular` and `inflect.uncountable`
/// lines from an inflections initializer.
pub fn parse_initializer(text: &str) -> Custom {
    let mut custom = Custom::default();
    let quoted = Regex::new(r#"["']([^"']+)["']|%w\[([^\]]*)\]|%w\(([^)]*)\)"#).expect("valid");
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let words: Vec<String> = quoted
            .captures_iter(line)
            .flat_map(|c| {
                if let Some(m) = c.get(1) {
                    vec![m.as_str().to_string()]
                } else {
                    c.get(2)
                        .or(c.get(3))
                        .map(|m| m.as_str().split_whitespace().map(str::to_string).collect())
                        .unwrap_or_default()
                }
            })
            .collect();
        if line.contains(".acronym") {
            custom.acronyms.extend(words);
        } else if line.contains(".irregular") && words.len() >= 2 {
            custom.irregulars.push((words[0].clone(), words[1].clone()));
        } else if line.contains(".uncountable") {
            custom.uncountables.extend(words);
        }
    }
    custom
}

fn apply(word: &str, rules: &[Rule]) -> String {
    for rule in rules {
        if rule.re.is_match(word) {
            return rule.re.replace(word, rule.replacement).into_owned();
        }
    }
    word.to_string()
}

/// Splits `line_item` into `line_` and `item`, so only the last word
/// is inflected.
fn split_last(word: &str) -> (&str, &str) {
    match word.rfind('_') {
        Some(i) => (&word[..=i], &word[i + 1..]),
        None => ("", word),
    }
}

fn inflect(word: &str, plural: bool) -> String {
    if word.is_empty() {
        return String::new();
    }
    let custom = CUSTOM.read().expect("inflections lock");
    let (head, last) = split_last(word);
    let lower = last.to_lowercase();
    let uncountable = DEFAULT_UNCOUNTABLES.contains(&lower.as_str()) || custom.uncountables.contains(&lower);
    if uncountable {
        return word.to_string();
    }
    let irregulars =
        DEFAULT_IRREGULARS.iter().map(|(s, p)| (s.to_string(), p.to_string())).chain(custom.irregulars.iter().cloned());
    for (singular, plural_form) in irregulars {
        if plural && (lower == singular || lower == plural_form) {
            return format!("{head}{plural_form}");
        }
        if !plural && (lower == plural_form || lower == singular) {
            return format!("{head}{singular}");
        }
    }
    let rules = if plural { &*PLURALS } else { &*SINGULARS };
    format!("{head}{}", apply(last, rules))
}

pub fn pluralize(word: &str) -> String {
    inflect(word, true)
}

pub fn singularize(word: &str) -> String {
    inflect(word, false)
}

/// `admin/sales_charts` → `Admin::SalesCharts`, with acronyms
/// (`oauth_client` → `OAuthClient` when `OAuth` is an acronym).
pub fn camelize(path: &str) -> String {
    let custom = CUSTOM.read().expect("inflections lock");
    path.split('/')
        .map(|part| {
            part.split('_')
                .map(|w| {
                    if let Some(a) = custom.acronyms.iter().find(|a| a.to_lowercase() == w.to_lowercase()) {
                        return a.clone();
                    }
                    let mut c = w.chars();
                    match c.next() {
                        Some(f) => f.to_uppercase().chain(c).collect(),
                        None => String::new(),
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("::")
}

/// `Admin::PdfGenerator` → `admin/pdf_generator`, `HTMLParser` →
/// `html_parser`, with acronyms (`OAuthClient` → `oauth_client`).
pub fn underscore(name: &str) -> String {
    let custom = CUSTOM.read().expect("inflections lock");
    name.split("::")
        .map(|seg| {
            let mut seg = seg.to_string();
            for a in &custom.acronyms {
                // An acronym is one word: `OAuthClient` → `Oauth_Client` first.
                if let Some(pos) = seg.find(a.as_str()) {
                    let mut lower = a.to_lowercase();
                    let after = &seg[pos + a.len()..];
                    if after.chars().next().is_some_and(|c| c.is_uppercase()) {
                        lower.push('_');
                    }
                    let before = &seg[..pos];
                    let sep = if !before.is_empty() && !before.ends_with('_') { "_" } else { "" };
                    seg = format!("{before}{sep}{lower}{after}");
                }
            }
            underscore_word(&seg)
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn underscore_word(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower = i > 0 && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if i > 0 && chars[i - 1] != '_' && (prev_lower || (next_lower && chars[i - 1].is_uppercase())) {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(*c);
        }
    }
    out
}

/// `invoice_items` → `InvoiceItem`
pub fn classify(table: &str) -> String {
    camelize(&singularize(table))
}

/// `InvoiceItem` → `invoice_items`
pub fn tableize(class: &str) -> String {
    pluralize(&underscore(class))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_and_singular() {
        for (s, p) in [
            ("invoice", "invoices"),
            ("entity", "entities"),
            ("status", "statuses"),
            ("person", "people"),
            ("address", "addresses"),
            ("line_item", "line_items"),
            ("category", "categories"),
            ("money", "money"),
            ("analysis", "analyses"),
            ("wolf", "wolves"),
            ("box", "boxes"),
        ] {
            assert_eq!(pluralize(s), p, "pluralize {s}");
            assert_eq!(singularize(p), s, "singularize {p}");
        }
    }

    #[test]
    fn camelize_and_underscore() {
        assert_eq!(camelize("admin/sales_charts"), "Admin::SalesCharts");
        assert_eq!(underscore("Admin::PdfGenerator"), "admin/pdf_generator");
        assert_eq!(underscore("HTMLParser"), "html_parser");
        assert_eq!(underscore("UI"), "ui");
        assert_eq!(underscore("Oauth2Client"), "oauth2_client");
        assert_eq!(classify("invoice_items"), "InvoiceItem");
        assert_eq!(tableize("InvoiceItem"), "invoice_items");
    }

    #[test]
    fn reads_an_initializer() {
        let c = parse_initializer(
            "ActiveSupport::Inflector.inflections(:en) do |inflect|\n  inflect.acronym \"OAuth\"\n  inflect.uncountable %w[media]\n  inflect.irregular 'octopus', 'octopi'\nend\n",
        );
        assert_eq!(c.acronyms, vec!["OAuth"]);
        assert_eq!(c.uncountables, vec!["media"]);
        assert_eq!(c.irregulars, vec![("octopus".to_string(), "octopi".to_string())]);
    }
}
