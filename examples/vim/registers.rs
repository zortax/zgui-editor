//! Where yanked and deleted text goes.

/// The unnamed register, with room for the named ones later.
#[derive(Default)]
pub struct Registers {
    unnamed: Option<(String, bool)>,
}

impl Registers {
    /// Stores `text`; `linewise` says whether a paste of it opens its own lines.
    pub fn store(&mut self, text: String, linewise: bool) {
        self.unnamed = Some((text, linewise));
    }

    /// What a paste takes out.
    pub fn take(&self) -> Option<(String, bool)> {
        self.unnamed.clone()
    }
}
