//! Package metadata extracted from `build.zig.zon`, so `[package]` in
//! `pixi.toml` can omit `name`/`version` — mirroring how the Rust backend
//! reads Cargo.toml.
//!
//! ZON is zig syntax, not TOML: the manifest is one anonymous struct
//! literal. Only the two top-level fields we need are extracted, with a
//! small scanner instead of a full parser. Handled forms:
//!
//! ```zon
//! .{
//!     .name = .my_package,          // zig >= 0.14: enum literal
//!     .name = .@"my-package",       // quoted identifier
//!     .name = "my-package",         // zig <= 0.13: string
//!     .version = "1.2.3",
//!     .dependencies = .{ ... },     // nested fields are ignored
//! }
//! ```

use std::path::{Path, PathBuf};

use miette::Diagnostic;
use pixi_build_backend::generated_recipe::MetadataProvider;
use rattler_conda_types::Version;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum ZonError {
    #[error("failed to read {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid version {version:?} in {path}")]
    InvalidVersion {
        path: PathBuf,
        version: String,
        #[source]
        source: rattler_conda_types::ParseVersionError,
    },
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ZonMetadata {
    pub name: Option<String>,
    pub version: Option<String>,
}

/// Extract top-level `.name` and `.version` from ZON source.
///
/// The scanner strips `//` comments, tracks `{`/`}` depth so nested
/// structs (`.dependencies`, `.paths`) are skipped, and reads the value
/// after `=` as either a string literal, an enum literal, or a quoted
/// identifier.
pub fn parse_zon(source: &str) -> ZonMetadata {
    let mut meta = ZonMetadata::default();
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;

    let skip_ws_and_comments = |i: &mut usize| {
        loop {
            while *i < bytes.len() && bytes[*i].is_ascii_whitespace() {
                *i += 1;
            }
            if *i + 1 < bytes.len() && bytes[*i] == b'/' && bytes[*i + 1] == b'/' {
                while *i < bytes.len() && bytes[*i] != b'\n' {
                    *i += 1;
                }
            } else {
                break;
            }
        }
    };

    let read_string = |i: &mut usize| -> Option<String> {
        // *i points at the opening quote
        *i += 1;
        let start = *i;
        while *i < bytes.len() && bytes[*i] != b'"' {
            if bytes[*i] == b'\\' {
                *i += 1;
            }
            *i += 1;
        }
        let s = source.get(start..*i).map(str::to_string);
        *i += 1; // closing quote
        s
    };

    while i < bytes.len() {
        match bytes[i] {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                let _ = read_string(&mut i);
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'.' if depth == 1 => {
                // possible top-level field: .ident = value
                i += 1;
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let field = &source[start..i];
                if field != "name" && field != "version" {
                    continue;
                }
                skip_ws_and_comments(&mut i);
                if i >= bytes.len() || bytes[i] != b'=' {
                    continue;
                }
                i += 1;
                skip_ws_and_comments(&mut i);
                let value = if i < bytes.len() && bytes[i] == b'"' {
                    read_string(&mut i)
                } else if i < bytes.len() && bytes[i] == b'.' {
                    i += 1;
                    if i + 1 < bytes.len() && bytes[i] == b'@' && bytes[i + 1] == b'"' {
                        i += 1;
                        read_string(&mut i)
                    } else {
                        let start = i;
                        while i < bytes.len()
                            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                        {
                            i += 1;
                        }
                        source.get(start..i).map(str::to_string)
                    }
                } else {
                    None
                };
                match field {
                    "name" => meta.name = value,
                    "version" => meta.version = value,
                    _ => unreachable!(),
                }
            }
            _ => i += 1,
        }
    }
    meta
}

/// [`MetadataProvider`] backed by `build.zig.zon` next to the manifest.
/// A missing file simply provides nothing; the project model then has to
/// carry the name/version itself.
pub struct ZonMetadataProvider {
    zon_path: PathBuf,
    ignore: bool,
    cache: Option<ZonMetadata>,
}

impl ZonMetadataProvider {
    pub fn new(manifest_root: &Path, ignore: bool) -> Self {
        Self {
            zon_path: manifest_root.join("build.zig.zon"),
            ignore,
            cache: None,
        }
    }

    fn metadata(&mut self) -> Result<&ZonMetadata, ZonError> {
        if self.cache.is_none() {
            let meta = if self.ignore || !self.zon_path.exists() {
                ZonMetadata::default()
            } else {
                let source =
                    fs_err::read_to_string(&self.zon_path).map_err(|source| ZonError::Io {
                        path: self.zon_path.clone(),
                        source,
                    })?;
                parse_zon(&source)
            };
            self.cache = Some(meta);
        }
        Ok(self.cache.as_ref().expect("just filled"))
    }

    /// Globs contributing to the metadata-cache fingerprint when the zon
    /// manifest was consulted.
    pub fn input_globs(&self) -> Vec<String> {
        if self.ignore {
            Vec::new()
        } else {
            vec!["build.zig.zon".to_string()]
        }
    }
}

impl MetadataProvider for ZonMetadataProvider {
    type Error = ZonError;

    fn name(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.metadata()?.name.clone())
    }

    fn version(&mut self) -> Result<Option<Version>, Self::Error> {
        let path = self.zon_path.clone();
        match &self.metadata()?.version {
            None => Ok(None),
            Some(raw) => raw
                .parse()
                .map(Some)
                .map_err(|source| ZonError::InvalidVersion {
                    path,
                    version: raw.clone(),
                    source,
                }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_enum_literal_name() {
        let meta = parse_zon(
            r#".{
                .name = .zig_sdl, // trailing comment
                .version = "0.1.0",
                .minimum_zig_version = "0.14.0",
            }"#,
        );
        assert_eq!(meta.name.as_deref(), Some("zig_sdl"));
        assert_eq!(meta.version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn test_parse_string_name_and_quoted_identifier() {
        let meta = parse_zon(r#".{ .name = "old-style", .version = "1.2.3" }"#);
        assert_eq!(meta.name.as_deref(), Some("old-style"));

        let meta = parse_zon(r#".{ .name = .@"my-pkg", .version = "1.2.3" }"#);
        assert_eq!(meta.name.as_deref(), Some("my-pkg"));
    }

    #[test]
    fn test_nested_fields_are_ignored() {
        let meta = parse_zon(
            r#".{
                .version = "2.0.0",
                .dependencies = .{
                    .sdl = .{
                        // a nested struct must not contribute fields, even
                        // one literally called .name or .version
                        .name = .not_this_one,
                        .version = "9.9.9",
                        .url = "https://example.com/sdl.tar.gz",
                        .hash = "1220deadbeef",
                    },
                },
                .name = .real_name,
                .paths = .{ "build.zig", "src" },
            }"#,
        );
        assert_eq!(meta.name.as_deref(), Some("real_name"));
        assert_eq!(meta.version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn test_comments_and_strings_do_not_confuse_the_scanner() {
        let meta = parse_zon(
            r#".{
                // .name = .commented_out,
                .description = "contains .name = .fake and a { brace",
                .name = .actual,
                .version = "0.0.1",
            }"#,
        );
        assert_eq!(meta.name.as_deref(), Some("actual"));
        assert_eq!(meta.version.as_deref(), Some("0.0.1"));
    }

    #[test]
    fn test_missing_file_provides_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut provider = ZonMetadataProvider::new(dir.path(), false);
        assert_eq!(provider.name().unwrap(), None);
        assert_eq!(provider.version().unwrap(), None);
    }

    #[test]
    fn test_provider_reads_and_ignores() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(
            dir.path().join("build.zig.zon"),
            r#".{ .name = .from_zon, .version = "3.2.1" }"#,
        )
        .unwrap();

        let mut provider = ZonMetadataProvider::new(dir.path(), false);
        assert_eq!(provider.name().unwrap().as_deref(), Some("from_zon"));
        assert_eq!(provider.version().unwrap().unwrap().to_string(), "3.2.1");
        assert_eq!(provider.input_globs(), vec!["build.zig.zon".to_string()]);

        let mut ignoring = ZonMetadataProvider::new(dir.path(), true);
        assert_eq!(ignoring.name().unwrap(), None);
        assert!(ignoring.input_globs().is_empty());
    }

    #[test]
    fn test_invalid_version_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(
            dir.path().join("build.zig.zon"),
            r#".{ .name = .pkg, .version = "not a version" }"#,
        )
        .unwrap();
        let mut provider = ZonMetadataProvider::new(dir.path(), false);
        assert!(provider.version().is_err());
    }
}
