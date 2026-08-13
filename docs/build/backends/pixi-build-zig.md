# pixi-build-zig

The `pixi-build-zig` backend builds projects that use the
[Zig build system](https://ziglang.org/learn/build-system/) (`build.zig`).
Because zig is a native cross-compiler that ships its own libc headers,
MinGW-w64/UCRT import libraries and macOS link stubs, this backend treats
cross-compilation as the normal case: it always passes an explicit target
derived from the platform the package is built for, so `pixi build
--target-platform` works the same way a native build does.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    This is why we require users to opt in to that feature by adding "pixi-build" to `workspace.preview`.

    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```

## Overview

The backend automatically generates conda packages from Zig projects by:

- **Using Zig Build**: runs `zig build --prefix $PREFIX`, so artifacts marked
  with `b.installArtifact()` land in the right place in the conda prefix
- **Explicit targets**: derives `-Dtarget`/`-Dcpu` from the conda target
  platform (`-Dcpu=baseline` by default, so packages run on any machine of
  the target architecture, not just ones with the build machine's CPU
  features)
- **Pinned glibc**: linux targets default to `*-linux-gnu.2.28`, matching the
  conda-forge sysroot
- **Release builds by default**: `-Doptimize=ReleaseFast` unless configured
  otherwise
- **Conda host dependencies**: passes `--search-prefix` so headers and
  libraries from host dependencies resolve
- **Zig C toolchain everywhere**: exports `CC="zig cc ..."`,
  `CXX="zig c++ ..."`, `AR="zig ar"` and `RANLIB="zig ranlib"` so build steps
  that spawn external tools compile and link with zig for the same target
- **Hermetic caches**: keeps `ZIG_GLOBAL_CACHE_DIR`/`ZIG_LOCAL_CACHE_DIR`
  inside the work directory

## Basic Usage

To use the Zig backend in your `pixi.toml`, add it to your package's build configuration:

```toml
[package]
name = "zig_package"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-zig", version = "*" }
```

### build.zig Requirements

The backend assumes the ecosystem convention that `build.zig` calls
`b.standardTargetOptions` and `b.standardOptimizeOption`, which register the
`-Dtarget`, `-Dcpu` and `-Doptimize` options. Projects that do not follow the
convention can set `standard-options = false` (see below).

Artifacts must be marked for installation with `b.installArtifact()`:

- **Executables** → `$PREFIX/bin/`
- **Libraries** → `$PREFIX/lib/`
- **Headers** → `$PREFIX/include/`

Example `build.zig`:

```zig
pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const exe = b.addExecutable(.{
        .name = "my-tool",
        .root_module = b.createModule(.{
            .root_source_file = b.path("src/main.zig"),
            .target = target,
            .optimize = optimize,
        }),
    });

    // This marks the executable for installation
    b.installArtifact(exe);
}
```

### Required Dependencies

The backend automatically adds `zig` to your build dependencies. You can add
it explicitly if you need a specific version:

```toml
[package.build-dependencies]
zig = ">=0.15.0,<0.16"
```

## Cross-compilation

The zig compiler that runs on the build machine can produce binaries for any
supported target, so building for another platform of the same OS family
works without any extra toolchain packages:

```shell
pixi build --target-platform linux-aarch64   # on a linux-64 machine
pixi build --target-platform win-arm64       # on a win-64 machine
```

The conda target platform maps to zig target triples as follows:

| conda platform | zig `-Dtarget` |
|---|---|
| `linux-64` | `x86_64-linux-gnu.<glibc-version>` |
| `linux-aarch64` | `aarch64-linux-gnu.<glibc-version>` |
| `osx-64` | `x86_64-macos[.<macos-deployment-target>]` |
| `osx-arm64` | `aarch64-macos[.<macos-deployment-target>]` |
| `win-64` | `x86_64-windows-<windows-abi>` |
| `win-arm64` | `aarch64-windows-<windows-abi>` |
| `wasi-wasm32` | `wasm32-wasi` |

Other linux architectures (ppc64le, s390x, riscv64, loongarch64, armv6/7) map
analogously. For platforms without a known mapping the backend errors and the
`target` option must be set explicitly.

## Configuration Options

You can customize the backend behavior using the `[package.build.config]`
section in your `pixi.toml`.

### `target`

- **Type**: `String`
- **Default**: derived from the target platform

Override the zig target triple passed as `-Dtarget`. The special value
`"native"` suppresses `-Dtarget`/`-Dcpu` entirely, producing a host-native
build (not recommended for redistributable packages).

```toml
[package.build.config]
target = "x86_64-linux-musl"
```

### `cpu`

- **Type**: `String`
- **Default**: `baseline`

The CPU feature set passed as `-Dcpu`. The default `baseline` produces
binaries that run on any machine of the target architecture. Raise it for
specialized builds, e.g. `x86_64_v3`.

### `optimize`

- **Type**: `String` (one of `Debug`, `ReleaseSafe`, `ReleaseFast`, `ReleaseSmall`)
- **Default**: `ReleaseFast`

The optimization mode passed as `-Doptimize`.

### `glibc-version`

- **Type**: `String`
- **Default**: `2.28`

The glibc version appended to `*-linux-gnu` triples. The default matches the
conda-forge sysroot. Set to an empty string to omit the suffix.

### `windows-abi`

- **Type**: `String` (one of `gnu`, `msvc`)
- **Default**: `gnu`

The ABI for Windows targets. `gnu` (MinGW-w64/UCRT) works out of the box on
every build machine because zig bundles the MinGW headers and UCRT import
libraries; `msvc` requires MSVC libraries to be available.

### `macos-deployment-target`

- **Type**: `String`
- **Default**: unset

Minimum macOS version appended to `*-macos` triples, e.g. `11.0`.

### `standard-options`

- **Type**: `Boolean`
- **Default**: `true`

Set to `false` when the project's `build.zig` does not use
`b.standardTargetOptions`/`b.standardOptimizeOption`; the backend then emits
no `-D` flags at all and only `extra-args` control the build.

### `export-c-toolchain`

- **Type**: `Boolean`
- **Default**: `true`

Exports `CC`, `CXX`, `AR` and `RANLIB` pointing at zig's C toolchain
(targeting the same triple as the zig build), plus untargeted
`CC_FOR_BUILD`/`CXX_FOR_BUILD` for tools that must run on the build machine
during cross builds. This routes configure/make steps and dependency build
scripts through zig. Note that C/C++ sources compiled inside `build.zig`
always use zig's bundled clang regardless of this setting.

### `extra-args`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` — platform-specific arguments completely replace base arguments

Additional command-line arguments appended to the `zig build` command after
the arguments the backend derives itself.

```toml
[package.build.config]
extra-args = ["-Dfeature=on"]
```

### `env`

- **Type**: `Map<String, String>`
- **Default**: `{}`
- **Target Merge Behavior**: `Merge` — platform variables override base variables with the same name, others are merged

Environment variables set during the build.

```toml
[package.build.config]
env = { ZIG_VERBOSE = "1" }
```

### `extra-input-globs`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` — platform-specific globs completely replace base globs

Extra input globs considered for rebuild detection, in addition to the
defaults (`**/*.zig`, `build.zig`, `build.zig.zon`).

## Limitations

- Dependencies declared in `build.zig.zon` must be available offline (e.g.
  vendored) because the build environment has no network access.
- Cross-compiling to macOS system frameworks requires an SDK; plain
  executables and libraries work without one.
- On Windows the build installs into `%LIBRARY_PREFIX%` (the `Library\`
  subdirectory of the prefix), following the conda convention.
