//! Mapping from conda platforms to zig target triples.
//!
//! The backend always emits an explicit `-Dtarget` (and `-Dcpu`) instead of
//! relying on zig's native default. This makes a native build just the
//! special case of a cross build where the target happens to equal the build
//! machine, and it keeps packages redistributable: without `-Dcpu=baseline`
//! zig compiles for the build machine's CPU features, producing binaries
//! that can crash with illegal instructions on older machines.

use miette::miette;
use rattler_conda_types::Platform;

use crate::config::ZigBackendConfig;

/// The default glibc version appended to `*-linux-gnu` triples. Matches the
/// glibc of the conda-forge `sysroot_linux-64` in current use.
const DEFAULT_GLIBC_VERSION: &str = "2.28";

/// Derive the zig target triple for a conda platform.
///
/// Returns an error for platforms zig has no obvious triple for; users can
/// always take over with the `target` config option.
pub fn zig_triple(host_platform: Platform, config: &ZigBackendConfig) -> miette::Result<String> {
    let glibc = match config.glibc_version.as_deref() {
        Some("") => String::new(),
        Some(version) => format!(".{version}"),
        None => format!(".{DEFAULT_GLIBC_VERSION}"),
    };
    let windows_abi = config.windows_abi.unwrap_or_default();
    let macos = match config.macos_deployment_target.as_deref() {
        Some(version) => format!(".{version}"),
        None => String::new(),
    };

    let triple = match host_platform {
        Platform::Linux64 => format!("x86_64-linux-gnu{glibc}"),
        Platform::LinuxAarch64 => format!("aarch64-linux-gnu{glibc}"),
        Platform::Linux32 => format!("x86-linux-gnu{glibc}"),
        Platform::LinuxArmV6l | Platform::LinuxArmV7l => {
            format!("arm-linux-gnueabihf{glibc}")
        }
        Platform::LinuxPpc64le => format!("powerpc64le-linux-gnu{glibc}"),
        Platform::LinuxPpc64 => format!("powerpc64-linux-gnu{glibc}"),
        Platform::LinuxS390X => format!("s390x-linux-gnu{glibc}"),
        Platform::LinuxRiscv64 => format!("riscv64-linux-gnu{glibc}"),
        Platform::LinuxLoongArch64 => format!("loongarch64-linux-gnu{glibc}"),
        Platform::Osx64 => format!("x86_64-macos{macos}"),
        Platform::OsxArm64 => format!("aarch64-macos{macos}"),
        Platform::Win64 => format!("x86_64-windows-{}", windows_abi.as_str()),
        Platform::WinArm64 => format!("aarch64-windows-{}", windows_abi.as_str()),
        Platform::Win32 => format!("x86-windows-{}", windows_abi.as_str()),
        Platform::EmscriptenWasm32 => "wasm32-emscripten".to_string(),
        Platform::WasiWasm32 => "wasm32-wasi".to_string(),
        Platform::FreeBsd64 => "x86_64-freebsd".to_string(),
        Platform::FreeBsdArm64 => "aarch64-freebsd".to_string(),
        other => {
            return Err(miette!(
                "pixi-build-zig does not know a zig target triple for platform `{other}`; \
                 set `target` in [package.build.config] to provide one explicitly"
            ));
        }
    };
    Ok(triple)
}

/// The target flags appended to `zig cc` / `zig c++` when exporting the C
/// toolchain environment (`CC`, `CXX`, ...). Empty when the user opted into a
/// native build via `target = "native"`.
///
/// These are clang-style flags (`-target`, `-mcpu`), not `zig build` `-D`
/// options, because they are consumed by subprocesses the build spawns
/// (configure/make scripts probing `$CC`), not by `build.zig`.
pub fn zig_cc_flags(host_platform: Platform, config: &ZigBackendConfig) -> miette::Result<String> {
    let triple = match config.target.as_deref() {
        Some("native") => return Ok(String::new()),
        Some(triple) => triple.to_string(),
        None => zig_triple(host_platform, config)?,
    };
    let cpu = config.cpu.as_deref().unwrap_or("baseline");
    Ok(format!(" -target {triple} -mcpu={cpu}"))
}

