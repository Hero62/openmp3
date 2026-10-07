//! Local romanization (no network): Japanese kana (Hepburn), Korean Hangul
//! (Revised Romanization, algorithmic), Cyrillic and Greek (transliteration).
//! Kanji / Hanzi need a dictionary and are left as-is.
//!
//! Optional translation (off by default) uses the MyMemory public API.

use anyhow::Result;
use serde_json::Value;

fn kana(c: char) -> Option<&'static str> {
    // Hiragana and katakana share the table (katakana = hiragana + 0x60).
    let h = match c {
        '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60)?,
        _ => c,
    };
    Some(match h {
        'あ' => "a", 'い' => "i", 'う' => "u", 'え' => "e", 'お' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        'さ' => "sa", 'し' => "shi", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'ざ' => "za", 'じ' => "ji", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        'た' => "ta", 'ち' => "chi", 'つ' => "tsu", 'て' => "te", 'と' => "to",
        'だ' => "da", 'ぢ' => "ji", 'づ' => "zu", 'で' => "de", 'ど' => "do",
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "hi", 'ふ' => "fu", 'へ' => "he", 'ほ' => "ho",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        'や' => "ya", 'ゆ' => "yu", 'よ' => "yo",
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        'わ' => "wa", 'ゐ' => "wi", 'ゑ' => "we", 'を' => "o", 'ん' => "n",
        'ぁ' => "a", 'ぃ' => "i", 'ぅ' => "u", 'ぇ' => "e", 'ぉ' => "o",
        'ゃ' => "ya", 'ゅ' => "yu", 'ょ' => "yo", 'ゎ' => "wa", 'ゔ' => "vu",
        'ー' => "-",
        _ => return None,
    })
}

fn is_small_y(c: char) -> Option<&'static str> {
    Some(match c {
        'ゃ' | 'ャ' => "a",
        'ゅ' | 'ュ' => "u",
        'ょ' | 'ョ' => "o",
        _ => return None,
    })
}

fn romanize_kana(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // sokuon: double the next consonant
        if c == 'っ' || c == 'ッ' {
            if let Some(n) = chars.get(i + 1).and_then(|&n| kana(n)) {
                if let Some(f) = n.chars().next() {
                    out.push(if n.starts_with("ch") { 't' } else { f });
                }
            }
            i += 1;
            continue;
        }
        if c == 'ー' {
            if let Some(last) = out.chars().last() {
                out.push(last);
            }
            i += 1;
            continue;
        }
        if let Some(r) = kana(c) {
            // yōon: ki + ゃ → kya, shi + ゃ → sha
            if let Some(v) = chars.get(i + 1).and_then(|&n| is_small_y(n)) {
                if r.ends_with('i') && r.len() > 1 {
                    let stem = &r[..r.len() - 1];
                    if stem == "sh" || stem == "ch" || stem == "j" {
                        out.push_str(stem);
                        out.push_str(v);
                    } else {
                        out.push_str(stem);
                        out.push('y');
                        out.push_str(v);
                    }
                    i += 2;
                    continue;
                }
            }
            out.push_str(r);
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

const HANGUL_L: [&str; 19] = ["g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p", "h"];
const HANGUL_V: [&str; 21] = ["a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we", "wi", "yu", "eu", "ui", "i"];
const HANGUL_T: [&str; 28] = ["", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "p", "l", "l", "p", "l", "m", "p", "p", "t", "t", "ng", "t", "t", "k", "t", "p", "t"];

fn romanize_hangul(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let u = c as u32;
        if (0xAC00..=0xD7A3).contains(&u) {
            let idx = u - 0xAC00;
            let l = (idx / 588) as usize;
            let v = ((idx % 588) / 28) as usize;
            let t = (idx % 28) as usize;
            out.push_str(HANGUL_L[l]);
            out.push_str(HANGUL_V[v]);
            out.push_str(HANGUL_T[t]);
        } else {
            out.push(c);
        }
    }
    out
}

fn translit(c: char) -> Option<&'static str> {
    Some(match c {
        // Cyrillic (Russian/Ukrainian, simplified BGN)
        'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' => "e", 'ё' => "yo", 'ж' => "zh",
        'з' => "z", 'и' => "i", 'й' => "y", 'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o",
        'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u", 'ф' => "f", 'х' => "kh", 'ц' => "ts",
        'ч' => "ch", 'ш' => "sh", 'щ' => "shch", 'ъ' => "", 'ы' => "y", 'ь' => "", 'э' => "e", 'ю' => "yu",
        'я' => "ya", 'і' => "i", 'ї' => "yi", 'є' => "ye", 'ґ' => "g",
        // Greek
        'α' => "a", 'β' => "v", 'γ' => "g", 'δ' => "d", 'ε' => "e", 'ζ' => "z", 'η' => "i", 'θ' => "th",
        'ι' => "i", 'κ' => "k", 'λ' => "l", 'μ' => "m", 'ν' => "n", 'ξ' => "x", 'ο' => "o", 'π' => "p",
        'ρ' => "r", 'σ' | 'ς' => "s", 'τ' => "t", 'υ' => "y", 'φ' => "f", 'χ' => "ch", 'ψ' => "ps", 'ω' => "o",
        'ά' => "a", 'έ' => "e", 'ή' => "i", 'ί' => "i", 'ό' => "o", 'ύ' => "y", 'ώ' => "o",
        _ => return None,
    })
}

