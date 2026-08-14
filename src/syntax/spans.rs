//! What the worker sends back: spans per line, stamped with the text they describe.

use smallvec::SmallVec;

use crate::syntax::LineSpan;

/// One delivery of highlight results.
#[derive(Debug)]
pub struct HighlightFrame {
    /// The buffer revision the spans describe. A frame whose revision is not the buffer's
    /// current one is dropped whole.
    pub revision: u64,
    /// The lines the frame covers.
    pub lines: std::ops::Range<usize>,
    /// One entry per line of `lines`: spans in line-local byte offsets, sorted by start.
    pub spans: Vec<SmallVec<[LineSpan; 8]>>,
}