/// The `-D` args the backend derives from the platform and configuration.
///
/// Empty when `standard-options = false`, because the flags only exist for
/// projects whose `build.zig` calls `b.standardTargetOptions` and
/// `b.standardOptimizeOption`.
pub fn zig_build_args(
    host_platform: Platform,
    config: &ZigBackendConfig,
) -> miette::Result<Vec<String>> {
    if !config.standard_options.unwrap_or(true) {
        return Ok(Vec::new());
    }

    let mut args = Vec::new();
    match config.target.as_deref() {
        // `native` opts out of explicit target selection entirely.
        Some("native") => {}
        Some(triple) => {
            args.push(format!("-Dtarget={triple}"));
            args.push(format!(
                "-Dcpu={}",
                config.cpu.as_deref().unwrap_or("baseline")
            ));
        }
        None => {
            args.push(format!("-Dtarget={}", zig_triple(host_platform, config)?));
            args.push(format!(
                "-Dcpu={}",
                config.cpu.as_deref().unwrap_or("baseline")
            ));
        }
    }
    args.push(format!(
        "-Doptimize={}",
        config.optimize.unwrap_or_default().as_str()
    ));
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WindowsAbi, ZigOptimize};

    fn config() -> ZigBackendConfig {
        ZigBackendConfig::default()
    }

    #[test]
    fn test_linux_triples_pin_glibc() {
        assert_eq!(
            zig_triple(Platform::Linux64, &config()).unwrap(),
            "x86_64-linux-gnu.2.28"
        );
        assert_eq!(
            zig_triple(Platform::LinuxAarch64, &config()).unwrap(),
            "aarch64-linux-gnu.2.28"
        );
        assert_eq!(
            zig_triple(Platform::LinuxPpc64le, &config()).unwrap(),
            "powerpc64le-linux-gnu.2.28"
        );
    }

    #[test]
    fn test_glibc_version_override_and_removal() {
        let cfg = ZigBackendConfig {
            glibc_version: Some("2.34".to_string()),
            ..config()
        };
        assert_eq!(
            zig_triple(Platform::Linux64, &cfg).unwrap(),
            "x86_64-linux-gnu.2.34"
        );

        let cfg = ZigBackendConfig {
            glibc_version: Some(String::new()),
            ..config()
        };
        assert_eq!(
            zig_triple(Platform::Linux64, &cfg).unwrap(),
            "x86_64-linux-gnu"
        );
    }

    #[test]
    fn test_windows_defaults_to_gnu_abi() {
        assert_eq!(
            zig_triple(Platform::Win64, &config()).unwrap(),
            "x86_64-windows-gnu"
        );
        assert_eq!(
            zig_triple(Platform::WinArm64, &config()).unwrap(),
            "aarch64-windows-gnu"
        );

        let cfg = ZigBackendConfig {
            windows_abi: Some(WindowsAbi::Msvc),
            ..config()
        };
        assert_eq!(
            zig_triple(Platform::Win64, &cfg).unwrap(),
            "x86_64-windows-msvc"
        );
    }

    #[test]
    fn test_macos_deployment_target_suffix() {
        assert_eq!(
            zig_triple(Platform::OsxArm64, &config()).unwrap(),
            "aarch64-macos"
        );
        let cfg = ZigBackendConfig {
            macos_deployment_target: Some("11.0".to_string()),
            ..config()
        };
        assert_eq!(
            zig_triple(Platform::OsxArm64, &cfg).unwrap(),
            "aarch64-macos.11.0"
        );
    }

    #[test]
    fn test_unknown_platform_is_an_error() {
        let err = zig_triple(Platform::ZosZ, &config()).unwrap_err();
        assert!(err.to_string().contains("zos-z"), "got: {err}");
    }

    #[test]
    fn test_build_args_defaults() {
        let args = zig_build_args(Platform::Linux64, &config()).unwrap();
        assert_eq!(
            args,
            vec![
                "-Dtarget=x86_64-linux-gnu.2.28",
                "-Dcpu=baseline",
                "-Doptimize=ReleaseFast",
            ]
        );
    }

    #[test]
    fn test_build_args_native_target_keeps_optimize() {
        let cfg = ZigBackendConfig {
            target: Some("native".to_string()),
            optimize: Some(ZigOptimize::ReleaseSafe),
            ..config()
        };
        let args = zig_build_args(Platform::Linux64, &cfg).unwrap();
        assert_eq!(args, vec!["-Doptimize=ReleaseSafe"]);
    }

    #[test]
    fn test_build_args_explicit_target_override() {
        let cfg = ZigBackendConfig {
            target: Some("x86_64-linux-musl".to_string()),
            cpu: Some("x86_64_v2".to_string()),
            ..config()
        };
        let args = zig_build_args(Platform::Linux64, &cfg).unwrap();
        assert_eq!(
            args,
            vec![
                "-Dtarget=x86_64-linux-musl",
                "-Dcpu=x86_64_v2",
                "-Doptimize=ReleaseFast",
            ]
        );
    }

    #[test]
    fn test_cc_flags_follow_target() {
        assert_eq!(
            zig_cc_flags(Platform::Linux64, &config()).unwrap(),
            " -target x86_64-linux-gnu.2.28 -mcpu=baseline"
        );
        let cfg = ZigBackendConfig {
            target: Some("native".to_string()),
            ..config()
        };
        assert_eq!(zig_cc_flags(Platform::Linux64, &cfg).unwrap(), "");
    }

    #[test]
    fn test_build_args_standard_options_disabled() {
        let cfg = ZigBackendConfig {
            standard_options: Some(false),
            ..config()
        };
        let args = zig_build_args(Platform::Linux64, &cfg).unwrap();
        assert!(args.is_empty());
    }
}
