//! Highlighting a text that is in no editor.
//!
//! A diff line, a preview, a fenced block a caller renders itself: each is a whole text that
//! exists once and never edits. One synchronous call parses it, queries it, and answers every
//! line's spans. The caller runs it off the interface thread and keeps the answer for as long
//! as the text stands.

use ropey::Rope;
use smallvec::SmallVec;

use crate::syntax::LineSpan;
use crate::syntax::registry::LanguageConfig;
use crate::syntax::worker::collect_spans;

/// Every line of one text, highlighted.
#[derive(Debug, Clone, Default)]
pub struct Highlighted {
    /// The capture names of the language, by the index each span carries.
    pub captures: Vec<String>,
    /// One entry per line: spans in line-local byte offsets, sorted by start and then end.
    pub lines: Vec<SmallVec<[LineSpan; 8]>>,
}

/// Parses `text` as `config`'s language and answers every line's spans.
///
/// `None` when the grammar or its query does not load, which leaves the text plain.
#[must_use]
pub fn highlight(config: &LanguageConfig, text: &str) -> Option<Highlighted> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&config.language).ok()?;
    let query = tree_sitter::Query::new(&config.language, &config.highlight_query).ok()?;

    let rope = Rope::from_str(text);
    let tree = parser.parse_with_options(
        &mut |byte, _| {
            if byte >= rope.len_bytes() {
                return &[] as &[u8];
            }
            let (chunk, start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - start..]
        },
        None,
        None,
    )?;

    let captures = query
        .capture_names()
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let lines = collect_spans(&tree, &query, &rope, 0..rope.len_lines());
    Some(Highlighted { captures, lines })
}

#[cfg(test)]
#[cfg(feature = "lang-rust")]
mod tests {
    use super::*;

    fn rust_config() -> LanguageConfig {
        LanguageConfig {
            name: "rust".to_string(),
            language: tree_sitter_rust::LANGUAGE.into(),
            highlight_query: tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
            injections_query: None,
            extensions: vec!["rs".to_string()],
        }
    }

    #[test]
    fn a_whole_text_is_highlighted_line_by_line() {
        let text = "fn main() {\n    let x = \"hello\";\n}\n";
        let answer = highlight(&rust_config(), text).expect("rust highlights");
        assert_eq!(answer.lines.len(), Rope::from_str(text).len_lines());
        assert!(!answer.captures.is_empty());

        // `fn` on line zero is a keyword-ish capture starting at byte zero.
        let first = &answer.lines[0];
        assert!(first.iter().any(|(start, end, _)| *start == 0 && *end >= 2));
        // The string literal sits on line one.
        let second = &answer.lines[1];
        let string_capture = second
            .iter()
            .any(|(_, _, capture)| answer.captures[*capture as usize].starts_with("string"));
        assert!(string_capture, "the literal is captured as a string");
    }

    #[test]
    fn an_empty_text_answers_empty_lines() {
        let answer = highlight(&rust_config(), "").expect("rust highlights");
        assert_eq!(answer.lines.len(), 1);
        assert!(answer.lines[0].is_empty());
    }
}
