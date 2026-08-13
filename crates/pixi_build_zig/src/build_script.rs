use minijinja::Environment;
use serde::Serialize;

#[derive(Serialize)]
pub struct BuildScriptContext {
    /// The directory containing `build.zig`.
    pub source_dir: String,

    /// Args to pass to `zig build`: the `-D` flags derived by the backend
    /// followed by the user's `extra-args`.
    pub args: Vec<String>,

    /// When set, `CC`/`CXX`/`AR`/`RANLIB` (and untargeted `CC_FOR_BUILD` /
    /// `CXX_FOR_BUILD`) are exported pointing at zig's C toolchain. Holds the
    /// clang-style target flags appended to `zig cc` / `zig c++`, e.g.
    /// `" -target x86_64-linux-gnu.2.28 -mcpu=baseline"`; may be empty for a
    /// native build.
    pub cc_flags: Option<String>,

    /// True when the build runs under bash (i.e. the build machine is not
    /// Windows).
    pub is_bash: bool,

    /// True when the TARGET platform is Windows: the install prefix then
    /// points at the `Library` subdirectory of the prefix, per conda
    /// convention, regardless of the build machine.
    pub host_is_windows: bool,
}

impl BuildScriptContext {
    pub fn render(&self) -> String {
        let env = Environment::new();
        let template = env
            .template_from_str(include_str!("build_script.j2"))
            .unwrap();
        template.render(self).unwrap().trim().to_string()
    }
}

#[cfg(test)]
mod test {
    fn render(is_bash: bool, host_is_windows: bool, cc_flags: Option<String>) -> String {
        super::BuildScriptContext {
            source_dir: String::from("my-source-dir"),
            args: vec![
                "-Dtarget=x86_64-linux-gnu.2.28".to_string(),
                "-Dcpu=baseline".to_string(),
                "-Doptimize=ReleaseFast".to_string(),
            ],
            cc_flags,
            is_bash,
            host_is_windows,
        }
        .render()
    }

    #[test]
    fn test_build_script_bash() {
        insta::assert_snapshot!(render(true, false, None), @r###"
        export ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-$SRC_DIR/.zig-global-cache}"
        export ZIG_LOCAL_CACHE_DIR="${ZIG_LOCAL_CACHE_DIR:-$SRC_DIR/.zig-local-cache}"

        mkdir -p "$PREFIX"
        zig build --build-file "my-source-dir/build.zig" --prefix "$PREFIX" --search-prefix "$PREFIX" -Dtarget=x86_64-linux-gnu.2.28 -Dcpu=baseline -Doptimize=ReleaseFast
        "###);
    }

    #[test]
    fn test_build_script_cmdexe() {
        insta::assert_snapshot!(render(false, true, None), @r###"
        if not defined ZIG_GLOBAL_CACHE_DIR SET "ZIG_GLOBAL_CACHE_DIR=%SRC_DIR%\.zig-global-cache"
        if not defined ZIG_LOCAL_CACHE_DIR SET "ZIG_LOCAL_CACHE_DIR=%SRC_DIR%\.zig-local-cache"

        if not exist "%LIBRARY_PREFIX%" mkdir "%LIBRARY_PREFIX%"
        zig build --build-file "my-source-dir\build.zig" --prefix "%LIBRARY_PREFIX%" --search-prefix "%LIBRARY_PREFIX%" -Dtarget=x86_64-linux-gnu.2.28 -Dcpu=baseline -Doptimize=ReleaseFast
        if errorlevel 1 exit 1
        "###);
    }

    #[test]
    fn test_build_script_c_toolchain_bash() {
        let flags = String::from(" -target x86_64-linux-gnu.2.28 -mcpu=baseline");
        insta::assert_snapshot!(render(true, false, Some(flags)), @r###"
        export ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-$SRC_DIR/.zig-global-cache}"
        export ZIG_LOCAL_CACHE_DIR="${ZIG_LOCAL_CACHE_DIR:-$SRC_DIR/.zig-local-cache}"

        export CC="zig cc -target x86_64-linux-gnu.2.28 -mcpu=baseline"
        export CXX="zig c++ -target x86_64-linux-gnu.2.28 -mcpu=baseline"
        export AR="zig ar"
        export RANLIB="zig ranlib"
        export CC_FOR_BUILD="zig cc"
        export CXX_FOR_BUILD="zig c++"

        mkdir -p "$PREFIX"
        zig build --build-file "my-source-dir/build.zig" --prefix "$PREFIX" --search-prefix "$PREFIX" -Dtarget=x86_64-linux-gnu.2.28 -Dcpu=baseline -Doptimize=ReleaseFast
        "###);
    }

    #[test]
    fn test_build_script_c_toolchain_cmdexe() {
        let flags = String::from(" -target x86_64-windows-gnu -mcpu=baseline");
        insta::assert_snapshot!(render(false, true, Some(flags)), @r###"
        if not defined ZIG_GLOBAL_CACHE_DIR SET "ZIG_GLOBAL_CACHE_DIR=%SRC_DIR%\.zig-global-cache"
        if not defined ZIG_LOCAL_CACHE_DIR SET "ZIG_LOCAL_CACHE_DIR=%SRC_DIR%\.zig-local-cache"

        SET "CC=zig cc -target x86_64-windows-gnu -mcpu=baseline"
        SET "CXX=zig c++ -target x86_64-windows-gnu -mcpu=baseline"
        SET "AR=zig ar"
        SET "RANLIB=zig ranlib"
        SET "CC_FOR_BUILD=zig cc"
        SET "CXX_FOR_BUILD=zig c++"

        if not exist "%LIBRARY_PREFIX%" mkdir "%LIBRARY_PREFIX%"
        zig build --build-file "my-source-dir\build.zig" --prefix "%LIBRARY_PREFIX%" --search-prefix "%LIBRARY_PREFIX%" -Dtarget=x86_64-linux-gnu.2.28 -Dcpu=baseline -Doptimize=ReleaseFast
        if errorlevel 1 exit 1
        "###);
    }

    #[test]
    fn test_build_script_cross_to_windows_from_bash() {
        // Cross-compiling FOR Windows FROM a unix machine: bash syntax, but
        // the install prefix must still follow the conda Library\ convention
        // of the target platform.
        insta::assert_snapshot!(render(true, true, None), @r###"
        export ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-$SRC_DIR/.zig-global-cache}"
        export ZIG_LOCAL_CACHE_DIR="${ZIG_LOCAL_CACHE_DIR:-$SRC_DIR/.zig-local-cache}"

        mkdir -p "$PREFIX/Library"
        zig build --build-file "my-source-dir/build.zig" --prefix "$PREFIX/Library" --search-prefix "$PREFIX/Library" -Dtarget=x86_64-linux-gnu.2.28 -Dcpu=baseline -Doptimize=ReleaseFast
        "###);
    }

    #[test]
    fn test_build_script_no_args() {
        let script = super::BuildScriptContext {
            source_dir: String::from("my-source-dir"),
            args: vec![],
            cc_flags: None,
            is_bash: true,
            host_is_windows: false,
        }
        .render();
        insta::assert_snapshot!(script, @r###"
        export ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-$SRC_DIR/.zig-global-cache}"
        export ZIG_LOCAL_CACHE_DIR="${ZIG_LOCAL_CACHE_DIR:-$SRC_DIR/.zig-local-cache}"

        mkdir -p "$PREFIX"
        zig build --build-file "my-source-dir/build.zig" --prefix "$PREFIX" --search-prefix "$PREFIX"
        "###);
    }
}
