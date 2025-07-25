mod utils;

use httpmock::prelude::*;
use moon_config::{
    BinConfig, BinEntry, ConfigLoader, NodePackageManager, NodeVersionFormat, ToolchainConfig,
    ToolchainPluginConfig,
};
use proto_core::{Id, PluginLocator, ProtoConfig, UnresolvedVersionSpec, warpgate::FileLocator};
use schematic::ConfigLoader as BaseLoader;
use serde_json::Value;
use serial_test::serial;
use starbase_sandbox::{create_empty_sandbox, create_sandbox};
use std::env;
use std::path::Path;
use utils::*;

const FILENAME: &str = ".moon/toolchain.yml";

fn load_config_from_file(path: &Path) -> ToolchainConfig {
    BaseLoader::<ToolchainConfig>::new()
        .file(path)
        .unwrap()
        .load()
        .unwrap()
        .config
}

fn load_config_from_root(root: &Path, proto: &ProtoConfig) -> miette::Result<ToolchainConfig> {
    ConfigLoader::default().load_toolchain_config(root, proto)
}

mod toolchain_config {
    use super::*;

    // #[test]
    // #[should_panic(
    //     expected = "unknown field `unknown`, expected one of `$schema`, `extends`, `bun`, `deno`, `node`, `rust`"
    // )]
    // fn error_unknown_field() {
    //     test_load_config(FILENAME, "unknown: 123", |path| {
    //         load_config_from_root(path, &ProtoConfig::default())
    //     });
    // }

    #[test]
    fn loads_defaults() {
        let config = test_load_config(FILENAME, "{}", |path| {
            load_config_from_root(path, &ProtoConfig::default())
        });

        assert!(config.deno.is_none());
        assert!(config.node.is_none());
        assert!(config.python.is_none());
        assert!(config.rust.is_none());
    }

    mod extends {
        use super::*;

        const SHARED_TOOLCHAIN: &str = r"
bun: {}
node: {}";

        #[test]
        fn recursive_merges() {
            let sandbox = create_sandbox("extends/toolchain");
            let config = test_config(sandbox.path().join("base-2.yml"), |path| {
                Ok(load_config_from_file(path))
            });

            let node = config.node.unwrap();

            assert_eq!(
                node.version.unwrap(),
                UnresolvedVersionSpec::parse("4.5.6").unwrap()
            );
            assert!(node.add_engines_constraint);
            assert!(!node.dedupe_on_lockfile_change);
            assert_eq!(node.package_manager, NodePackageManager::Yarn);

            let yarn = node.yarn.unwrap();

            assert_eq!(
                yarn.version.unwrap(),
                UnresolvedVersionSpec::parse("3.3.0").unwrap()
            );
        }

        #[test]
        fn recursive_merges_typescript() {
            let sandbox = create_sandbox("extends/toolchain");
            let config = test_config(sandbox.path().join("typescript-2.yml"), |path| {
                Ok(load_config_from_file(path))
            });

            let cfg = config.plugins.get("typescript").unwrap();

            assert_eq!(
                cfg.config.get("rootConfigFileName").unwrap(),
                &Value::String("tsconfig.root.json".to_owned())
            );
            assert_eq!(
                cfg.config.get("createMissingConfig").unwrap(),
                &Value::Bool(false)
            );
            assert_eq!(
                cfg.config.get("syncProjectReferences").unwrap(),
                &Value::Bool(true)
            );
        }

        #[test]
        fn loads_from_url() {
            let sandbox = create_empty_sandbox();
            let server = MockServer::start();

            server.mock(|when, then| {
                when.method(GET).path("/config.yml");
                then.status(200).body(SHARED_TOOLCHAIN);
            });

            let url = server.url("/config.yml");

            sandbox.create_file(
                ".moon/toolchain.yml",
                format!(
                    r"
extends: '{url}'

deno: {{}}
"
                ),
            );

            let config = test_config(sandbox.path(), |root| {
                load_config_from_root(root, &ProtoConfig::default())
            });

            assert!(config.bun.is_some());
            assert!(config.deno.is_some());
            assert!(config.node.is_some());
        }

