# zgui-editor

An embeddable, high-performance code editor component for [Zgui](https://github.com/zortax/zgui)
applications — from a small syntax-highlighted field inside another app up to the foundation of
a full editor or IDE.

```rust
use zgui::prelude::*;
use zgui_editor::{Editor, EditorProps};

view! {
    Editor(class = "my-editor", text = source, language = "rust")
}
```

## What it does

- **Very large files.** One retained element paints only the visible lines; the scroll model is
  the editor's own `f64` fractional line, so a ten-million-line buffer scrolls to pixel
  precision and a keystroke costs a keystroke. The buffer is a rope
  ([ropey](https://crates.io/crates/ropey)); snapshots for background work are O(1).
- **Tree-sitter highlighting, incrementally.** A worker thread owns the parser and the tree;
  edits cross as `tree.edit` deltas, queries run only over the viewport (± overscan), and stale
  results are dropped by revision. Grammars are pluggable through `LanguageRegistry`; common
  ones are bundled behind cargo features (`lang-rust`, `lang-toml`, `lang-markdown`).
- **App-definable key handling.** An `on_key` filter hears every key before the editor does.
  The bundled vim example (`examples/vim`) implements modal editing — counts, operators,
  registers, `/` search — entirely outside the component, over the public `Command` vocabulary
  and the synchronous `EditorHandle::query` API.
- **Editing model.** Multi-cursor-capable selections, grapheme-aware movement, vim-style undo
  coalescing, word motions, plain-text search.
- **Mouse.** Click/drag selection with pointer capture and edge autoscroll, double-click word
  and triple-click line selection, middle-click primary-selection paste on Linux, scrollbar
  dragging.
- **Smooth scrolling.** Wheel input glides (critically-damped), trackpads track exactly,
  `scrolloff`-style caret margins are configurable.
- **Line numbers.** Absolute, relative (vim-style), or none.
- **Completion popovers.** The caret's window rectangle is a signal (`handle.caret_rect()`),
  edits and selection moves are events, and the key filter lets an open popover steal its keys
  — see `examples/completion.rs`.

## Theming is CSS

The element's `color` is the text and its `background` is the background; everything else is a
custom property, inheriting through the cascade like any other:

```css
.my-editor {
    font-family: 'JetBrains Mono', monospace;
    font-size: 14px;
    tab-size: 4;
    --editor-selection: rgba(137, 180, 250, 0.3);
    --editor-cursor: #f5e0dc;
    --editor-current-line: rgba(88, 91, 112, 0.35);
    --editor-gutter-fg: #6c7086;
    --syntax-keyword: #cba6f7;
    --syntax-function: #89b4fa;
    --syntax-string: #a6e3a1;
    --syntax-comment: #6c7086;
}
```

Syntax colours map from tree-sitter capture names with dots as hyphens
(`function.method` → `--syntax-function-method`), falling back along the dots and then to the
foreground.

## Driving it

Everything is a `Command`, applied through the `EditorHandle` delivered by `on_ready` (and via
local context). The handle also answers synchronous reads — `query(|snapshot| ...)` exposes the
rope, the selections, motion ranges, and search — and publishes signals (`cursor_position()`,
`scroll_state()`, `caret_rect()`, `revision()`) for status lines and custom chrome.

## Examples

```sh
cargo run --example basic --features lang-rust [-- path/to/file]
cargo run --example vim --features lang-rust [-- path/to/file]
cargo run --example completion --features lang-rust
```

## Known limitations

- **IME**: input methods do not compose over the editor yet. The platform only starts
  compositions over the runtime's intrinsic editable elements; see `src/ime.rs` for the
  limitation and the upstream path. Dead-key composition works.
- **No soft wrap**: long lines scroll horizontally. The line cache and scroll model keep the
  seam open for it.
- Injections (markdown embedding rust) are parsed single-layer for now.

## Development

The framework is pre-1.0; `.cargo/config.toml` patches the git dependencies to a local
checkout. Tests run headlessly:

```sh
cargo test --all-features            # unit + component + pipeline tests
cargo test --example vim --features lang-rust   # the vim layer, end to end
```
