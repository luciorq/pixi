use indexmap::IndexMap;
use pixi_build_backend::generated_recipe::BackendConfig;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The optimization mode passed to `zig build` as `-Doptimize=<mode>`.
///
/// These are the four modes registered by `b.standardOptimizeOption` in a
/// `build.zig`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum ZigOptimize {
    Debug,
    ReleaseSafe,
    /// The default: packages are redistributable release artifacts.
    #[default]
    ReleaseFast,
    ReleaseSmall,
}

impl ZigOptimize {
    pub fn as_str(&self) -> &'static str {
        match self {
            ZigOptimize::Debug => "Debug",
            ZigOptimize::ReleaseSafe => "ReleaseSafe",
            ZigOptimize::ReleaseFast => "ReleaseFast",
            ZigOptimize::ReleaseSmall => "ReleaseSmall",
        }
    }
}

/// The Windows ABI used when deriving the zig target triple.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowsAbi {
    /// MinGW-w64/UCRT (`*-windows-gnu`). Zig bundles the MinGW headers and
    /// UCRT import libraries, so this works out of the box, including when
    /// cross-compiling.
    #[default]
    Gnu,
    /// MSVC ABI (`*-windows-msvc`). Requires MSVC libraries to be available.
    Msvc,
}

impl WindowsAbi {
    pub fn as_str(&self) -> &'static str {
        match self {
            WindowsAbi::Gnu => "gnu",
            WindowsAbi::Msvc => "msvc",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ZigBackendConfig {
    /// Extra args to pass to `zig build`, appended after the args the backend
    /// derives itself.
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Environment variables set while the build script runs.
    #[serde(default)]
    pub env: IndexMap<String, String>,
    /// Deprecated. Setting this has no effect; debug data is always written to
    /// the `debug` subdirectory of the work directory.
    #[serde(alias = "debug_dir")]
    pub debug_dir: Option<PathBuf>,
    /// Extra input globs to include in addition to the default ones.
    #[serde(default)]
    pub extra_input_globs: Vec<String>,
    /// Override the zig target triple passed as `-Dtarget=<triple>`. The
    /// special value `"native"` suppresses both `-Dtarget` and `-Dcpu`,
    /// producing a host-native build.
    pub target: Option<String>,
    /// The CPU feature set passed as `-Dcpu=<cpu>`. Defaults to `baseline` so
    /// that packages run on any machine of the target architecture rather
    /// than only on machines with the build machine's CPU features.
    pub cpu: Option<String>,
    /// The optimization mode passed as `-Doptimize=<mode>`. Defaults to
    /// `ReleaseFast`.
    pub optimize: Option<ZigOptimize>,
    /// The glibc version appended to `*-linux-gnu` target triples, e.g.
    /// `2.28` producing `x86_64-linux-gnu.2.28`. Defaults to `2.28`, matching
    /// the conda-forge sysroot. Set to an empty string to omit the suffix.
    pub glibc_version: Option<String>,
    /// The ABI used for Windows targets. Defaults to `gnu` (MinGW-w64/UCRT),
    /// which zig supports out of the box on every build machine.
    pub windows_abi: Option<WindowsAbi>,
    /// Minimum macOS version appended to `*-macos` target triples, e.g.
    /// `11.0` producing `aarch64-macos.11.0`. No suffix when unset.
    pub macos_deployment_target: Option<String>,
    /// Whether the project's `build.zig` uses `b.standardTargetOptions` and
    /// `b.standardOptimizeOption` (the ecosystem convention). When set to
    /// `false` the backend emits no `-Dtarget`/`-Dcpu`/`-Doptimize` flags at
    /// all and only `extra-args` control the build. Defaults to `true`.
    pub standard_options: Option<bool>,
    /// Whether rattler-build relocates binaries (rpath/install-name
    /// rewriting) after the build. When unset, relocation is enabled except
    /// when cross-compiling for macOS from a non-mac machine: the Mach-O
    /// post-processing needs `install_name_tool`/`codesign`, which only
    /// exist on macOS, and zig links and ad-hoc-signs its artifacts itself.
    pub binary_relocation: Option<bool>,
    /// Whether to export the zig-based C toolchain (`CC="zig cc ..."`,
    /// `CXX="zig c++ ..."`, `AR="zig ar"`, `RANLIB="zig ranlib"`, plus
    /// untargeted `CC_FOR_BUILD`/`CXX_FOR_BUILD`) so that anything the build
    /// spawns — configure/make steps, dependency build scripts probing `$CC`
    /// — compiles and links with zig for the same target. C/C++ sources
    /// compiled inside `build.zig` always use zig's bundled clang regardless
    /// of this setting. Defaults to `true`.
    pub export_c_toolchain: Option<bool>,
}

impl BackendConfig for ZigBackendConfig {
    fn debug_dir(&self) -> Option<&Path> {
        self.debug_dir.as_deref()
    }

    /// Merge this configuration with a target-specific configuration.
    /// Target-specific values override base values using the following rules:
    /// - `extra_args`: platform-specific completely replaces base
    /// - `env`: platform env vars override base, others merge
    /// - `extra_input_globs`: platform-specific completely replaces base
    /// - scalar options: platform-specific value wins when set
    fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
        Ok(Self {
            extra_args: if target_config.extra_args.is_empty() {
                self.extra_args.clone()
            } else {
                target_config.extra_args.clone()
            },
            env: {
                let mut merged_env = self.env.clone();
                merged_env.extend(target_config.env.clone());
                merged_env
            },
            debug_dir: self.debug_dir.clone(),
            extra_input_globs: if target_config.extra_input_globs.is_empty() {
                self.extra_input_globs.clone()
            } else {
                target_config.extra_input_globs.clone()
            },
            target: target_config.target.clone().or_else(|| self.target.clone()),
            cpu: target_config.cpu.clone().or_else(|| self.cpu.clone()),
            optimize: target_config.optimize.or(self.optimize),
            glibc_version: target_config
                .glibc_version
                .clone()
                .or_else(|| self.glibc_version.clone()),
            windows_abi: target_config.windows_abi.or(self.windows_abi),
            macos_deployment_target: target_config
                .macos_deployment_target
                .clone()
                .or_else(|| self.macos_deployment_target.clone()),
            standard_options: target_config.standard_options.or(self.standard_options),
            binary_relocation: target_config.binary_relocation.or(self.binary_relocation),
            export_c_toolchain: target_config.export_c_toolchain.or(self.export_c_toolchain),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_ensure_deserialize_from_empty() {
        let json_data = json!({});
        serde_json::from_value::<ZigBackendConfig>(json_data).unwrap();
    }

    #[test]
    fn test_deserialize_full_config() {
        let json_data = json!({
            "extra-args": ["-Dfoo=bar"],
            "env": {"KEY": "value"},
            "extra-input-globs": ["assets/**"],
            "target": "x86_64-linux-musl",
            "cpu": "x86_64_v3",
            "optimize": "ReleaseSafe",
            "glibc-version": "2.34",
            "windows-abi": "msvc",
            "macos-deployment-target": "11.0",
            "standard-options": true,
        });
        let config = serde_json::from_value::<ZigBackendConfig>(json_data).unwrap();
        assert_eq!(config.optimize, Some(ZigOptimize::ReleaseSafe));
        assert_eq!(config.windows_abi, Some(WindowsAbi::Msvc));
        assert_eq!(config.glibc_version.as_deref(), Some("2.34"));
    }

    #[test]
    fn test_invalid_optimize_rejected() {
        let json_data = json!({"optimize": "Fastest"});
        assert!(serde_json::from_value::<ZigBackendConfig>(json_data).is_err());
    }

    #[test]
    fn test_merge_with_target_config() {
        let mut base_env = indexmap::IndexMap::new();
        base_env.insert("BASE_VAR".to_string(), "base_value".to_string());
        base_env.insert("SHARED_VAR".to_string(), "base_shared".to_string());

        let base_config = ZigBackendConfig {
            extra_args: vec!["--base-arg".to_string()],
            env: base_env,
            optimize: Some(ZigOptimize::ReleaseSmall),
            glibc_version: Some("2.28".to_string()),
            ..Default::default()
        };

        let mut target_env = indexmap::IndexMap::new();
        target_env.insert("TARGET_VAR".to_string(), "target_value".to_string());
        target_env.insert("SHARED_VAR".to_string(), "target_shared".to_string());

        let target_config = ZigBackendConfig {
            extra_args: vec!["--target-arg".to_string()],
            env: target_env,
            cpu: Some("x86_64_v2".to_string()),
            ..Default::default()
        };

        let merged = base_config
            .merge_with_target_config(&target_config)
            .unwrap();

        // extra_args are completely overridden
        assert_eq!(merged.extra_args, vec!["--target-arg".to_string()]);

        // env merges with target taking precedence
        assert_eq!(merged.env.get("BASE_VAR"), Some(&"base_value".to_string()));
        assert_eq!(
            merged.env.get("TARGET_VAR"),
            Some(&"target_value".to_string())
        );
        assert_eq!(
            merged.env.get("SHARED_VAR"),
            Some(&"target_shared".to_string())
        );

        // scalars: target wins when set, base otherwise
        assert_eq!(merged.cpu.as_deref(), Some("x86_64_v2"));
        assert_eq!(merged.optimize, Some(ZigOptimize::ReleaseSmall));
        assert_eq!(merged.glibc_version.as_deref(), Some("2.28"));
    }
}
