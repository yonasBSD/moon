use crate::actions;
use crate::infer_tasks_from_scripts;
use miette::IntoDiagnostic;
use moon_action::Operation;
use moon_action_context::ActionContext;
use moon_common::Id;
use moon_common::path::WorkspaceRelativePath;
use moon_common::path::WorkspaceRelativePathBuf;
use moon_common::path::is_root_level_source;
use moon_config::{
    DependencyConfig, DependencyScope, DependencySource, HasherConfig, NodeConfig,
    NodePackageManager, PlatformType, ProjectConfig, ProjectsAliasesList, ProjectsSourcesList,
    TaskConfig, TasksConfigsMap, UnresolvedVersionSpec,
};
use moon_console::Console;
use moon_hash::{ContentHasher, DepsHash};
use moon_logger::debug;
use moon_node_lang::PackageJsonCache;
use moon_node_lang::node::{find_package_manager_workspaces_root, get_package_manager_workspaces};
use moon_node_tool::NodeTool;
use moon_node_tool::get_node_env_paths;
use moon_platform::{Platform, Runtime, RuntimeReq};
use moon_process::Command;
use moon_project::Project;
use moon_task::Task;
use moon_tool::get_proto_version_env;
use moon_tool::prepend_path_env_var;
use moon_tool::{Tool, ToolManager};
use moon_utils::{async_trait, path};
use proto_core::ProtoEnvironment;
use rustc_hash::FxHashMap;
use schematic::Config;
use starbase_styles::color;
use starbase_utils::glob::GlobSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::{collections::BTreeMap, path::Path};
use tracing::instrument;

const LOG_TARGET: &str = "moon:node-platform";

pub struct NodePlatform {
    config: NodeConfig,

    console: Arc<Console>,

    package_names: FxHashMap<String, Id>,

    packages_root: PathBuf,

    proto_env: Arc<ProtoEnvironment>,

    toolchain: ToolManager<NodeTool>,

    workspace_root: PathBuf,
}

impl NodePlatform {
    pub fn new(
        config: &NodeConfig,
        workspace_root: &Path,
        proto_env: Arc<ProtoEnvironment>,
        console: Arc<Console>,
    ) -> Self {
        NodePlatform {
            packages_root: path::normalize(workspace_root.join(&config.packages_root)),
            config: config.to_owned(),
            package_names: FxHashMap::default(),
            proto_env,
            toolchain: ToolManager::new(Runtime::new(Id::raw("node"), RuntimeReq::Global)),
            workspace_root: workspace_root.to_path_buf(),
            console,
        }
    }
}

#[async_trait]
impl Platform for NodePlatform {
    fn get_type(&self) -> PlatformType {
        PlatformType::Node
    }

    fn get_runtime_from_config(&self, project_config: Option<&ProjectConfig>) -> Runtime {
        if let Some(config) = &project_config {
            if let Some(node_config) = &config.toolchain.node {
                if let Some(version) = &node_config.version {
                    return Runtime::new_override(
                        Id::raw("node"),
                        RuntimeReq::Toolchain(version.to_owned()),
                    );
                }
            }
        }

        if let Some(version) = &self.config.version {
            return Runtime::new(Id::raw("node"), RuntimeReq::Toolchain(version.to_owned()));
        }

        // Global
        Runtime::new(Id::raw("node"), RuntimeReq::Global)
    }

    fn matches(&self, platform: &PlatformType, runtime: Option<&Runtime>) -> bool {
        if matches!(platform, PlatformType::Node) {
            return true;
        }

        if let Some(runtime) = &runtime {
            return runtime.toolchain == "node";
        }

        false
    }

    // PROJECT GRAPH

    fn find_dependency_workspace_root(
        &self,
        starting_dir: &str,
    ) -> miette::Result<WorkspaceRelativePathBuf> {
        let root = find_package_manager_workspaces_root(
            self.workspace_root.join(starting_dir),
            matches!(self.config.package_manager, NodePackageManager::Pnpm),
        )?
        .unwrap_or(self.packages_root.clone());

        if let Ok(root) = root.strip_prefix(&self.workspace_root) {
            return WorkspaceRelativePathBuf::from_path(root).into_diagnostic();
        }

        Ok(WorkspaceRelativePathBuf::default())
    }

    fn is_project_in_dependency_workspace(
        &self,
        deps_root: &WorkspaceRelativePath,
        project_source: &str,
    ) -> miette::Result<bool> {
        let mut in_workspace = false;
        let deps_root_path = deps_root.to_logical_path(&self.workspace_root);

        // Single version policy / only a root package.json
        if self.config.root_package_only {
            return Ok(true);
        }

        // Root package is always considered within the workspace
        if is_root_level_source(project_source) && deps_root_path == self.workspace_root {
            return Ok(true);
        }

        if let Some(globs) = get_package_manager_workspaces(
            deps_root_path,
            matches!(self.config.package_manager, NodePackageManager::Pnpm),
        )? {
            in_workspace = GlobSet::new(&globs)?.matches(project_source);
        }

        Ok(in_workspace)
    }

