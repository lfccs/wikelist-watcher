//! `Data\Localization\english\global.ini` parsing and lookup.
//!
//! Format: UTF-8 with BOM, CRLF line endings, `key=value` per line. Some keys
//! carry a `,P` suffix (platform variants); DataForge references strings as
//! `@key`. Values contain literal `\n` sequences for line breaks.

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Localization {
    /// lowercase key (without `,P`) -> value
    map: HashMap<String, String>,
}

impl Localization {
    pub fn parse(bytes: &[u8]) -> Self {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_start_matches('\u{feff}');
        let mut map: HashMap<String, String> = HashMap::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let key = key.trim();
            let base = key.strip_suffix(",P").unwrap_or(key).to_ascii_lowercase();
            // Prefer the plain key over the `,P` variant when both exist.
            if key.ends_with(",P") && map.contains_key(&base) {
                continue;
            }
            map.insert(base, value.to_string());
        }
        Self { map }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Look up a key such as `@TheCollector_Coin_Name` or `TheCollector_Coin_Name`.
    pub fn get(&self, key: &str) -> Option<&str> {
        let k = key.trim().trim_start_matches('@');
        if k.is_empty() {
            return None;
        }
        let k = k.strip_suffix(",P").unwrap_or(k).to_ascii_lowercase();
        self.map.get(&k).map(String::as_str)
    }

    /// Localized text with `\n` escapes turned into real newlines, or the key
    /// itself (without `@`) when missing or a placeholder.
    pub fn text(&self, key: &str) -> String {
        match self.get(key) {
            Some(v) if !is_placeholder(v) => v.replace("\\n", "\n"),
            _ => key.trim_start_matches('@').to_string(),
        }
    }

    /// Like [`text`] but returns None when the key is missing or a placeholder.
    pub fn text_opt(&self, key: &str) -> Option<String> {
        match self.get(key) {
            Some(v) if !is_placeholder(v) => Some(v.replace("\\n", "\n")),
            _ => None,
        }
    }
}

fn is_placeholder(v: &str) -> bool {
    matches!(v.trim(), "" | "@LOC_PLACEHOLDER" | "@LOC_UNINITIALIZED" | "@LOC_EMPTY" | "@blank_space")
}

/// Strip `<EM4>` style markup and expand `~mission(...)` tokens into readable text.
pub fn plain_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                // skip markup tag
                for d in chars.by_ref() {
                    if d == '>' {
                        break;
                    }
                }
            }
            '~' if chars.peek() == Some(&'m') => {
                let mut token = String::new();
                let mut depth = 0;
                let mut started = false;
                for d in chars.by_ref() {
                    if d == '(' {
                        depth += 1;
                        started = true;
                        continue;
                    }
                    if d == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    if started {
                        token.push(d);
                    }
                }
                let name = token.split('|').next().unwrap_or("").to_ascii_lowercase();
                out.push_str(match name.as_str() {
                    "destination" => "qualquer Wikelo Emporium",
                    "item" => "[item]",
                    "amount" => "[qtd]",
                    "total" => "[total]",
                    _ => "[...]",
                });
            }
            _ => out.push(c),
        }
    }
    collapse_blank_lines(&out)
}

fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(t);
        out.push('\n');
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_looks_up() {
        let ini = "\u{feff}TheCollector_Coin_Name=Wikelo Favor\r\nSome_Key,P=Platform\r\nTheCollector_Intro_Desc=Hello.\\n\\nBye.\r\n";
        let loc = Localization::parse(ini.as_bytes());
        assert_eq!(loc.get("@TheCollector_Coin_Name"), Some("Wikelo Favor"));
        assert_eq!(loc.get("thecollector_coin_name"), Some("Wikelo Favor"));
        assert_eq!(loc.get("@Some_Key"), Some("Platform"));
        assert_eq!(loc.text("@TheCollector_Intro_Desc"), "Hello.\n\nBye.");
        assert_eq!(loc.text("@Missing_Key"), "Missing_Key");
        assert_eq!(loc.text_opt("@Missing_Key"), None);
    }

    #[test]
    fn plain_text_strips_markup_and_tokens() {
        let s = "Bring to <EM4>~mission(destination|ListAll)</EM4> and I make.\n\n\n<EM4>Reputation Awarded:</EM4> 25";
        assert_eq!(plain_text(s), "Bring to qualquer Wikelo Emporium and I make.\n\nReputation Awarded: 25");
    }
}