fn romanize_alpha(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let lower: Vec<char> = c.to_lowercase().collect();
        let lc = lower.first().copied().unwrap_or(c);
        match translit(lc) {
            Some(r) => {
                if c != lc && !r.is_empty() {
                    let mut it = r.chars();
                    if let Some(f) = it.next() {
                        out.extend(f.to_uppercase());
                        out.push_str(it.as_str());
                    }
                } else {
                    out.push_str(r);
                }
            }
            None => out.push(c),
        }
    }
    out
}

/// Romanize a line if it contains a supported non-Latin script.
pub fn romanize(text: &str) -> Option<String> {
    let has = |f: fn(char) -> bool| text.chars().any(f);
    let kana_p = |c: char| ('\u{3040}'..='\u{30FF}').contains(&c);
    let hangul_p = |c: char| ('\u{AC00}'..='\u{D7A3}').contains(&c);
    let cyr_greek_p = |c: char| ('\u{0370}'..='\u{04FF}').contains(&c);
    if !has(kana_p) && !has(hangul_p) && !has(cyr_greek_p) {
        return None;
    }
    let mut s = text.to_string();
    if has(kana_p) {
        s = romanize_kana(&s);
    }
    if has(hangul_p) {
        s = romanize_hangul(&s);
    }
    if has(cyr_greek_p) {
        s = romanize_alpha(&s);
    }
    (s != text).then_some(s)
}

/// Translate every lyric line in place (adds `translation`). Online; only
/// called when the user turned translation on.
pub async fn translate_lines(v: &mut Value, to: &str) -> Result<()> {
    let to: String = to.chars().filter(|c| c.is_ascii_alphabetic() || *c == '-').take(5).collect();
    let Some(lines) = v["lines"].as_array_mut() else { return Ok(()) };
    let texts: Vec<String> = lines.iter().map(|l| l["text"].as_str().unwrap_or_default().to_string()).collect();
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build()?;
    // Batch into ~450-char chunks joined by newlines (service limit is 500).
    let mut chunks: Vec<Vec<usize>> = vec![vec![]];
    let mut len = 0;
    for (i, t) in texts.iter().enumerate() {
        if len + t.len() > 450 && !chunks.last().unwrap().is_empty() {
            chunks.push(vec![]);
            len = 0;
        }
        chunks.last_mut().unwrap().push(i);
        len += t.len() + 1;
    }
    for chunk in chunks {
        let q = chunk.iter().map(|&i| texts[i].as_str()).collect::<Vec<_>>().join("\n");
        if q.trim().is_empty() {
            continue;
        }
        let resp: Value = client
            .get("https://api.mymemory.translated.net/get")
            .query(&[("q", q.as_str()), ("langpair", &format!("Autodetect|{to}"))])
            .send()
            .await?
            .json()
            .await?;
        let out = resp["responseData"]["translatedText"].as_str().unwrap_or_default();
        for (&i, t) in chunk.iter().zip(out.split('\n')) {
            if !t.trim().is_empty() && t.trim() != texts[i].trim() {
                lines[i]["translation"] = Value::String(t.trim().to_string());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kana() {
        assert_eq!(romanize("こんにちは").unwrap(), "konnichiha");
        assert_eq!(romanize("きょう").unwrap(), "kyou");
        assert_eq!(romanize("ちょっと").unwrap(), "chotto");
        assert_eq!(romanize("カラオケ").unwrap(), "karaoke");
        assert_eq!(romanize("ラーメン").unwrap(), "raamen");
    }

    #[test]
    fn hangul_cyrillic_greek() {
        assert_eq!(romanize("사랑해").unwrap(), "saranghae");
        assert_eq!(romanize("Привет").unwrap(), "Privet");
        assert_eq!(romanize("Καλημέρα").unwrap(), "Kalimera");
        assert!(romanize("hello").is_none());
    }
}