    #[instrument(skip_all)]
    fn load_project_graph_aliases(
        &mut self,
        projects_list: &ProjectsSourcesList,
        aliases_list: &mut ProjectsAliasesList,
    ) -> miette::Result<()> {
        debug!(
            target: LOG_TARGET,
            "Loading names (aliases) from project {}'s",
            color::file("package.json")
        );

        for (project_id, project_source) in projects_list {
            if let Some(package_json) =
                PackageJsonCache::read(project_source.to_path(&self.workspace_root))?
            {
                if let Some(package_name) = package_json.data.name {
                    self.package_names
                        .insert(package_name.clone(), project_id.to_owned());

                    if package_name != project_id.as_str() {
                        aliases_list.push((project_id.to_owned(), package_name.clone()));
                    }
                }
            }
        }

        Ok(())
    }

    #[instrument(skip(self))]
    fn load_project_implicit_dependencies(
        &self,
        project_id: &str,
        project_source: &str,
    ) -> miette::Result<Vec<DependencyConfig>> {
        let mut implicit_deps = vec![];

        debug!(
            target: LOG_TARGET,
            "Scanning {} for implicit dependency relations",
            color::id(project_id),
        );

        if let Some(package_json) =
            PackageJsonCache::read(self.workspace_root.join(project_source))?
        {
            let mut find_implicit_relations =
                |package_deps: &BTreeMap<String, String>, scope: &DependencyScope| {
                    for dep_name in package_deps.keys() {
                        if let Some(dep_project_id) = self.package_names.get(dep_name) {
                            implicit_deps.push(DependencyConfig {
                                id: dep_project_id.to_owned(),
                                scope: *scope,
                                source: DependencySource::Implicit,
                                via: Some(format!("package {dep_name}")),
                            });
                        }
                    }
                };

            if let Some(dependencies) = &package_json.data.dependencies {
                find_implicit_relations(dependencies, &DependencyScope::Production);
            }

            if let Some(dev_dependencies) = &package_json.data.dev_dependencies {
                find_implicit_relations(dev_dependencies, &DependencyScope::Development);
            }

            if let Some(peer_dependencies) = &package_json.data.peer_dependencies {
                find_implicit_relations(peer_dependencies, &DependencyScope::Peer);
            }

            if let Some(optional_dependencies) = &package_json.data.optional_dependencies {
                find_implicit_relations(optional_dependencies, &DependencyScope::Build);
            }
        }

        Ok(implicit_deps)
    }

    #[instrument(skip(self))]
    fn load_project_tasks(
        &self,
        project_id: &str,
        project_source: &str,
    ) -> miette::Result<TasksConfigsMap> {
        let mut tasks = BTreeMap::new();

        if !self.config.infer_tasks_from_scripts {
            return Ok(tasks);
        }

        debug!(
            target: LOG_TARGET,
            "Inferring {} tasks from {}",
            color::id(project_id),
            color::file("package.json")
        );

        if let Some(package_json) =
            PackageJsonCache::read(self.workspace_root.join(project_source))?
        {
            for (id, partial_task) in
                infer_tasks_from_scripts(project_id, &package_json, self.config.package_manager)?
            {
                tasks.insert(id, TaskConfig::from_partial(partial_task));
            }
        }

        Ok(tasks)
    }

    // TOOLCHAIN

    fn is_toolchain_enabled(&self) -> miette::Result<bool> {
        Ok(self.config.version.is_some())
    }

    fn get_tool(&self) -> miette::Result<Box<&dyn Tool>> {
        let tool = self.toolchain.get()?;

        Ok(Box::new(tool))
    }

    fn get_tool_for_version(&self, req: RuntimeReq) -> miette::Result<Box<&dyn Tool>> {
        let tool = self.toolchain.get_for_version(&req)?;

        Ok(Box::new(tool))
    }

    fn get_dependency_configs(&self) -> miette::Result<Option<(String, String)>> {
        let tool = self.toolchain.get()?;
        let depman = tool.get_package_manager();

        Ok(Some((
            depman.get_lock_filename(),
            depman.get_manifest_filename(),
        )))
    }

    async fn setup_toolchain(&mut self) -> miette::Result<()> {
        let req = match &self.config.version {
            Some(v) => RuntimeReq::Toolchain(v.to_owned()),
            None => RuntimeReq::Global,
        };

        let mut last_versions = FxHashMap::default();

        if !self.toolchain.has(&req) {
            self.toolchain.register(
                &req,
                NodeTool::new(
                    Arc::clone(&self.proto_env),
                    Arc::clone(&self.console),
                    &self.config,
                    &req,
                )
                .await?,
            );
        }

        self.toolchain.setup(&req, &mut last_versions).await?;

        Ok(())
    }

