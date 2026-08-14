//! The thread that parses, and what crosses to and from it.
//!
//! One worker per editor owns the parser, the tree and the compiled query. Edits arrive as
//! `tree.edit` deltas with an O(1) rope snapshot, so the parse is incremental and the UI thread
//! never blocks on it. Highlighting is windowed: only the lines near the viewport are queried,
//! which is what keeps a ten-million-line file's keystroke costing a keystroke.
//!
//! The message loop drains its inbox before working, folding a burst of edits into one parse,
//! and rehighlights its window after every state change — so the answer converges on the newest
//! text however the messages raced.

use std::sync::Arc;

use ropey::Rope;
use smallvec::SmallVec;
use streaming_iterator::StreamingIterator;

use crate::syntax::LineSpan;
use crate::syntax::registry::LanguageConfig;
use crate::syntax::spans::HighlightFrame;

/// How many lines beyond each edge of the viewport are highlighted along with it.
pub const OVERSCAN_LINES: usize = 200;

/// What the UI sends the worker.
pub enum ToWorker {
    /// Highlight as this language from now on, over this text.
    SetLanguage(Option<Arc<LanguageConfig>>, Rope, u64),
    /// The text changed.
    Edited {
        /// The edits, in the order they were applied.
        input_edits: Vec<tree_sitter::InputEdit>,
        /// The whole new text, as an O(1) snapshot.
        snapshot: Rope,
        /// Which revision the snapshot is.
        revision: u64,
    },
    /// The viewport moved; highlight these lines from now on.
    Window(std::ops::Range<usize>),
}

/// What the worker sends back.
pub enum FromWorker {
    /// The capture vocabulary of a newly loaded language, once per `SetLanguage`.
    Captures(Vec<String>),
    /// A batch of highlighted lines.
    Frame(HighlightFrame),
}

/// Starts the worker; the returned sender is the whole interface. The thread exits when the
/// sender is dropped.
pub fn spawn(frames: flume::Sender<FromWorker>) -> flume::Sender<ToWorker> {
    let (sender, inbox) = flume::unbounded::<ToWorker>();
    std::thread::Builder::new()
        .name("zgui-editor-syntax".to_string())
        .spawn(move || Worker::new(frames).run(inbox))
        .expect("a thread can be spawned");
    sender
}

/// The worker's whole state.
struct Worker {
    frames: flume::Sender<FromWorker>,
    parser: tree_sitter::Parser,
    language: Option<Arc<LanguageConfig>>,
    query: Option<tree_sitter::Query>,
    tree: Option<tree_sitter::Tree>,
    rope: Rope,
    revision: u64,
    window: std::ops::Range<usize>,
}

impl Worker {
    fn new(frames: flume::Sender<FromWorker>) -> Self {
        Self {
            frames,
            parser: tree_sitter::Parser::new(),
            language: None,
            query: None,
            tree: None,
            rope: Rope::new(),
            revision: 0,
            window: 0..0,
        }
    }

    fn run(mut self, inbox: flume::Receiver<ToWorker>) {
        while let Ok(first) = inbox.recv() {
            let mut reparse = self.apply(first);
            // Everything already waiting joins this round, so a burst of keystrokes is one
            // parse rather than one per key.
            while let Ok(message) = inbox.try_recv() {
                reparse |= self.apply(message);
            }
            if reparse {
                self.parse();
            }
            if self.publish().is_err() {
                return;
            }
        }
    }

    /// Takes one message in. Answers whether the tree has to be parsed again.
    fn apply(&mut self, message: ToWorker) -> bool {
        match message {
            ToWorker::SetLanguage(language, rope, revision) => {
                self.language = language;
                self.rope = rope;
                self.revision = revision;
                self.tree = None;
                self.query = None;
                if let Some(config) = self.language.as_ref() {
                    if self.parser.set_language(&config.language).is_ok() {
                        match tree_sitter::Query::new(&config.language, &config.highlight_query) {
                            Ok(query) => {
                                let names: Vec<String> = query
                                    .capture_names()
                                    .iter()
                                    .map(|name| name.to_string())
                                    .collect();
                                let _ = self.frames.send(FromWorker::Captures(names));
                                self.query = Some(query);
                            }
                            Err(_) => self.language = None,
                        }
                    } else {
                        self.language = None;
                    }
                }
                self.language.is_some()
            }
            ToWorker::Edited {
                input_edits,
                snapshot,
                revision,
            } => {
                if let Some(tree) = self.tree.as_mut() {
                    for edit in &input_edits {
                        tree.edit(edit);
                    }
                }
                self.rope = snapshot;
                self.revision = revision;
                self.language.is_some()
            }
            ToWorker::Window(window) => {
                self.window = window;
                false
            }
        }
    }

    /// Parses the current text, reusing the edited tree.
    fn parse(&mut self) {
        if self.language.is_none() {
            return;
        }
        let rope = self.rope.clone();
        let tree = self.parser.parse_with_options(
            &mut |byte, _| {
                if byte >= rope.len_bytes() {
                    return &[] as &[u8];
                }
                let (chunk, start, _, _) = rope.chunk_at_byte(byte);
                &chunk.as_bytes()[byte - start..]
            },
            self.tree.as_ref(),
            None,
        );
        if let Some(tree) = tree {
            self.tree = Some(tree);
        }
    }

