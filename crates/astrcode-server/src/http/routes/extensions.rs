//! 扩展查看 / 重载 / 启停路由。

use std::collections::{BTreeMap, BTreeSet};

use astrcode_extensions::runner::{ExtensionStageDiagnostics, ExtensionStageStatus};
use astrcode_protocol::{
    events::ClientNotification,
    http::{
        ExtensionDeclarationDto, ExtensionDiagnosticsDto, ExtensionListResponseDto,
        ExtensionReloadResponseDto, ExtensionStageDiagnosticsDto, ExtensionStateDto,
        SetExtensionEnabledRequest, SetExtensionEnabledResponseDto,
    },
};
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};

use super::super::{HttpState, bad_request_response, internal_error_response};

pub(in crate::http) async fn list_extensions(State(state): State<HttpState>) -> Response {
    Json(ExtensionListResponseDto {
        extensions: collect_extensions(&state).await,
    })
    .into_response()
}

pub(in crate::http) async fn reload_extensions(State(state): State<HttpState>) -> Response {
    let reload_errors = state.runtime.reload_extensions().await;
    state
        .event_bus
        .send_notification(ClientNotification::ExtensionRegistryChanged);
    for error in &reload_errors {
        tracing::warn!("extension reload error: {error}");
    }
    Json(ExtensionReloadResponseDto { reload_errors }).into_response()
}

pub(in crate::http) async fn set_enabled(
    State(state): State<HttpState>,
    Json(request): Json<SetExtensionEnabledRequest>,
) -> Response {
    let mut candidate = state.runtime.config_manager().raw_config_snapshot();
    let extension_states = candidate
        .runtime
        .extension_states
        .get_or_insert_with(BTreeMap::new);
    extension_states.insert(request.extension_id.clone(), request.enabled);

    if let Err(error) = candidate.clone().into_effective() {
        return bad_request_response("invalid_extension_state", error);
    }

    if let Err(error) = state
        .runtime
        .config_manager
        .config_store()
        .save(&candidate)
        .await
    {
        return internal_error_response("save_failed", error);
    }

    if let Err(error) = state
        .runtime
        .config_manager
        .apply_raw_config_and_rebuild(candidate)
    {
        return bad_request_response("invalid_extension_state", error);
    }
    state.runtime.sync_session_model_bindings();

    // 通知扩展配置已变更
    let config_errors = state
        .runtime
        .config_manager
        .notify_extensions_config_changed()
        .await;
    for error in &config_errors {
        tracing::warn!("extension config notify error: {error}");
    }

    let reload_errors = state.runtime.reload_extensions().await;
    state
        .event_bus
        .send_notification(ClientNotification::ExtensionRegistryChanged);
    for error in &reload_errors {
        tracing::warn!("extension reload error: {error}");
    }

    Json(SetExtensionEnabledResponseDto {
        success: true,
        reload_errors,
    })
    .into_response()
}

async fn collect_extensions(state: &HttpState) -> Vec<ExtensionStateDto> {
    let effective = state.runtime.config_manager().read_effective();
    let runner = state.runtime.extension_runner();
    let loaded_ids = runner.registered_extension_ids().await;
    let loaded_set: BTreeSet<_> = loaded_ids.iter().cloned().collect();
    let registry = runner.registry_snapshot().await;
    let declarations: BTreeMap<_, _> = registry
        .extensions
        .into_iter()
        .map(|declaration| (declaration.id.clone(), declaration))
        .collect();
    let diagnostics = runner.diagnostics_snapshot();
    let bundled_set: BTreeSet<_> = astrcode_bundled_extensions::bundled_extension_ids()
        .into_iter()
        .map(str::to_string)
        .collect();

    let mut ids: BTreeSet<String> = loaded_set.iter().cloned().collect();
    ids.extend(bundled_set.iter().cloned());
    ids.extend(effective.extensions.extension_states.keys().cloned());
    ids.extend(diagnostics.keys().cloned());

    ids.into_iter()
        .map(|extension_id| {
            let source = if bundled_set.contains(&extension_id) {
                "builtin"
            } else if loaded_set.contains(&extension_id) {
                "disk"
            } else {
                "unknown"
            };
            ExtensionStateDto {
                enabled: astrcode_bundled_extensions::extension_enabled(
                    &effective.extensions.extension_states,
                    &extension_id,
                ),
                loaded: loaded_set.contains(&extension_id),
                declaration: declarations
                    .get(&extension_id)
                    .cloned()
                    .map(extension_declaration_dto),
                diagnostics: diagnostics
                    .get(&extension_id)
                    .cloned()
                    .map(extension_diagnostics_dto),
                extension_id,
                source: source.to_string(),
            }
        })
        .collect()
}

fn extension_declaration_dto(
    declaration: astrcode_extensions::runner::ExtensionDeclarationSnapshot,
) -> ExtensionDeclarationDto {
    ExtensionDeclarationDto {
        id: declaration.id,
        capabilities: declaration.capabilities,
        tools: declaration.tools,
        dynamic_tools: declaration.dynamic_tools,
        commands: declaration.commands,
        dynamic_commands: declaration.dynamic_commands,
        keybindings: declaration.keybindings,
        status_items: declaration.status_items,
        events: declaration.events,
    }
}

fn extension_diagnostics_dto(
    diagnostics: astrcode_extensions::runner::ExtensionDiagnostics,
) -> ExtensionDiagnosticsDto {
    ExtensionDiagnosticsDto {
        load: extension_stage_diagnostics_dto(diagnostics.load),
        register: extension_stage_diagnostics_dto(diagnostics.register),
        start: extension_stage_diagnostics_dto(diagnostics.start),
        hook_calls: diagnostics.hook_calls,
        hook_timeouts: diagnostics.hook_timeouts,
        last_hook: diagnostics.last_hook,
        last_duration_ms: diagnostics.last_duration_ms,
        last_error: diagnostics.last_error,
    }
}

fn extension_stage_diagnostics_dto(
    diagnostics: ExtensionStageDiagnostics,
) -> ExtensionStageDiagnosticsDto {
    ExtensionStageDiagnosticsDto {
        status: extension_stage_status_string(diagnostics.status).to_string(),
        duration_ms: diagnostics.duration_ms,
        error: diagnostics.error,
    }
}

fn extension_stage_status_string(status: ExtensionStageStatus) -> &'static str {
    match status {
        ExtensionStageStatus::Unknown => "unknown",
        ExtensionStageStatus::Running => "running",
        ExtensionStageStatus::Succeeded => "succeeded",
        ExtensionStageStatus::Failed => "failed",
        ExtensionStageStatus::Skipped => "skipped",
    }
}
