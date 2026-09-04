//! The grammars this build bundles, one cargo feature each.

#[allow(unused_variables)]
pub(crate) fn register_bundled(registry: &crate::syntax::registry::LanguageRegistry) {
    #[cfg(feature = "lang-rust")]
    registry.register(crate::syntax::registry::LanguageConfig {
        name: "rust".to_string(),
        language: tree_sitter_rust::LANGUAGE.into(),
        highlight_query: tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
        injections_query: Some(tree_sitter_rust::INJECTIONS_QUERY.to_string()),
        extensions: vec!["rs".to_string()],
    });

    #[cfg(feature = "lang-toml")]
    registry.register(crate::syntax::registry::LanguageConfig {
        name: "toml".to_string(),
        language: tree_sitter_toml_ng::LANGUAGE.into(),
        highlight_query: tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_string(),
        injections_query: None,
        extensions: vec!["toml".to_string()],
    });

    #[cfg(feature = "lang-markdown")]
    registry.register(crate::syntax::registry::LanguageConfig {
        name: "markdown".to_string(),
        language: tree_sitter_md::LANGUAGE.into(),
        highlight_query: tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_string(),
        injections_query: Some(tree_sitter_md::INJECTION_QUERY_BLOCK.to_string()),
        extensions: vec!["md".to_string(), "markdown".to_string()],
    });

    #[cfg(feature = "lang-python")]
    registry.register(crate::syntax::registry::LanguageConfig {
        name: "python".to_string(),
        language: tree_sitter_python::LANGUAGE.into(),
        highlight_query: tree_sitter_python::HIGHLIGHTS_QUERY.to_string(),
        injections_query: None,
        extensions: vec!["py".to_string(), "pyi".to_string()],
    });
}
