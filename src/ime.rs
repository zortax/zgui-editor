//! Where input-method support will live, and why it does not yet.
//!
//! The platform starts an IME composition only over elements it is told are editable, and the
//! runtime's editable set is currently the intrinsic `editor` and `field` elements
//! (`zgui-runtime/src/editing.rs`) — a `custom` element is never reported, so a composition
//! never starts over this editor. Committed IME text (`ime_commit`) is still delivered and
//! could be inserted, but without preedit display and caret anchoring that is half a feature,
//! and half of this one confuses more than it helps.
//!
//! The intended fix is upstream: let an element opt into the editable set, and report the
//! caret's rectangle for the candidate window. This module is the seam that composition state
//! will live behind when it lands; nothing else in the crate needs to change shape for it.
//!
//! Until then: keyboard layouts that compose through dead keys work (the platform resolves
//! them to `Key::Character` before the editor sees them); full input methods — CJK — do not.
