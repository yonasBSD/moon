use super::syncs::codeowners::SyncCodeownersArgs;
use super::syncs::config_schemas::SyncConfigSchemasArgs;
use super::syncs::hooks::SyncHooksArgs;
use crate::components::run_action_pipeline;
use crate::session::CliSession;
use clap::Subcommand;
use iocraft::prelude::element;
use moon_action_graph::ActionGraphBuilderOptions;
use moon_console::ui::{Container, Notice, StyledText, Variant};
use starbase::AppResult;

#[derive(Clone, Debug, Subcommand)]
pub enum SyncCommands {
    #[command(
        name = "codeowners",
        about = "Aggregate and sync code owners to a `CODEOWNERS` file."
    )]
    Codeowners(SyncCodeownersArgs),

    #[command(
        name = "config-schemas",
        about = "Generate and sync configuration JSON schemas for use within editors."
    )]
    ConfigSchemas(SyncConfigSchemasArgs),

    #[command(
        name = "hooks",
        about = "Generate and sync hook scripts for the workspace configured VCS."
    )]
    Hooks(SyncHooksArgs),

    #[command(
        name = "projects",
        about = "Sync all projects and configs in the workspace."
    )]
    Projects,
}

pub async fn sync(session: CliSession) -> AppResult {
    let workspace_graph = session.get_workspace_graph().await?;
    let mut action_graph_builder = session
        .build_action_graph_with_options(&workspace_graph, ActionGraphBuilderOptions::default())
        .await?;

    action_graph_builder.sync_workspace();

    for project in workspace_graph.projects.get_all_unexpanded() {
        action_graph_builder.sync_project(project)?;
    }

    run_action_pipeline(
        &session,
        action_graph_builder.build_context(),
        action_graph_builder.build(),
    )
    .await?;

    session.console.render(element! {
        Container {
            Notice(variant: Variant::Success) {
                StyledText(content: format!("Synced the workspace and all projects"))
            }
        }
    })?;

    Ok(None)
}
