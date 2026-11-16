//! 判断カードのペイロード(ANDON_JSON)の定義とパース。
//! I/O・保存・ポリシーは持たない。

use serde::Deserialize;

use crate::types::{CardDiagram, CardOption};

/// メッセージ末尾のカード指令マーカー。
pub const MARKER: &str = "ANDON_JSON";

/// `ANDON_JSON{...}` ペイロード。全フィールド省略可だが空オブジェクトは無効。
#[derive(Debug, Clone, Deserialize)]
pub struct CardPayload {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub diagram: Option<CardDiagram>,
    #[serde(default)]
    pub options: Vec<CardOption>,
}

/// 汎用選択肢(指令がないメッセージのフォールバック)。
pub fn fallback_options() -> Vec<CardOption> {
    ["承認", "却下", "方針を変えて"]
        .iter()
        .enumerate()
        .map(|(i, label)| CardOption {
            id: format!("f{i}"),
            label: (*label).to_owned(),
        })
        .collect()
}

/// メッセージ末尾の `ANDON_JSON{...}` を探してパースする。
/// 見つからない・壊れている場合は None(フォールバックに任せる)。
pub fn parse(message: &str) -> Option<CardPayload> {
    let at = message.rfind(MARKER)? + MARKER.len();
    let rest = strip_fence(&message[at..]);
    let json = first_json_object(rest)?;
    serde_json::from_str(json).ok()
}

/// 回答を Devin へのメッセージ文に組み立てる。
pub fn answer_message(option: Option<&CardOption>, text: Option<&str>) -> String {
    match (option, text) {
        (_, Some(t)) => t.to_owned(),
        (Some(o), None) => format!("{}: {}", o.id, o.label),
        (None, None) => "承認".to_owned(),
    }
}

fn strip_fence(s: &str) -> &str {
    let s = s.trim_start();
    let s = s.strip_prefix("```").unwrap_or(s);
    s.strip_prefix("json").unwrap_or(s).trim_start()
}

/// 先頭から最初の `{...}` をブレース対応で切り出す。文字列・エスケープを考慮。
fn first_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trailing_payload() {
        let msg = "どちらで進めますか?\n\nANDON_JSON{\"title\":\"方針\",\"options\":[{\"id\":\"a\",\"label\":\"案A\"}]}";
        let card = parse(msg).unwrap();
        assert_eq!(card.title.as_deref(), Some("方針"));
        assert_eq!(card.options[0].id, "a");
    }

    #[test]
    fn parses_fenced_payload() {
        let msg = "説明\nANDON_JSON ```json\n{\"options\":[{\"id\":\"x\",\"label\":\"L\"}]}\n```";
        let card = parse(msg).unwrap();
        assert_eq!(card.options[0].id, "x");
    }

    #[test]
    fn braces_inside_strings_are_ignored() {
        let msg = "ANDON_JSON{\"summary\":\"} と { を含む\",\"options\":[]}";
        let card = parse(msg).unwrap();
        assert_eq!(card.summary.as_deref(), Some("} と { を含む"));
    }

    #[test]
    fn returns_none_without_marker() {
        assert!(parse("普通のメッセージ {json: true}").is_none());
    }

    #[test]
    fn returns_none_on_broken_json() {
        assert!(parse("ANDON_JSON{broken").is_none());
    }

    #[test]
    fn answer_prefers_free_text() {
        let opt = CardOption {
            id: "a".into(),
            label: "案A".into(),
        };
        assert_eq!(answer_message(Some(&opt), Some("別案で")), "別案で");
        assert_eq!(answer_message(Some(&opt), None), "a: 案A");
    }
}
