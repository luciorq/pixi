mod build_script;
mod config;
mod target;

use build_script::BuildScriptContext;
use config::ZigBackendConfig;
use miette::IntoDiagnostic;
use pixi_build_backend::{
    generated_recipe::{DefaultMetadataProvider, GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    tools::BackendIdentifier,
    variants::NormalizedKey,
};
use rattler_build_recipe::stage0::{BinaryRelocation, Item, Script, SerializableMatchSpec, Value};
use rattler_conda_types::{ChannelUrl, Platform};
use std::collections::HashSet;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Default, Clone)]
pub struct ZigGenerator {}

#[async_trait::async_trait]
impl GenerateRecipe for ZigGenerator {
    type Config = ZigBackendConfig;

    async fn generate_recipe(
        &self,
        model: &pixi_build_types::ProjectModel,
        config: &Self::Config,
        manifest_path: PathBuf,
        host_platform: Platform,
        _python_params: Option<PythonParams>,
        _variants: &HashSet<NormalizedKey>,
        _channels: Vec<ChannelUrl>,
        _cache_dir: Option<PathBuf>,
        _workspace_scratch_directory: Option<PathBuf>,
        _workspace_directory: Option<PathBuf>,
        _checkout_root: Option<PathBuf>,
    ) -> miette::Result<GeneratedRecipe> {
        // Determine the manifest root, because `manifest_path` can be
        // either a direct file path or a directory path.
        let manifest_root = if manifest_path.is_file() {
            manifest_path
                .parent()
                .ok_or_else(|| {
                    miette::Error::msg(format!(
                        "Manifest path {} is a file but has no parent directory.",
                        manifest_path.display()
                    ))
                })?
                .to_path_buf()
        } else {
            manifest_path.clone()
        };

        let mut generated_recipe =
            GeneratedRecipe::from_model(model.clone(), &mut DefaultMetadataProvider)
                .into_diagnostic()?;

        let requirements = &mut generated_recipe.recipe.requirements;

        // The zig compiler runs on the build machine, so it belongs in the
        // build requirements. A user-pinned `zig` build dependency coexists
        // with this spec; the solver intersects them.
        requirements.build.push(Item::Value(Value::new_concrete(
            SerializableMatchSpec::from("zig"),
            None,
        )));

        // Derive the `-Dtarget`/`-Dcpu`/`-Doptimize` flags from the host
        // platform (= the platform the package is built FOR). Emitting an
        // explicit target makes native builds and cross builds take the
        // exact same path.
        let mut args = target::zig_build_args(host_platform, config)?;
        args.extend(config.extra_args.iter().cloned());

        // Export the zig-based C toolchain for everything the build spawns,
        // unless the user opted out.
        let cc_flags = if config.export_c_toolchain.unwrap_or(true) {
            Some(target::zig_cc_flags(host_platform, config)?)
        } else {
            None
        };

        let build_script = BuildScriptContext {
            source_dir: manifest_root.display().to_string(),
            args,
            cc_flags,
            is_bash: !Platform::current().is_windows(),
            host_is_windows: host_platform.is_windows(),
        }
        .render();

        // Cross-compiling FOR macOS FROM a non-mac machine: rattler-build's
        // Mach-O post-processing shells out to install_name_tool/codesign,
        // which only exist on macOS (its builtin relinker cannot add the
        // default `lib/` rpath). Zig links and ad-hoc-signs its artifacts
        // itself, so default to skipping binary relocation there instead of
        // failing the build. `binary-relocation` in the config overrides
        // this in either direction.
        let cross_to_macos = host_platform.is_osx() && !Platform::current().is_osx();
        if !config.binary_relocation.unwrap_or(!cross_to_macos) {
            generated_recipe
                .recipe
                .build
                .dynamic_linking
                .binary_relocation = BinaryRelocation::Boolean(Value::new_concrete(false, None));
        }

        generated_recipe.recipe.build.script = Script::from_content(build_script)
            .with_env(
                config
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::new_concrete(v.clone(), None)))
                    .collect(),
            )
            .with_secrets(model.secrets.iter().cloned().collect());

        Ok(generated_recipe)
    }

    /// Returns the build input globs used by the backend.
    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        _workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<Vec<String>> {
        Ok([
            // Zig sources
            "**/*.zig",
            // Zig build files
            "build.zig",
            "build.zig.zon",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain(config.extra_input_globs.clone())
        .collect())
    }
}