        #[test]
        fn loads_from_url_and_saves_temp_file() {
            let sandbox = create_empty_sandbox();
            let server = MockServer::start();

            server.mock(|when, then| {
                when.method(GET).path("/config.yml");
                then.status(200).body(SHARED_TOOLCHAIN);
            });

            let temp_dir = sandbox.path().join(".moon/cache/temp");
            let url = server.url("/config.yml");

            sandbox.create_file(".moon/toolchain.yml", format!(r"extends: '{url}'"));

            assert!(!temp_dir.exists());

            test_config(sandbox.path(), |root| {
                load_config_from_root(root, &ProtoConfig::default())
            });

            assert!(temp_dir.exists());
        }
    }

    mod bun {
        use super::*;

        // #[test]
        // fn uses_defaults() {
        //     let config = test_load_config(FILENAME, "bun: {}", |path| {
        //         load_config_from_root(path, &ProtoConfig::default())
        //     });

        //     let cfg = config.bun.unwrap();

        //     assert!(cfg.plugin.is_some());
        // }

        #[test]
        fn enables_via_proto() {
            let config = test_load_config(FILENAME, "{}", |path| {
                let mut proto = ProtoConfig::default();
                proto.versions.insert(
                    Id::raw("bun"),
                    UnresolvedVersionSpec::parse("1.0.0").unwrap().into(),
                );

                load_config_from_root(path, &proto)
            });

            assert!(config.bun.is_some());
            assert_eq!(
                config.bun.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }

        #[test]
        fn inherits_plugin_locator() {
            let config = test_load_config(FILENAME, "bun: {}", |path| {
                let mut tools = ProtoConfig::default();
                tools.inherit_builtin_plugins();

                load_config_from_root(path, &tools)
            });

            assert!(config.bun.unwrap().plugin.is_some());
        }

        #[test]
        #[serial]
        fn proto_version_doesnt_override() {
            let config = test_load_config(
                FILENAME,
                r"
bun:
  version: 1.0.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("bun"),
                        UnresolvedVersionSpec::parse("2.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            assert!(config.bun.is_some());
            assert_eq!(
                config.bun.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }

        #[test]
        #[serial]
        fn inherits_version_from_env_var() {
            unsafe { env::set_var("MOON_BUN_VERSION", "1.0.0") };

            let config = test_load_config(
                FILENAME,
                r"
bun:
  version: 3.0.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("bun"),
                        UnresolvedVersionSpec::parse("2.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            unsafe { env::remove_var("MOON_BUN_VERSION") };

            assert_eq!(
                config.bun.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }

        #[test]
        fn inherits_version_from_node_pm() {
            let config = test_load_config(
                FILENAME,
                r"
bun: {}
node:
  packageManager: bun
  bun:
    version: 1.0.0
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            assert_eq!(
                config.bun.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );

            assert_eq!(
                config.node.unwrap().bun.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }

        #[test]
        fn inherits_args_from_node_pm() {
            let config = test_load_config(
                FILENAME,
                r"
bun: {}
node:
  packageManager: bun
  bun:
    version: 1.0.0
    installArgs: [--frozen]
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            assert_eq!(config.bun.unwrap().install_args, vec!["--frozen"]);

            assert_eq!(
                config.node.unwrap().bun.unwrap().install_args,
                vec!["--frozen"]
            );
        }
    }

    mod deno {
        use super::*;

        #[test]
        fn uses_defaults() {
            let config = test_load_config(FILENAME, "deno: {}", |path| {
                load_config_from_root(path, &ProtoConfig::default())
            });

            let cfg = config.deno.unwrap();

            assert_eq!(cfg.deps_file, "deps.ts".to_owned());
            assert!(!cfg.lockfile);
        }

        #[test]
        fn sets_values() {
            let config = test_load_config(
                FILENAME,
                r"
deno:
  depsFile: dependencies.ts
  lockfile: true
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            let cfg = config.deno.unwrap();

            assert_eq!(cfg.deps_file, "dependencies.ts".to_owned());
            assert!(cfg.lockfile);
        }

        #[test]
        fn can_set_bin_objects() {
            let config = test_load_config(
                FILENAME,
                r"
deno:
  bins:
    - https://deno.land/std@0.192.0/http/file_server.ts
    - bin: https://deno.land/std@0.192.0/http/file_server.ts
      name: 'fs'
      force: true
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            let cfg = config.deno.unwrap();

            assert_eq!(
                cfg.bins,
                vec![
                    BinEntry::Name("https://deno.land/std@0.192.0/http/file_server.ts".into()),
                    BinEntry::Config(BinConfig {
                        bin: "https://deno.land/std@0.192.0/http/file_server.ts".into(),
                        name: Some("fs".into()),
                        force: true,
                        ..BinConfig::default()
                    }),
                ]
            );
        }

        #[test]
        fn enables_via_proto() {
            let config = test_load_config(FILENAME, "{}", |path| {
                let mut proto = ProtoConfig::default();
                proto.versions.insert(
                    Id::raw("deno"),
                    UnresolvedVersionSpec::parse("1.30.0").unwrap().into(),
                );

                load_config_from_root(path, &proto)
            });

            assert!(config.deno.is_some());
            // assert_eq!(config.deno.unwrap().version.unwrap(), "1.30.0");
        }

        #[test]
        fn inherits_plugin_locator() {
            let config = test_load_config(FILENAME, "deno: {}", |path| {
                let mut tools = ProtoConfig::default();
                tools.inherit_builtin_plugins();

                load_config_from_root(path, &tools)
            });

            assert!(config.deno.unwrap().plugin.is_some());
        }

        #[test]
        #[serial]
        fn proto_version_doesnt_override() {
            let config = test_load_config(
                FILENAME,
                r"
deno:
  version: 1.30.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("deno"),
                        UnresolvedVersionSpec::parse("1.40.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            assert!(config.deno.is_some());
            assert_eq!(
                config.deno.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.30.0").unwrap()
            );
        }

        #[test]
        #[serial]
        fn inherits_version_from_env_var() {
            unsafe { env::set_var("MOON_DENO_VERSION", "1.20.0") };

            let config = test_load_config(
                FILENAME,
                r"
deno:
  version: 1.30.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("deno"),
                        UnresolvedVersionSpec::parse("1.40.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            unsafe { env::remove_var("MOON_DENO_VERSION") };

            assert_eq!(
                config.deno.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.20.0").unwrap()
            );
        }
    }

    mod node {
        use super::*;

        #[test]
        fn uses_defaults() {
            let config = test_load_config(FILENAME, "node: {}", |path| {
                load_config_from_root(path, &ProtoConfig::default())
            });

            let cfg = config.node.unwrap();

            assert!(cfg.dedupe_on_lockfile_change);
            assert!(!cfg.infer_tasks_from_scripts);
        }

        #[test]
        fn sets_values() {
            let config = test_load_config(
                FILENAME,
                r"
node:
  dedupeOnLockfileChange: false
  inferTasksFromScripts: true
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            let cfg = config.node.unwrap();

            assert!(!cfg.dedupe_on_lockfile_change);
            assert!(cfg.infer_tasks_from_scripts);
        }

        #[test]
        fn enables_via_proto() {
            let config = test_load_config(FILENAME, "{}", |path| {
                let mut proto = ProtoConfig::default();
                proto.versions.insert(
                    Id::raw("node"),
                    UnresolvedVersionSpec::parse("18.0.0").unwrap().into(),
                );

                load_config_from_root(path, &proto)
            });

            assert!(config.node.is_some());
            assert_eq!(
                config.node.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("18.0.0").unwrap()
            );
        }

        #[test]
        fn inherits_plugin_locator() {
            let config = test_load_config(FILENAME, "node: {}", |path| {
                let mut tools = ProtoConfig::default();
                tools.inherit_builtin_plugins();

                load_config_from_root(path, &tools)
            });

            assert!(config.node.unwrap().plugin.is_some());
        }

        #[test]
        #[serial]
        fn proto_version_doesnt_override() {
            let config = test_load_config(
                FILENAME,
                r"
node:
  version: 20.0.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("node"),
                        UnresolvedVersionSpec::parse("18.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            assert!(config.node.is_some());
            assert_eq!(
                config.node.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("20.0.0").unwrap()
            );
        }

        #[test]
        #[serial]
        fn inherits_version_from_env_var() {
            unsafe { env::set_var("MOON_NODE_VERSION", "19.0.0") };

            let config = test_load_config(
                FILENAME,
                r"
node:
  version: 20.0.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("node"),
                        UnresolvedVersionSpec::parse("18.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            unsafe { env::remove_var("MOON_NODE_VERSION") };

            assert_eq!(
                config.node.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("19.0.0").unwrap()
            );
        }

        mod npm {
            use super::*;

            #[test]
            #[serial]
            fn proto_version_doesnt_override() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  npm:
    version: 9.0.0
",
                    |path| {
                        let mut proto = ProtoConfig::default();
                        proto.versions.insert(
                            Id::raw("npm"),
                            UnresolvedVersionSpec::parse("8.0.0").unwrap().into(),
                        );

                        load_config_from_root(path, &proto)
                    },
                );

                assert_eq!(
                    config.node.unwrap().npm.version.unwrap(),
                    UnresolvedVersionSpec::parse("9.0.0").unwrap()
                );
            }

            #[test]
            fn inherits_plugin_locator() {
                let config = test_load_config(FILENAME, "node:\n  npm: {}", |path| {
                    let mut tools = ProtoConfig::default();
                    tools.inherit_builtin_plugins();

                    load_config_from_root(path, &tools)
                });

                assert!(config.node.unwrap().npm.plugin.is_some());
            }

            #[test]
            #[serial]
            fn inherits_version_from_env_var() {
                unsafe { env::set_var("MOON_NPM_VERSION", "10.0.0") };

                let config = test_load_config(
                    FILENAME,
                    r"
node:
  npm:
    version: 9.0.0
",
                    |path| {
                        let mut proto = ProtoConfig::default();
                        proto.versions.insert(
                            Id::raw("npm"),
                            UnresolvedVersionSpec::parse("8.0.0").unwrap().into(),
                        );

                        load_config_from_root(path, &proto)
                    },
                );

                unsafe { env::remove_var("MOON_NPM_VERSION") };

                assert_eq!(
                    config.node.unwrap().npm.version.unwrap(),
                    UnresolvedVersionSpec::parse("10.0.0").unwrap()
                );
            }

            #[test]
            fn fallsback_version_format() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  packageManager: npm
  dependencyVersionFormat: workspace
",
                    |path| load_config_from_root(path, &ProtoConfig::default()),
                );

                let cfg = config.node.unwrap();

                assert_eq!(cfg.dependency_version_format, NodeVersionFormat::File);
            }
        }

        mod pnpm {
            use super::*;

            #[test]
            fn enables_when_defined() {
                let config = test_load_config(FILENAME, "node: {}", |path| {
                    load_config_from_root(path, &ProtoConfig::default())
                });

                assert!(config.node.unwrap().pnpm.is_none());

                let config = test_load_config(FILENAME, "node:\n  pnpm: {}", |path| {
                    load_config_from_root(path, &ProtoConfig::default())
                });

                assert!(config.node.unwrap().pnpm.is_some());
            }

            #[test]
            fn inherits_plugin_locator() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  packageManager: pnpm
  pnpm: {}
",
                    |path| {
                        let mut tools = ProtoConfig::default();
                        tools.inherit_builtin_plugins();

                        load_config_from_root(path, &tools)
                    },
                );

                assert!(config.node.unwrap().pnpm.unwrap().plugin.is_some());
            }

            #[test]
            fn inherits_plugin_locator_when_none() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  packageManager: pnpm
",
                    |path| {
                        let mut tools = ProtoConfig::default();
                        tools.inherit_builtin_plugins();

                        load_config_from_root(path, &tools)
                    },
                );

                assert!(config.node.unwrap().pnpm.unwrap().plugin.is_some());
            }

            #[test]
            #[serial]
            fn proto_version_doesnt_override() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  pnpm:
    version: 9.0.0
",
                    |path| {
                        let mut proto = ProtoConfig::default();
                        proto.versions.insert(
                            Id::raw("pnpm"),
                            UnresolvedVersionSpec::parse("8.0.0").unwrap().into(),
                        );

                        load_config_from_root(path, &proto)
                    },
                );

                assert_eq!(
                    config.node.unwrap().pnpm.unwrap().version.unwrap(),
                    UnresolvedVersionSpec::parse("9.0.0").unwrap()
                );
            }

            #[test]
            #[serial]
            fn inherits_version_from_env_var() {
                unsafe { env::set_var("MOON_PNPM_VERSION", "10.0.0") };

                let config = test_load_config(
                    FILENAME,
                    r"
node:
  pnpm:
    version: 9.0.0
",
                    |path| {
                        let mut proto = ProtoConfig::default();
                        proto.versions.insert(
                            Id::raw("pnpm"),
                            UnresolvedVersionSpec::parse("8.0.0").unwrap().into(),
                        );

                        load_config_from_root(path, &proto)
                    },
                );

                unsafe { env::remove_var("MOON_PNPM_VERSION") };

                assert_eq!(
                    config.node.unwrap().pnpm.unwrap().version.unwrap(),
                    UnresolvedVersionSpec::parse("10.0.0").unwrap()
                );
            }
        }

        mod yarn {
            use super::*;

            #[test]
            fn enables_when_defined() {
                let config = test_load_config(FILENAME, "node: {}", |path| {
                    load_config_from_root(path, &ProtoConfig::default())
                });

                assert!(config.node.unwrap().yarn.is_none());

                let config = test_load_config(FILENAME, "node:\n  yarn: {}", |path| {
                    load_config_from_root(path, &ProtoConfig::default())
                });

                assert!(config.node.unwrap().yarn.is_some());
            }

            #[test]
            fn inherits_plugin_locator() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  packageManager: yarn
  yarn: {}
",
                    |path| {
                        let mut tools = ProtoConfig::default();
                        tools.inherit_builtin_plugins();

                        load_config_from_root(path, &tools)
                    },
                );

                assert!(config.node.unwrap().yarn.unwrap().plugin.is_some());
            }

            #[test]
            fn inherits_plugin_locator_when_none() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  packageManager: yarn
",
                    |path| {
                        let mut tools = ProtoConfig::default();
                        tools.inherit_builtin_plugins();

                        load_config_from_root(path, &tools)
                    },
                );

                assert!(config.node.unwrap().yarn.unwrap().plugin.is_some());
            }

            #[test]
            #[serial]
            fn proto_version_doesnt_override() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  yarn:
    version: 9.0.0
",
                    |path| {
                        let mut proto = ProtoConfig::default();
                        proto.versions.insert(
                            Id::raw("yarn"),
                            UnresolvedVersionSpec::parse("8.0.0").unwrap().into(),
                        );

                        load_config_from_root(path, &proto)
                    },
                );

                assert_eq!(
                    config.node.unwrap().yarn.unwrap().version.unwrap(),
                    UnresolvedVersionSpec::parse("9.0.0").unwrap()
                );
            }

            #[test]
            #[serial]
            fn inherits_version_from_env_var() {
                unsafe { env::set_var("MOON_YARN_VERSION", "10.0.0") };

                let config = test_load_config(
                    FILENAME,
                    r"
node:
  yarn:
    version: 9.0.0
",
                    |path| {
                        let mut proto = ProtoConfig::default();
                        proto.versions.insert(
                            Id::raw("yarn"),
                            UnresolvedVersionSpec::parse("8.0.0").unwrap().into(),
                        );

                        load_config_from_root(path, &proto)
                    },
                );

                unsafe { env::remove_var("MOON_YARN_VERSION") };

                assert_eq!(
                    config.node.unwrap().yarn.unwrap().version.unwrap(),
                    UnresolvedVersionSpec::parse("10.0.0").unwrap()
                );
            }
        }

        mod bun {
            use super::*;

            #[test]
            fn enables_when_defined() {
                let config = test_load_config(FILENAME, "node: {}", |path| {
                    load_config_from_root(path, &ProtoConfig::default())
                });

                assert!(config.node.unwrap().bun.is_none());

                let config = test_load_config(FILENAME, "node:\n  bun: {}", |path| {
                    load_config_from_root(path, &ProtoConfig::default())
                });

                assert!(config.node.unwrap().bun.is_some());
            }

            #[test]
            fn inherits_plugin_locator() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  packageManager: bun
  bun: {}
",
                    |path| {
                        let mut tools = ProtoConfig::default();
                        tools.inherit_builtin_plugins();

                        load_config_from_root(path, &tools)
                    },
                );

                assert!(config.node.unwrap().bun.unwrap().plugin.is_some());
            }

            #[test]
            fn inherits_plugin_locator_when_none() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  packageManager: bun
",
                    |path| {
                        let mut tools = ProtoConfig::default();
                        tools.inherit_builtin_plugins();

                        load_config_from_root(path, &tools)
                    },
                );

                assert!(config.node.unwrap().bun.unwrap().plugin.is_some());
            }

            #[test]
            #[serial]
            fn proto_version_doesnt_override() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  bun:
    version: 1.0.0
",
                    |path| {
                        let mut proto = ProtoConfig::default();
                        proto.versions.insert(
                            Id::raw("bun"),
                            UnresolvedVersionSpec::parse("0.0.1").unwrap().into(),
                        );

                        load_config_from_root(path, &proto)
                    },
                );

                assert_eq!(
                    config.node.unwrap().bun.unwrap().version.unwrap(),
                    UnresolvedVersionSpec::parse("1.0.0").unwrap()
                );
            }