    async fn teardown_toolchain(&mut self) -> miette::Result<()> {
        self.toolchain.teardown_all().await?;

        Ok(())
    }

    // ACTIONS

    #[instrument(skip_all)]
    async fn setup_tool(
        &mut self,
        _context: &ActionContext,
        runtime: &Runtime,
        last_versions: &mut FxHashMap<String, UnresolvedVersionSpec>,
    ) -> miette::Result<u8> {
        let req = &runtime.requirement;

        if !self.toolchain.has(req) {
            self.toolchain.register(
                req,
                NodeTool::new(
                    Arc::clone(&self.proto_env),
                    Arc::clone(&self.console),
                    &self.config,
                    req,
                )
                .await?,
            );
        }

        let installed = self.toolchain.setup(req, last_versions).await?;

        actions::setup_tool(self.toolchain.get_for_version(req)?, &self.workspace_root).await?;

        Ok(installed)
    }

    #[instrument(skip_all)]
    async fn install_deps(
        &self,
        _context: &ActionContext,
        runtime: &Runtime,
        working_dir: &Path,
    ) -> miette::Result<Vec<Operation>> {
        actions::install_deps(
            self.toolchain.get_for_version(&runtime.requirement)?,
            working_dir,
            &self.console,
        )
        .await
    }

    #[instrument(skip_all)]
    async fn sync_project(
        &self,
        _context: &ActionContext,
        project: &Project,
        dependencies: &FxHashMap<Id, Arc<Project>>,
    ) -> miette::Result<bool> {
        let modified = actions::sync_project(project, dependencies, &self.config).await?;

        Ok(modified)
    }

    #[instrument(skip_all)]
    async fn hash_manifest_deps(
        &self,
        manifest_path: &Path,
        hasher: &mut ContentHasher,
        _hasher_config: &HasherConfig,
    ) -> miette::Result<()> {
        if let Ok(Some(package)) = PackageJsonCache::read(manifest_path) {
            let name = package.data.name.unwrap_or_else(|| "unknown".into());
            let mut hash = DepsHash::new(name);

            if let Some(optional_deps) = &package.data.optional_dependencies {
                hash.add_deps(optional_deps);
            }

            if let Some(peer_deps) = &package.data.peer_dependencies {
                hash.add_deps(peer_deps);
            }

            if let Some(dev_deps) = &package.data.dev_dependencies {
                hash.add_deps(dev_deps);
            }

            if let Some(deps) = &package.data.dependencies {
                hash.add_deps(deps);
            }

            hasher.hash_content(hash)?;
        }

        Ok(())
    }

    #[instrument(skip_all)]
    async fn hash_run_target(
        &self,
        project: &Project,
        runtime: &Runtime,
        hasher: &mut ContentHasher,
        hasher_config: &HasherConfig,
    ) -> miette::Result<()> {
        let node_hash = actions::create_target_hasher(
            self.toolchain.get_for_version(&runtime.requirement).ok(),
            project,
            &self.workspace_root,
            hasher_config,
        )
        .await?;

        hasher.hash_content(node_hash)?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn create_run_target_command(
        &self,
        context: &ActionContext,
        project: &Project,
        task: &Task,
        runtime: &Runtime,
        _working_dir: &Path,
    ) -> miette::Result<Command> {
        let mut command = actions::create_target_command_without_tool(
            &self.config,
            context,
            project,
            task,
            &self.workspace_root,
        )?;

        if let Ok(node) = self.toolchain.get_for_version(&runtime.requirement) {
            if let Some(version) = get_proto_version_env(&node.tool) {
                command.env("PROTO_NODE_VERSION", version);
            }

            if let Ok(npm) = node.get_npm() {
                if let Some(version) = get_proto_version_env(&npm.tool) {
                    command.env("PROTO_NPM_VERSION", version);
                }
            }

            if let Ok(pnpm) = node.get_pnpm() {
                if let Some(version) = get_proto_version_env(&pnpm.tool) {
                    command.env("PROTO_PNPM_VERSION", version);
                }
            }

            if let Ok(yarn) = node.get_yarn() {
                if let Some(version) = get_proto_version_env(&yarn.tool) {
                    command.env("PROTO_YARN_VERSION", version);
                }
            }

            if let Ok(bun) = node.get_bun() {
                if let Some(version) = get_proto_version_env(&bun.tool) {
                    command.env("PROTO_BUN_VERSION", version);
                }
            }
        }

        let mut paths = vec![];
        let mut current_dir = project.root.as_path();

        loop {
            paths.push(current_dir.join("node_modules").join(".bin"));

            if current_dir == self.workspace_root {
                break;
            }

            match current_dir.parent() {
                Some(dir) => {
                    current_dir = dir;
                }
                None => break,
            };
        }

        if !runtime.requirement.is_global() {
            paths.extend(get_node_env_paths(&self.proto_env));
        }

        command.env("PATH", prepend_path_env_var(paths));

        Ok(command)
    }
}