pub fn main() {
    // Non-mac build machines have no system `codesign`, but rattler-build
    // (embedded in this process) can ad-hoc sign Mach-O binaries itself; it
    // gates that behind this environment variable. Default it on so that
    // relocation-enabled cross builds for macOS re-sign after relinking. An
    // explicit value from the caller wins.
    // SAFETY: no other threads exist yet; the tokio runtime starts below.
    if !Platform::current().is_osx() && std::env::var_os("RATTLER_BUILD_BUILTIN_CODESIGN").is_none()
    {
        unsafe { std::env::set_var("RATTLER_BUILD_BUILTIN_CODESIGN", "1") };
    }
    async_main();
}

#[tokio::main]
async fn async_main() {
    if let Err(err) = pixi_build_backend::cli::main(|log| {
        IntermediateBackendInstantiator::<ZigGenerator>::new(
            BackendIdentifier::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            log,
            Arc::default(),
        )
    })
    .await
    {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;

    macro_rules! project_fixture {
        ($($json:tt)+) => {
            serde_json::from_value::<pixi_build_types::ProjectModel>(
                serde_json::json!($($json)+)
            ).expect("Failed to create ProjectModel from JSON fixture.")
        };
    }

    fn script_content(recipe: &GeneratedRecipe) -> String {
        recipe
            .recipe
            .build
            .script
            .content
            .as_ref()
            .expect("script content should be set")
            .iter()
            .filter_map(|item| item.as_value().and_then(|v| v.as_concrete()))
            .cloned()
            .collect::<Vec<String>>()
            .join("\n")
    }

    #[tokio::test]
    async fn test_zig_is_in_build_requirements() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "runDependencies": {
                        "boltons": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let generated_recipe = ZigGenerator::default()
            .generate_recipe(
                &project_model,
                &ZigBackendConfig::default(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
        ".source[0].path" => "[ ... path ... ]",
        ".build.script" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_zig_is_added_even_if_already_present() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "buildDependencies": {
                        "zig": {
                            "binary": {
                                "version": "0.16.*"
                            }
                        }
                    }
                },
            }
        });

        let generated_recipe = ZigGenerator::default()
            .generate_recipe(
                &project_model,
                &ZigBackendConfig::default(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // The user spec and the backend-added spec both land in the recipe
        // and intersect in the solver.
        let zig_count = generated_recipe
            .recipe
            .requirements
            .build
            .iter()
            .filter(|item| {
                item.as_value()
                    .and_then(|v| v.as_concrete())
                    .and_then(|spec| spec.0.name.as_exact())
                    .is_some_and(|name| name.as_normalized() == "zig")
            })
            .count();
        assert_eq!(
            zig_count, 2,
            "expected user and backend zig in build requirements"
        );
    }

    #[tokio::test]
    async fn test_target_args_are_rendered() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = ZigGenerator::default()
            .generate_recipe(
                &project_model,
                &ZigBackendConfig::default(),
                PathBuf::from("."),
                Platform::WinArm64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        let content = script_content(&generated_recipe);
        assert!(content.contains("-Dtarget=aarch64-windows-gnu"));
        assert!(content.contains("-Dcpu=baseline"));
        assert!(content.contains("-Doptimize=ReleaseFast"));
    }

    #[tokio::test]
    async fn test_extra_args_come_after_derived_args() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = ZigGenerator::default()
            .generate_recipe(
                &project_model,
                &ZigBackendConfig {
                    extra_args: vec!["-Dfoo=bar".to_string()],
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        let content = script_content(&generated_recipe);
        let derived = content
            .find("-Dtarget=")
            .expect("derived target arg present");
        let extra = content.find("-Dfoo=bar").expect("extra arg present");
        assert!(derived < extra, "extra args must come last");
    }

    #[tokio::test]
    async fn test_c_toolchain_exported_by_default() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = ZigGenerator::default()
            .generate_recipe(
                &project_model,
                &ZigBackendConfig::default(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        let content = script_content(&generated_recipe);
        assert!(content.contains(r#"CC="zig cc -target x86_64-linux-gnu.2.28 -mcpu=baseline""#));
        assert!(content.contains(r#"RANLIB="zig ranlib""#));
        assert!(content.contains(r#"CC_FOR_BUILD="zig cc""#));
    }

    #[tokio::test]
    async fn test_c_toolchain_export_can_be_disabled() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = ZigGenerator::default()
            .generate_recipe(
                &project_model,
                &ZigBackendConfig {
                    export_c_toolchain: Some(false),
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        let content = script_content(&generated_recipe);
        assert!(!content.contains("CC="));
        assert!(!content.contains("AR="));
    }

    #[tokio::test]
    async fn test_standard_options_disabled_omits_d_flags() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = ZigGenerator::default()
            .generate_recipe(
                &project_model,
                &ZigBackendConfig {
                    standard_options: Some(false),
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        let content = script_content(&generated_recipe);
        assert!(!content.contains("-Dtarget="));
        assert!(!content.contains("-Dcpu="));
        assert!(!content.contains("-Doptimize="));
    }

    #[tokio::test]
    async fn test_env_vars_are_set() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let env = IndexMap::from([("foo".to_string(), "bar".to_string())]);

        let generated_recipe = ZigGenerator::default()
            .generate_recipe(
                &project_model,
                &ZigBackendConfig {
                    env: env.clone(),
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        insta::assert_yaml_snapshot!(generated_recipe.recipe.build.script,
        {
            ".content" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_cross_to_macos_disables_binary_relocation() {
        // These tests run on linux/windows CI machines; guard the one
        // platform where the cross-to-macos default does not apply.
        if Platform::current().is_osx() {
            return;
        }

        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generator = ZigGenerator::default();

        // Cross to macOS: relocation disabled by default.
        let recipe = generator
            .generate_recipe(
                &project_model,
                &ZigBackendConfig::default(),
                PathBuf::from("."),
                Platform::OsxArm64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");
        assert_eq!(
            recipe.recipe.build.dynamic_linking.binary_relocation,
            BinaryRelocation::Boolean(Value::new_concrete(false, None)),
        );

        // Same target with an explicit override: relocation stays enabled.
        let recipe = generator
            .generate_recipe(
                &project_model,
                &ZigBackendConfig {
                    binary_relocation: Some(true),
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::OsxArm64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");
        assert_eq!(
            recipe.recipe.build.dynamic_linking.binary_relocation,
            BinaryRelocation::default(),
        );

        // Non-mac target: relocation untouched.
        let recipe = generator
            .generate_recipe(
                &project_model,
                &ZigBackendConfig::default(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");
        assert_eq!(
            recipe.recipe.build.dynamic_linking.binary_relocation,
            BinaryRelocation::default(),
        );
    }

    #[test]
    fn test_input_globs_includes_extra_globs() {
        let config = ZigBackendConfig {
            extra_input_globs: vec!["custom/*.txt".to_string(), "extra/**/*.zig".to_string()],
            ..Default::default()
        };

        let generator = ZigGenerator::default();

        let result = generator
            .extract_input_globs_from_build(&config, PathBuf::new(), false)
            .unwrap();

        let contains = |needle: &str| result.iter().any(|g| g == needle);

        for extra_glob in &config.extra_input_globs {
            assert!(
                contains(extra_glob),
                "Result should contain extra glob: {extra_glob}"
            );
        }

        assert!(contains("**/*.zig"));
        assert!(contains("build.zig"));
        assert!(contains("build.zig.zon"));
    }
}