            #[test]
            #[serial]
            fn inherits_version_from_env_var() {
                unsafe { env::set_var("MOON_BUN_VERSION", "1.0.0") };

                let config = test_load_config(
                    FILENAME,
                    r"
node:
  bun:
    version: 0.0.1
",
                    |path| {
                        let mut proto = ProtoConfig::default();
                        proto.versions.insert(
                            Id::raw("bun"),
                            UnresolvedVersionSpec::parse("0.1.0").unwrap().into(),
                        );

                        load_config_from_root(path, &proto)
                    },
                );

                unsafe { env::remove_var("MOON_BUN_VERSION") };

                assert_eq!(
                    config.node.unwrap().bun.unwrap().version.unwrap(),
                    UnresolvedVersionSpec::parse("1.0.0").unwrap()
                );
            }

            #[test]
            #[serial]
            fn inherits_version_from_bun_tool() {
                let config = test_load_config(
                    FILENAME,
                    r"
bun:
  version: 1.0.0
node:
  packageManager: bun
",
                    |path| load_config_from_root(path, &ProtoConfig::default()),
                );

                assert_eq!(
                    config.bun.unwrap().version.unwrap(),
                    UnresolvedVersionSpec::parse("1.0.0").unwrap()
                );

                assert_eq!(
                    config.node.unwrap().bun.unwrap().version.unwrap(),
                    UnresolvedVersionSpec::parse("1.0.0").unwrap()
                );
            }

