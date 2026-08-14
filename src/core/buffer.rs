//! The text, and the one number that says which text it currently is.
//!
//! A [`Buffer`] is a rope and a revision. The rope is ropey's: edits anywhere in a very large
//! file are cheap, and cloning is O(1), which is what hands a consistent snapshot to a background
//! parser without copying the file. The revision moves once per applied transaction, and is what
//! every cache and every worker result is checked against — a result stamped with an old revision
//! describes text that no longer exists and is dropped.

use ropey::Rope;

use crate::core::edit::Edit;

/// The text being edited.
#[derive(Clone, Debug)]
pub struct Buffer {
    rope: Rope,
    revision: u64,
}

impl Buffer {
    /// A buffer holding `text`.
    ///
    /// Named after [`Rope::from_str`], deliberately; there is no parse and so no `FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            revision: 0,
        }
    }

    /// A buffer read from `reader`, without holding the file in one contiguous allocation.
    pub fn from_reader<R: std::io::Read>(reader: R) -> std::io::Result<Self> {
        Ok(Self {
            rope: Rope::from_reader(reader)?,
            revision: 0,
        })
    }

    /// The text. Cloning the returned rope is O(1) and yields a snapshot an edit cannot move.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Which text this is. Moves once per applied transaction.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// How many bytes the text holds.
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// The whole text, copied out. For saving and for tests, not for the frame.
    ///
    /// Deliberately not `Display`: formatting a multi-hundred-megabyte buffer into another
    /// buffer is a decision a caller should make by name.
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        self.rope.to_string()
    }

    /// Replaces the whole text, as loading a file does.
    pub(crate) fn set_text(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.revision += 1;
    }

    /// Applies `edits`, which are sorted by start descending and do not overlap, as one revision.
    ///
    /// Descending order is what lets each edit's offsets be used as they are: an edit changes
    /// nothing before itself.
    pub(crate) fn apply(&mut self, edits: &[Edit]) {
        debug_assert!(
            edits
                .windows(2)
                .all(|pair| pair[1].range.end <= pair[0].range.start),
            "edits are sorted descending and disjoint"
        );
        for edit in edits {
            // The one place byte addresses become ropey's char indices.
            let start = self.rope.byte_to_char(edit.range.start);
            let end = self.rope.byte_to_char(edit.range.end);
            if end > start {
                self.rope.remove(start..end);
            }
            if !edit.inserted.is_empty() {
                self.rope.insert(start, &edit.inserted);
            }
        }
        self.revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_apply_in_descending_order() {
        let mut buffer = Buffer::from_str("one two three");
        let edits = vec![
            Edit {
                range: 8..13,
                inserted: "3".to_string(),
                deleted: "three".to_string(),
            },
            Edit {
                range: 0..3,
                inserted: "1".to_string(),
                deleted: "one".to_string(),
            },
        ];
        buffer.apply(&edits);
        assert_eq!(buffer.to_string(), "1 two 3");
        assert_eq!(buffer.revision(), 1);
    }

    #[test]
    fn an_insertion_deletes_nothing() {
        let mut buffer = Buffer::from_str("ab");
        buffer.apply(&[Edit {
            range: 1..1,
            inserted: "x".to_string(),
            deleted: String::new(),
        }]);
        assert_eq!(buffer.to_string(), "axb");
    }
}
