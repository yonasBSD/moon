use crate::operations::*;
use crate::plugins::*;
use crate::utils::should_skip_action;
use miette::IntoDiagnostic;
use moon_action::{Action, ActionStatus, Operation};
use moon_action_context::ActionContext;
use moon_app_context::AppContext;
use moon_common::color;
use moon_pdk_api::SyncWorkspaceInput;
use moon_remote::RemoteService;
use moon_toolchain_plugin::ToolchainRegistry;
use moon_workspace_graph::WorkspaceGraph;
use std::sync::Arc;
use tokio::task;
use tracing::{debug, instrument};

#[instrument(skip_all)]
pub async fn sync_workspace(
    action: &mut Action,
    _action_context: Arc<ActionContext>,
    app_context: Arc<AppContext>,
    workspace_graph: Arc<WorkspaceGraph>,
    toolchain_registry: Arc<ToolchainRegistry>,
) -> miette::Result<ActionStatus> {
    let _lock = app_context.cache_engine.create_lock(action.get_prefix())?;

    // Connect to the remote service in this action,
    // as it always runs before tasks, and we don't need it
    // for non-pipeline related features!
    if app_context.workspace_config.remote.is_enabled() {
        RemoteService::connect(
            &app_context.workspace_config.remote,
            &app_context.workspace_root,
        )
        .await?;
    }

    if should_skip_action("MOON_SKIP_SYNC_WORKSPACE").is_some() {
        debug!(
            "Skipping workspace sync because {} is set",
            color::symbol("MOON_SKIP_SYNC_WORKSPACE")
        );

        return Ok(ActionStatus::Skipped);
    }

    debug!("Syncing workspace");

    // Run operations in parallel
    let mut operation_futures: Vec<task::JoinHandle<miette::Result<Vec<Operation>>>> = vec![];

    {
        debug!("Syncing config schemas");

        let app_context = Arc::clone(&app_context);

        operation_futures.push(task::spawn(async move {
            Ok(vec![
                Operation::sync_operation("config-schemas")?
                    .track_async_with_check(
                        || sync_config_schemas(&app_context, false),
                        |result| result,
                    )
                    .await?,
            ])
        }));
    }

    if app_context.workspace_config.codeowners.sync_on_run {
        debug!(
            "Syncing code owners ({} enabled)",
            color::property("codeowners.syncOnRun"),
        );

        let app_context = Arc::clone(&app_context);

        operation_futures.push(task::spawn(async move {
            Ok(vec![
                Operation::sync_operation("codeowners")?
                    .track_async_with_check(
                        || sync_codeowners(&app_context, &workspace_graph, false),
                        |result| result.is_some(),
                    )
                    .await?,
            ])
        }));
    }

    if app_context.workspace_config.vcs.sync_hooks {
        debug!(
            "Syncing {} hooks ({} enabled)",
            app_context.workspace_config.vcs.manager,
            color::property("vcs.syncHooks"),
        );

        let app_context = Arc::clone(&app_context);

        operation_futures.push(task::spawn(async move {
            Ok(vec![
                Operation::sync_operation("vcs-hooks")?
                    .track_async_with_check(|| sync_vcs_hooks(&app_context, false), |result| result)
                    .await?,
            ])
        }));
    }

    if toolchain_registry.has_plugins() {
        debug!("Syncing operations from toolchains");

        let app_context = Arc::clone(&app_context);

        operation_futures.push(task::spawn(async move {
            let mut ops = vec![];

            for sync_result in toolchain_registry
                .sync_workspace_all(|registry, toolchain| SyncWorkspaceInput {
                    context: registry.create_context(),
                    toolchain_config: registry
                        .create_config(&toolchain.id, &app_context.toolchain_config),
                })
                .await?
            {
                ops.push(finalize_sync_operation(sync_result)?);
            }

            Ok(ops)
        }));
    }

    for future in operation_futures {
        action.operations.extend(future.await.into_diagnostic()??);
    }

    Ok(ActionStatus::Passed)
}
