//! Which languages the editor knows, and what each one is.
//!
//! The registry is the pluggable seam: an application registers any tree-sitter grammar with a
//! highlight query, and the bundled features register the common ones. It is `Send`, because
//! the worker thread resolves languages from it too.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// One language: its grammar, and the query that colours it.
pub struct LanguageConfig {
    /// The name the language is registered under, like `rust`.
    pub name: String,
    /// The compiled grammar.
    pub language: tree_sitter::Language,
    /// The highlight query source, with the conventional capture names.
    pub highlight_query: String,
    /// The injections query source, held for the day layers are evaluated.
    pub injections_query: Option<String>,
    /// The file extensions the language claims.
    pub extensions: Vec<String>,
}

/// The languages an editor can highlight.
#[derive(Clone, Default)]
pub struct LanguageRegistry {
    inner: Arc<RwLock<HashMap<String, Arc<LanguageConfig>>>>,
}

impl LanguageRegistry {
    /// A registry with nothing in it.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry holding every language this build bundles.
    pub fn with_bundled(self) -> Self {
        crate::syntax::languages::register_bundled(&self);
        self
    }

    /// Registers `config`, replacing any language of the same name.
    pub fn register(&self, config: LanguageConfig) {
        let name = config.name.clone();
        self.inner
            .write()
            .expect("the registry lock is never poisoned")
            .insert(name, Arc::new(config));
    }

    /// The language registered as `name`.
    pub fn by_name(&self, name: &str) -> Option<Arc<LanguageConfig>> {
        self.inner
            .read()
            .expect("the registry lock is never poisoned")
            .get(name)
            .cloned()
    }

    /// The language claiming the file extension `extension`.
    pub fn by_extension(&self, extension: &str) -> Option<Arc<LanguageConfig>> {
        self.inner
            .read()
            .expect("the registry lock is never poisoned")
            .values()
            .find(|config| config.extensions.iter().any(|held| held == extension))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "lang-yaml")]
    fn yaml_claims_both_of_its_extensions() {
        let registry = LanguageRegistry::new().with_bundled();
        for extension in ["yaml", "yml"] {
            let config = registry
                .by_extension(extension)
                .expect("a bundled language claims it");
            assert_eq!(config.name, "yaml");
        }
    }

    #[test]
    #[cfg(feature = "lang-json")]
    fn json_claims_its_extension() {
        let registry = LanguageRegistry::new().with_bundled();
        let config = registry.by_extension("json").expect("json is bundled");
        assert_eq!(config.name, "json");
    }
}
