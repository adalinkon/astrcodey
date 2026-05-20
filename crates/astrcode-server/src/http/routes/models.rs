//! Model 列表 / 当前激活 / 连通性测试路由。

use astrcode_protocol::http::{
    AvailableModelDto, CurrentModelResponseDto, ModelListResponseDto, ModelTestResponseDto,
};
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};

use super::super::HttpState;

pub(in crate::http) async fn get_current_model(State(state): State<HttpState>) -> Response {
    let raw = state.runtime.config_manager.read_raw_config();
    let eff = state.runtime.config_manager.read_effective();
    Json(CurrentModelResponseDto {
        profile_name: raw.active_profile.clone(),
        model_id: eff.llm.model_id.clone(),
        provider_kind: eff.llm.provider_kind.clone(),
    })
    .into_response()
}

pub(in crate::http) async fn list_models(State(state): State<HttpState>) -> Response {
    let raw = state.runtime.config_manager.read_raw_config();
    let models: Vec<AvailableModelDto> = raw
        .profiles
        .iter()
        .flat_map(|p| {
            p.models.iter().map(|m| AvailableModelDto {
                profile_name: p.name.clone(),
                model_id: m.id.clone(),
                provider_kind: p.provider_kind.clone(),
            })
        })
        .collect();
    Json(ModelListResponseDto { models }).into_response()
}

pub(in crate::http) async fn test_model(State(state): State<HttpState>) -> Response {
    let start = std::time::Instant::now();
    match state
        .runtime
        .config_manager
        .read_llm_provider()
        .generate(vec![astrcode_core::llm::LlmMessage::user("Hi")], vec![])
        .await
    {
        Ok(mut rx) => {
            while rx.recv().await.is_some() {}
            Json(ModelTestResponseDto {
                success: true,
                message: format!("ok ({}ms)", start.elapsed().as_millis()),
            })
            .into_response()
        },
        Err(error) => Json(ModelTestResponseDto {
            success: false,
            message: error.to_string(),
        })
        .into_response(),
    }
}