    /// Highlights the window over the current tree and sends the frame.
    fn publish(&mut self) -> Result<(), ()> {
        let (Some(tree), Some(query)) = (self.tree.as_ref(), self.query.as_ref()) else {
            return Ok(());
        };
        let total = self.rope.len_lines();
        let start_line = self.window.start.saturating_sub(OVERSCAN_LINES);
        let end_line = (self.window.end + OVERSCAN_LINES).min(total);
        if start_line >= end_line {
            return Ok(());
        }
        let start_byte = self.rope.line_to_byte(start_line);
        let end_byte = if end_line >= total {
            self.rope.len_bytes()
        } else {
            self.rope.line_to_byte(end_line)
        };

        let mut spans: Vec<SmallVec<[LineSpan; 8]>> = vec![SmallVec::new(); end_line - start_line];
        let mut cursor = tree_sitter::QueryCursor::new();
        cursor.set_byte_range(start_byte..end_byte);
        let mut matches =
            cursor.matches(query, tree.root_node(), RopeProvider { rope: &self.rope });
        while let Some(matched) = matches.next() {
            for capture in matched.captures {
                let node = capture.node;
                let from = node.start_position();
                let to = node.end_position();
                for row in from.row..=to.row {
                    if row < start_line || row >= end_line {
                        continue;
                    }
                    let col_start = if row == from.row {
                        from.column as u32
                    } else {
                        0
                    };
                    let col_end = if row == to.row {
                        to.column as u32
                    } else {
                        u32::MAX
                    };
                    if col_end > col_start {
                        spans[row - start_line].push((col_start, col_end, capture.index as u16));
                    }
                }
            }
        }
        for line in spans.iter_mut() {
            line.sort_by_key(|(start, end, _)| (*start, *end));
        }

        self.frames
            .send(FromWorker::Frame(HighlightFrame {
                revision: self.revision,
                lines: start_line..end_line,
                spans,
            }))
            .map_err(|_| ())
    }
}

/// Hands tree-sitter the text of a node, straight out of the rope's chunks.
struct RopeProvider<'a> {
    rope: &'a Rope,
}

/// The chunks of one node's text, as byte slices.
struct RopeChunks<'a> {
    chunks: ropey::iter::Chunks<'a>,
}

impl<'a> Iterator for RopeChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        self.chunks.next().map(str::as_bytes)
    }
}

impl<'a> tree_sitter::TextProvider<&'a [u8]> for RopeProvider<'a> {
    type I = RopeChunks<'a>;

    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        let end = node.end_byte().min(self.rope.len_bytes());
        let start = node.start_byte().min(end);
        RopeChunks {
            chunks: self.rope.byte_slice(start..end).chunks(),
        }
    }
}

#[cfg(test)]
#[cfg(feature = "lang-rust")]
mod tests {
    use super::*;

    fn rust_config() -> Arc<LanguageConfig> {
        Arc::new(LanguageConfig {
            name: "rust".to_string(),
            language: tree_sitter_rust::LANGUAGE.into(),
            highlight_query: tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
            injections_query: None,
            extensions: vec!["rs".to_string()],
        })
    }

    fn drain_frames(receiver: &flume::Receiver<FromWorker>) -> Vec<HighlightFrame> {
        let mut frames = Vec::new();
        while let Ok(message) = receiver.recv_timeout(std::time::Duration::from_secs(5)) {
            if let FromWorker::Frame(frame) = message {
                frames.push(frame);
                if !frames.is_empty() && receiver.is_empty() {
                    break;
                }
            }
        }
        frames
    }

    #[test]
    fn the_worker_highlights_and_follows_edits() {
        let (frames_tx, frames_rx) = flume::unbounded();
        let to_worker = spawn(frames_tx);
        let text = "fn main() {\n    let x = \"hello\";\n}\n";
        to_worker
            .send(ToWorker::SetLanguage(
                Some(rust_config()),
                Rope::from_str(text),
                0,
            ))
            .unwrap();
        to_worker.send(ToWorker::Window(0..4)).unwrap();
        let frames = drain_frames(&frames_rx);
        let frame = frames.last().expect("a frame arrived");
        assert_eq!(frame.revision, 0);
        assert!(
            frame.spans.iter().any(|line| !line.is_empty()),
            "something was highlighted"
        );

        // An edit: insert at the start of line 1, keeping the text parseable.
        let mut rope = Rope::from_str(text);
        rope.insert(rope.byte_to_char(12), "    let y = 1;\n");
        let edit = tree_sitter::InputEdit {
            start_byte: 12,
            old_end_byte: 12,
            new_end_byte: 27,
            start_position: tree_sitter::Point { row: 1, column: 0 },
            old_end_position: tree_sitter::Point { row: 1, column: 0 },
            new_end_position: tree_sitter::Point { row: 2, column: 0 },
        };
        to_worker
            .send(ToWorker::Edited {
                input_edits: vec![edit],
                snapshot: rope,
                revision: 1,
            })
            .unwrap();
        let frames = drain_frames(&frames_rx);
        let frame = frames.last().expect("a frame after the edit");
        assert_eq!(frame.revision, 1, "the frame is stamped with the new text");
    }
}
