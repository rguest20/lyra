//! Secret-safe configuration for the local SearXNG search service.

use std::io;
use std::path::Path;

use serde::Deserialize;

#[derive(Clone, Debug, Default)]
pub(super) struct ProviderConfig {
    pub searxng_url: Option<String>,
    pub config_error: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ConfigFile {
    search: SearchSection,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SearchSection {
    searxng: SearxngSection,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct SearxngSection {
    url: Option<String>,
}

impl ProviderConfig {
    pub fn load() -> io::Result<Self> {
        let path = std::env::var_os("COMMUNITY_CONFIG").map_or_else(
            || std::path::PathBuf::from("config.toml"),
            std::path::PathBuf::from,
        );
        let (file, config_error) = match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<ConfigFile>(&contents) {
                Ok(file) => (file, None),
                Err(_) => (
                    ConfigFile::default(),
                    Some(invalid_config(&path).to_string()),
                ),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (ConfigFile::default(), None),
            Err(_) => (
                ConfigFile::default(),
                Some(invalid_config(&path).to_string()),
            ),
        };
        Ok(Self {
            searxng_url: usable(std::env::var("SEARXNG_URL").ok())
                .or_else(|| usable(file.search.searxng.url)),
            config_error,
        })
    }
}

fn usable(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn invalid_config(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "search configuration at {} is unreadable or invalid",
            path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_searxng_configuration_without_exposing_values_in_errors() {
        let parsed: ConfigFile = toml::from_str(
            r#"[search.searxng]
url = "http://searxng.local:8080/search"
"#,
        )
        .unwrap();
        assert_eq!(
            parsed.search.searxng.url.as_deref(),
            Some("http://searxng.local:8080/search")
        );
        let missing_url: ConfigFile = toml::from_str("[search.searxng]\n").unwrap();
        assert!(missing_url.search.searxng.url.is_none());
        let error = toml::from_str::<ConfigFile>("[search.searxng\n").unwrap_err();
        assert!(!error.to_string().contains("api_key"));
        let safe_error = invalid_config(Path::new("config.toml"));
        assert!(safe_error.to_string().contains("unreadable or invalid"));
        assert!(!safe_error.to_string().contains("api_key"));
    }
}