            #[test]
            #[serial]
            fn inherits_args_from_bun_tool() {
                let config = test_load_config(
                    FILENAME,
                    r"
bun:
  version: 1.0.0
  installArgs:
    - --frozen
node:
  packageManager: bun
",
                    |path| load_config_from_root(path, &ProtoConfig::default()),
                );

                assert_eq!(config.bun.unwrap().install_args, vec!["--frozen"]);

                assert_eq!(
                    config.node.unwrap().bun.unwrap().install_args,
                    vec!["--frozen"]
                );
            }

            #[test]
            fn fallsback_version_format() {
                let config = test_load_config(
                    FILENAME,
                    r"
node:
  packageManager: bun
  dependencyVersionFormat: workspace-tilde
",
                    |path| load_config_from_root(path, &ProtoConfig::default()),
                );

                let cfg = config.node.unwrap();

                assert_eq!(cfg.dependency_version_format, NodeVersionFormat::Workspace);
            }
        }
    }

    mod python {
        use super::*;

        #[test]
        fn enables_via_proto() {
            let config = test_load_config(FILENAME, "{}", |path| {
                let mut proto = ProtoConfig::default();
                proto.versions.insert(
                    Id::raw("python"),
                    UnresolvedVersionSpec::parse("1.0.0").unwrap().into(),
                );

                load_config_from_root(path, &proto)
            });

            assert!(config.python.is_some());
            assert_eq!(
                config.python.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }

        #[test]
        fn inherits_plugin_locator() {
            let config = test_load_config(FILENAME, "python: {}", |path| {
                let mut tools = ProtoConfig::default();
                tools.inherit_builtin_plugins();

                load_config_from_root(path, &tools)
            });

            assert!(config.python.unwrap().plugin.is_some());
        }

        #[test]
        #[serial]
        fn proto_version_doesnt_override() {
            let config = test_load_config(
                FILENAME,
                r"
python:
  version: 1.0.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("python"),
                        UnresolvedVersionSpec::parse("2.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            assert!(config.python.is_some());
            assert_eq!(
                config.python.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }

        #[test]
        #[serial]
        fn inherits_version_from_env_var() {
            unsafe { env::set_var("MOON_PYTHON_VERSION", "1.0.0") };

            let config = test_load_config(
                FILENAME,
                r"
python:
  version: 3.0.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("python"),
                        UnresolvedVersionSpec::parse("2.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            unsafe { env::remove_var("MOON_PYTHON_VERSION") };

            assert_eq!(
                config.python.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }
    }

    mod rust {
        use super::*;

        #[test]
        fn uses_defaults() {
            let config = test_load_config(FILENAME, "rust: {}", |path| {
                load_config_from_root(path, &ProtoConfig::default())
            });

            let cfg = config.rust.unwrap();

            assert!(cfg.bins.is_empty());
            assert!(!cfg.sync_toolchain_config);
        }

        #[test]
        fn sets_values() {
            let config = test_load_config(
                FILENAME,
                r"
rust:
  bins: [cargo-make]
  syncToolchainConfig: true
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            let cfg = config.rust.unwrap();

            assert_eq!(cfg.bins, vec![BinEntry::Name("cargo-make".into())]);
            assert!(cfg.sync_toolchain_config);
        }

        #[test]
        fn can_set_bin_objects() {
            let config = test_load_config(
                FILENAME,
                r"
rust:
  bins:
    - cargo-make
    - bin: cargo-nextest
      name: 'next'
    - bin: cargo-insta
      local: true
  syncToolchainConfig: true
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            let cfg = config.rust.unwrap();

            assert_eq!(
                cfg.bins,
                vec![
                    BinEntry::Name("cargo-make".into()),
                    BinEntry::Config(BinConfig {
                        bin: "cargo-nextest".into(),
                        name: Some("next".into()),
                        ..BinConfig::default()
                    }),
                    BinEntry::Config(BinConfig {
                        bin: "cargo-insta".into(),
                        local: true,
                        ..BinConfig::default()
                    }),
                ]
            );
            assert!(cfg.sync_toolchain_config);
        }

        #[test]
        fn enables_via_proto() {
            let config = test_load_config(FILENAME, "{}", |path| {
                let mut proto = ProtoConfig::default();
                proto.versions.insert(
                    Id::raw("rust"),
                    UnresolvedVersionSpec::parse("1.69.0").unwrap().into(),
                );

                load_config_from_root(path, &proto)
            });

            assert!(config.rust.is_some());
            assert_eq!(
                config.rust.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.69.0").unwrap()
            );
        }

        #[test]
        fn inherits_plugin_locator() {
            let config = test_load_config(FILENAME, "rust: {}", |path| {
                let mut tools = ProtoConfig::default();
                tools.inherit_builtin_plugins();

                load_config_from_root(path, &tools)
            });

            assert!(config.rust.unwrap().plugin.is_some());
        }

        #[test]
        #[serial]
        fn proto_version_doesnt_override() {
            let config = test_load_config(
                FILENAME,
                r"
rust:
  version: 1.60.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("rust"),
                        UnresolvedVersionSpec::parse("1.69.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            assert!(config.rust.is_some());
            assert_eq!(
                config.rust.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.60.0").unwrap()
            );
        }

        #[test]
        #[serial]
        fn inherits_version_from_env_var() {
            unsafe { env::set_var("MOON_RUST_VERSION", "1.70.0") };

            let config = test_load_config(
                FILENAME,
                r"
rust:
  version: 1.60.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("rust"),
                        UnresolvedVersionSpec::parse("1.65.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            unsafe { env::remove_var("MOON_RUST_VERSION") };

            assert_eq!(
                config.rust.unwrap().version.unwrap(),
                UnresolvedVersionSpec::parse("1.70.0").unwrap()
            );
        }
    }

    mod typescript {
        use super::*;

        #[test]
        fn sets_values() {
            let config = test_load_config(
                FILENAME,
                r"
typescript:
  projectConfigFileName: tsconf.json
  syncProjectReferences: false
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            let cfg = config.plugins.get("typescript").unwrap();

            assert_eq!(
                cfg.config.get("projectConfigFileName").unwrap(),
                &Value::String("tsconf.json".to_owned())
            );
            assert_eq!(
                cfg.config.get("syncProjectReferences").unwrap(),
                &Value::Bool(false)
            );
        }
    }

    mod plugin {
        use super::*;

        #[test]
        fn uses_defaults() {
            let config = test_load_config(
                FILENAME,
                r"
plugin:
  plugin: file://example.wasm
",
                |path| load_config_from_root(path, &ProtoConfig::default()),
            );

            let cfg = config.plugins.get("plugin").unwrap();

            assert_eq!(
                cfg,
                &ToolchainPluginConfig {
                    plugin: Some(PluginLocator::File(Box::new(FileLocator {
                        file: "file://example.wasm".into(),
                        path: None
                    }))),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn inherits_proto_version() {
            let config = test_load_config(
                FILENAME,
                r"
plugin:
  plugin: file://example.wasm
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("plugin"),
                        UnresolvedVersionSpec::parse("1.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            let cfg = config.plugins.get("plugin").unwrap();

            assert_eq!(
                cfg.version.as_ref().unwrap(),
                &UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }

        #[test]
        fn inherits_proto_version_with_different_id() {
            let config = test_load_config(
                FILENAME,
                r"
plugin:
  plugin: file://example.wasm
  versionFromPrototools: 'plugin-other'
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("plugin-other"),
                        UnresolvedVersionSpec::parse("1.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            let cfg = config.plugins.get("plugin").unwrap();

            assert_eq!(
                cfg.version.as_ref().unwrap(),
                &UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }

        #[test]
        fn doesnt_inherit_proto_version_when_disabled() {
            let config = test_load_config(
                FILENAME,
                r"
plugin:
  plugin: file://example.wasm
  versionFromPrototools: false
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("plugin"),
                        UnresolvedVersionSpec::parse("1.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            let cfg = config.plugins.get("plugin").unwrap();

            assert!(cfg.version.is_none());
        }

        #[test]
        #[serial]
        fn proto_version_doesnt_override() {
            let config = test_load_config(
                FILENAME,
                r"
plugin:
  plugin: file://example.wasm
  version: 1.0.0
",
                |path| {
                    let mut proto = ProtoConfig::default();
                    proto.versions.insert(
                        Id::raw("plugin"),
                        UnresolvedVersionSpec::parse("2.0.0").unwrap().into(),
                    );

                    load_config_from_root(path, &proto)
                },
            );

            let cfg = config.plugins.get("plugin").unwrap();

            assert_eq!(
                cfg.version.as_ref().unwrap(),
                &UnresolvedVersionSpec::parse("1.0.0").unwrap()
            );
        }
    }

    mod pkl {
        use super::*;
        use moon_config::*;
        use starbase_sandbox::locate_fixture;
        use std::collections::BTreeMap;

        #[test]
        fn loads_pkl() {
            let mut config = test_config(locate_fixture("pkl"), |path| {
                let proto = proto_core::ProtoConfig::default();
                ConfigLoader::default().load_toolchain_config(path, &proto)
            });

            assert_eq!(
                config.node.take().unwrap(),
                NodeConfig {
                    add_engines_constraint: false,
                    bin_exec_args: vec!["--profile".into()],
                    bun: None,
                    dedupe_on_lockfile_change: false,
                    dependency_version_format: NodeVersionFormat::WorkspaceCaret,
                    infer_tasks_from_scripts: true,
                    npm: NpmConfig::default(),
                    package_manager: NodePackageManager::Yarn,
                    packages_root: ".".into(),
                    plugin: None,
                    pnpm: None,
                    root_package_only: true,
                    sync_package_manager_field: false,
                    sync_project_workspace_dependencies: false,
                    sync_version_manager_config: Some(NodeVersionManager::Nvm),
                    version: Some(UnresolvedVersionSpec::parse("20.12").unwrap()),
                    yarn: Some(YarnConfig {
                        install_args: vec!["--immutable".into()],
                        plugin: None,
                        plugins: vec![],
                        version: Some(UnresolvedVersionSpec::parse("4").unwrap())
                    })
                }
            );
            assert_eq!(
                config.plugins.get("typescript").unwrap().config,
                BTreeMap::from_iter([
                    ("createMissingConfig".into(), Value::Bool(false)),
                    ("includeProjectReferenceSources".into(), Value::Bool(true)),
                    ("includeSharedTypes".into(), Value::Bool(true)),
                    (
                        "projectConfigFileName".into(),
                        Value::String("tsconfig.app.json".into())
                    ),
                    (
                        "rootConfigFileName".into(),
                        Value::String("tsconfig.root.json".into())
                    ),
                    (
                        "rootOptionsConfigFileName".into(),
                        Value::String("tsconfig.opts.json".into())
                    ),
                    ("routeOutDirToCache".into(), Value::Bool(true)),
                    ("syncProjectReferences".into(), Value::Bool(false)),
                    ("syncProjectReferencesToPaths".into(), Value::Bool(true)),
                ])
            );
        }
    }
}
