//! The `workforce` module's API (design doc §8.15): the skills and
//! certifications catalogs, people's skills and certifications, what work
//! requires, and who fits it. Reading needs `workforce:read` (anywhere);
//! catalogs, requirements and other people's records need the instance-wide
//! `workforce:manage`. Everyone keeps their own skills (self-assessed) and
//! certifications.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use huxley_authz::permissions;
use huxley_db::{OrgScope, begin_scoped};
use huxley_workforce::availability::{self, Schedule, TimeOff, TimeOffRequest};
use huxley_workforce::catalog::{self, Certification, CertificationInput, Skill, SkillInput};
use huxley_workforce::people::{
    self, CertificationHeld, MatrixCell, Person, PersonSummary, SkillLevel,
};
use huxley_workforce::requirements::{self, Fit, Requirement};
use huxley_workforce::{MODULE, WorkforceError};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::authz::Authed;
use crate::module_routes::require_module;
use crate::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/skills", get(list_skills).post(create_skill))
        .route("/skills/{id}", put(update_skill).delete(archive_skill))
        .route(
            "/certifications",
            get(list_certifications).post(create_certification),
        )
        .route(
            "/certifications/{id}",
            put(update_certification).delete(archive_certification),
        )
        .route("/people", get(list_people))
        .route("/matrix", get(matrix))
        .route("/people/{user_id}", get(get_person))
        .route("/people/{user_id}/skills/{skill_id}", put(set_skill))
        .route(
            "/people/{user_id}/certifications",
            axum::routing::post(add_certification),
        )
        .route(
            "/people/{user_id}/certifications/{held_id}",
            put(update_held).delete(remove_held),
        )
        .route(
            "/requirements/{target_type}/{target_id}",
            get(get_requirements).put(set_requirements),
        )
        .route("/fit", get(fit))
        .route("/insights", get(insights))
        .route(
            "/people/{user_id}/schedule",
            get(get_schedule).put(set_schedule),
        )
        .route(
            "/people/{user_id}/time-off",
            axum::routing::post(request_time_off),
        )
        .route("/time-off", get(list_time_off))
        .route(
            "/time-off/{id}/decision",
            axum::routing::post(decide_time_off),
        )
        .route(
            "/time-off/{id}/cancel",
            axum::routing::post(cancel_time_off),
        )
}

pub(crate) fn map(e: WorkforceError) -> ApiError {
    match e {
        WorkforceError::NotFound => ApiError::not_found(),
        WorkforceError::Invalid { field, message } => {
            ApiError::validation().with_field_error(format!("/{field}"), message)
        }
        WorkforceError::VersionConflict => ApiError::conflict(
            "It was changed by someone else since you loaded it. Reload and try again.",
        ),
        WorkforceError::Db(e) => ApiError::internal(&e),
        WorkforceError::Sqlx(e) => ApiError::internal(&e),
    }
}

pub(crate) async fn system<T>(
    state: &AppState,
    f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<T, WorkforceError>,
) -> ApiResult<T> {
    let mut tx = begin_scoped(&state.db, &OrgScope::System)
        .await
        .map_err(|e| ApiError::internal(&e))?;
    let value = f(&mut tx).await.map_err(map)?;
    tx.commit().await.map_err(|e| ApiError::internal(&e))?;
    Ok(value)
}

/// The module is on, and the caller may read workforce records.
async fn reader(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    if authed
        .principal
        .granted_anywhere()
        .contains(&permissions::WORKFORCE_READ)
    {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

/// The module is on, and the caller manages workforce records.
async fn manager(state: &AppState, authed: &Authed) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    authed.require_instance(permissions::WORKFORCE_MANAGE)
}

/// The module is on, and the caller is `user_id` or manages records.
async fn self_or_manager(state: &AppState, authed: &Authed, user_id: Uuid) -> ApiResult<()> {
    require_module(state, MODULE).await?;
    if authed.user.auth.user.id == user_id
        || authed.principal.can_instance(permissions::WORKFORCE_MANAGE)
    {
        Ok(())
    } else {
        Err(ApiError::forbidden())
    }
}

/// The skills catalog.
#[utoipa::path(get, path = "/api/v1/workforce/skills", tag = "workforce",
    responses((status = 200, body = Vec<Skill>)))]
pub async fn list_skills(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Skill>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        system(&state, async |tx| catalog::skills(tx).await).await?,
    ))
}

/// Adds a skill to the catalog. Needs `workforce:manage`.
#[utoipa::path(post, path = "/api/v1/workforce/skills", tag = "workforce", request_body = SkillInput,
    responses((status = 201, body = Skill), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_skill(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<SkillInput>,
) -> ApiResult<(StatusCode, Json<Skill>)> {
    manager(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    let skill = system(&state, async |tx| {
        catalog::create_skill(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(skill)))
}

#[utoipa::path(put, path = "/api/v1/workforce/skills/{id}", tag = "workforce", params(("id" = Uuid, Path)),
    request_body = SkillInput,
    responses((status = 200, body = Skill), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_skill(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<SkillInput>,
) -> ApiResult<Json<Skill>> {
    manager(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            catalog::update_skill(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives a skill; people's levels in it and requirements on it go too.
#[utoipa::path(delete, path = "/api/v1/workforce/skills/{id}", tag = "workforce", params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_skill(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    manager(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    system(&state, async |tx| {
        catalog::archive_skill(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The certifications catalog.
#[utoipa::path(get, path = "/api/v1/workforce/certifications", tag = "workforce",
    responses((status = 200, body = Vec<Certification>)))]
pub async fn list_certifications(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<Certification>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        system(&state, async |tx| catalog::certifications(tx).await).await?,
    ))
}

/// Adds a certification to the catalog. Needs `workforce:manage`.
#[utoipa::path(post, path = "/api/v1/workforce/certifications", tag = "workforce",
    request_body = CertificationInput,
    responses((status = 201, body = Certification), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn create_certification(
    State(state): State<AppState>,
    authed: Authed,
    Json(body): Json<CertificationInput>,
) -> ApiResult<(StatusCode, Json<Certification>)> {
    manager(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    let cert = system(&state, async |tx| {
        catalog::create_certification(tx, &ctx, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(cert)))
}

#[utoipa::path(put, path = "/api/v1/workforce/certifications/{id}", tag = "workforce",
    params(("id" = Uuid, Path)), request_body = CertificationInput,
    responses((status = 200, body = Certification), (status = 409, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_certification(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<CertificationInput>,
) -> ApiResult<Json<Certification>> {
    manager(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            catalog::update_certification(tx, &ctx, id, &body).await
        })
        .await?,
    ))
}

/// Archives a certification; requirements on it go, held ones stay.
#[utoipa::path(delete, path = "/api/v1/workforce/certifications/{id}", tag = "workforce",
    params(("id" = Uuid, Path)),
    responses((status = 204), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn archive_certification(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    manager(&state, &authed).await?;
    let ctx = authed.user.ctx.clone();
    system(&state, async |tx| {
        catalog::archive_certification(tx, &ctx, id).await
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// People, with how many skills and certifications they have.
#[utoipa::path(get, path = "/api/v1/workforce/people", tag = "workforce",
    responses((status = 200, body = Vec<PersonSummary>)))]
pub async fn list_people(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<PersonSummary>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        system(&state, async |tx| people::people(tx).await).await?,
    ))
}

/// Everyone's level in every skill (the skills matrix).
#[utoipa::path(get, path = "/api/v1/workforce/matrix", tag = "workforce",
    responses((status = 200, body = Vec<MatrixCell>)))]
pub async fn matrix(
    State(state): State<AppState>,
    authed: Authed,
) -> ApiResult<Json<Vec<MatrixCell>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        system(&state, async |tx| people::matrix(tx).await).await?,
    ))
}

/// A person's skills and certifications. Yours, or with `workforce:read`.
#[utoipa::path(get, path = "/api/v1/workforce/people/{user_id}", tag = "workforce",
    params(("user_id" = Uuid, Path)),
    responses((status = 200, body = Person), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn get_person(
    State(state): State<AppState>,
    authed: Authed,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<Person>> {
    if authed.user.auth.user.id == user_id {
        require_module(&state, MODULE).await?;
    } else {
        reader(&state, &authed).await?;
    }
    Ok(Json(
        system(&state, async |tx| people::person(tx, user_id).await).await?,
    ))
}

/// Sets someone's level in a skill (`null` removes it). Your own levels are
/// self-assessed; verifying needs `workforce:manage`, and not for yourself.
#[utoipa::path(put, path = "/api/v1/workforce/people/{user_id}/skills/{skill_id}", tag = "workforce",
    params(("user_id" = Uuid, Path), ("skill_id" = Uuid, Path)), request_body = SkillLevel,
    responses((status = 200, body = Person), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_skill(
    State(state): State<AppState>,
    authed: Authed,
    Path((user_id, skill_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SkillLevel>,
) -> ApiResult<Json<Person>> {
    self_or_manager(&state, &authed, user_id).await?;
    let me = authed.user.auth.user.id;
    let verifier = if body.verified {
        if me == user_id {
            return Err(ApiError::validation()
                .with_field_error("/verified", "someone else verifies your skills"));
        }
        authed.require_instance(permissions::WORKFORCE_MANAGE)?;
        Some(me)
    } else {
        None
    };
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            people::set_skill(tx, &ctx, user_id, skill_id, &body, verifier).await
        })
        .await?,
    ))
}

/// Adds a certification someone holds. Yours, or with `workforce:manage`.
#[utoipa::path(post, path = "/api/v1/workforce/people/{user_id}/certifications", tag = "workforce",
    params(("user_id" = Uuid, Path)), request_body = CertificationHeld,
    responses((status = 201, body = Person), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn add_certification(
    State(state): State<AppState>,
    authed: Authed,
    Path(user_id): Path<Uuid>,
    Json(body): Json<CertificationHeld>,
) -> ApiResult<(StatusCode, Json<Person>)> {
    self_or_manager(&state, &authed, user_id).await?;
    let ctx = authed.user.ctx.clone();
    let person = system(&state, async |tx| {
        people::add_certification(tx, &ctx, user_id, &body).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(person)))
}

/// Changes a held certification (e.g. renewed).
#[utoipa::path(put, path = "/api/v1/workforce/people/{user_id}/certifications/{held_id}", tag = "workforce",
    params(("user_id" = Uuid, Path), ("held_id" = Uuid, Path)), request_body = CertificationHeld,
    responses((status = 200, body = Person), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn update_held(
    State(state): State<AppState>,
    authed: Authed,
    Path((user_id, held_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<CertificationHeld>,
) -> ApiResult<Json<Person>> {
    self_or_manager(&state, &authed, user_id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            people::update_certification(tx, &ctx, user_id, held_id, &body).await
        })
        .await?,
    ))
}

#[utoipa::path(delete, path = "/api/v1/workforce/people/{user_id}/certifications/{held_id}", tag = "workforce",
    params(("user_id" = Uuid, Path), ("held_id" = Uuid, Path)),
    responses((status = 200, body = Person), (status = 404, body = ApiError, content_type = "application/problem+json")))]
pub async fn remove_held(
    State(state): State<AppState>,
    authed: Authed,
    Path((user_id, held_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Person>> {
    self_or_manager(&state, &authed, user_id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            people::remove_certification(tx, &ctx, user_id, held_id).await
        })
        .await?,
    ))
}

/// What a queue, ticket type or ticket requires.
#[utoipa::path(get, path = "/api/v1/workforce/requirements/{target_type}/{target_id}", tag = "workforce",
    params(("target_type" = String, Path, description = "`queue`, `ticket_type` or `ticket`"), ("target_id" = Uuid, Path)),
    responses((status = 200, body = Vec<Requirement>)))]
pub async fn get_requirements(
    State(state): State<AppState>,
    authed: Authed,
    Path((target_type, target_id)): Path<(String, Uuid)>,
) -> ApiResult<Json<Vec<Requirement>>> {
    reader(&state, &authed).await?;
    Ok(Json(
        system(&state, async |tx| {
            requirements::of(tx, &target_type, target_id).await
        })
        .await?,
    ))
}

/// Replaces what a queue, ticket type or ticket requires: skills at a
/// minimum level, and certifications (hard requirements). Needs
/// `workforce:manage`, or for a ticket `ticket:write` where it is.
#[utoipa::path(put, path = "/api/v1/workforce/requirements/{target_type}/{target_id}", tag = "workforce",
    params(("target_type" = String, Path), ("target_id" = Uuid, Path)), request_body = Vec<Requirement>,
    responses((status = 200, body = Vec<Requirement>), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_requirements(
    State(state): State<AppState>,
    authed: Authed,
    Path((target_type, target_id)): Path<(String, Uuid)>,
    Json(body): Json<Vec<Requirement>>,
) -> ApiResult<Json<Vec<Requirement>>> {
    if target_type == "ticket" {
        require_module(&state, MODULE).await?;
        let org = crate::recommend_routes::ticket_org(&state, target_id).await?;
        authed
            .require_at(&state, permissions::TICKET_WRITE, org)
            .await?;
    } else {
        manager(&state, &authed).await?;
    }
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            requirements::set(tx, &ctx, &target_type, target_id, &body).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct InsightParams {
    /// Certifications expiring within this many days (default 60).
    pub days: Option<i32>,
}

/// Skill gaps: who meets each queue's and ticket type's requirements,
/// recent work done without them, and certifications about to expire with
/// what they'd leave uncovered. Needs `workforce:manage`.
#[utoipa::path(get, path = "/api/v1/workforce/insights", operation_id = "skill_gap_insights", tag = "workforce",
    params(InsightParams), responses((status = 200, body = huxley_workforce::insights::Insights)))]
pub async fn insights(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<InsightParams>,
) -> ApiResult<Json<huxley_workforce::insights::Insights>> {
    manager(&state, &authed).await?;
    let days = params.days.unwrap_or(60).clamp(1, 365);
    Ok(Json(
        system(&state, async |tx| {
            huxley_workforce::insights::insights(tx, days).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct FitParams {
    pub queue_id: Option<Uuid>,
    pub ticket_type_id: Option<Uuid>,
    /// A ticket (or project task) with requirements of its own.
    pub ticket_id: Option<Uuid>,
    /// When to check availability (RFC 3339; default now).
    pub at: Option<String>,
}

/// How everyone fits work in a queue and/or of a ticket type, best first:
/// required certifications held, and how well skills meet the levels.
#[utoipa::path(get, path = "/api/v1/workforce/fit", tag = "workforce", params(FitParams),
    responses((status = 200, body = Vec<Fit>)))]
pub async fn fit(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<FitParams>,
) -> ApiResult<Json<Vec<Fit>>> {
    reader(&state, &authed).await?;
    let at = match &params.at {
        Some(at) => time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339)
            .map_err(|_| ApiError::validation().with_field_error("/at", "an RFC 3339 time"))?
            .unix_timestamp(),
        None => time::OffsetDateTime::now_utc().unix_timestamp(),
    };
    Ok(Json(
        system(&state, async |tx| {
            let work = requirements::Work {
                queue: params.queue_id,
                ticket_type: params.ticket_type_id,
                ticket: params.ticket_id,
            };
            requirements::fit(tx, work, None, Some(at)).await
        })
        .await?,
    ))
}

/// Someone's working schedule (none: no hours limit). Yours, or with
/// `workforce:read`.
#[utoipa::path(get, path = "/api/v1/workforce/people/{user_id}/schedule", tag = "workforce",
    params(("user_id" = Uuid, Path)), responses((status = 200, body = Option<Schedule>)))]
pub async fn get_schedule(
    State(state): State<AppState>,
    authed: Authed,
    Path(user_id): Path<Uuid>,
) -> ApiResult<Json<Option<Schedule>>> {
    if authed.user.auth.user.id == user_id {
        require_module(&state, MODULE).await?;
    } else {
        reader(&state, &authed).await?;
    }
    Ok(Json(
        system(&state, async |tx| availability::schedule(tx, user_id).await).await?,
    ))
}

/// Sets someone's working schedule (`null` removes it). Yours, or with
/// `workforce:manage`.
#[utoipa::path(put, path = "/api/v1/workforce/people/{user_id}/schedule", tag = "workforce",
    params(("user_id" = Uuid, Path)), request_body = Option<Schedule>,
    responses((status = 200, body = Option<Schedule>), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn set_schedule(
    State(state): State<AppState>,
    authed: Authed,
    Path(user_id): Path<Uuid>,
    Json(body): Json<Option<Schedule>>,
) -> ApiResult<Json<Option<Schedule>>> {
    self_or_manager(&state, &authed, user_id).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            availability::set_schedule(tx, &ctx, user_id, body.as_ref()).await
        })
        .await?,
    ))
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct TimeOffParams {
    /// Someone's (default: everyone's, which needs `workforce:read`).
    pub user_id: Option<Uuid>,
    /// `requested`, `approved`, `declined` or `cancelled`.
    pub status: Option<String>,
}

/// Time off that hasn't long ended, soonest first.
#[utoipa::path(get, path = "/api/v1/workforce/time-off", tag = "workforce", params(TimeOffParams),
    responses((status = 200, body = Vec<TimeOff>)))]
pub async fn list_time_off(
    State(state): State<AppState>,
    authed: Authed,
    Query(params): Query<TimeOffParams>,
) -> ApiResult<Json<Vec<TimeOff>>> {
    if params.user_id == Some(authed.user.auth.user.id) {
        require_module(&state, MODULE).await?;
    } else {
        reader(&state, &authed).await?;
    }
    Ok(Json(
        system(&state, async |tx| {
            availability::time_off(tx, params.user_id, params.status.as_deref()).await
        })
        .await?,
    ))
}

/// Requests time off. Yours (it waits for a manager's approval), or someone
/// else's with `workforce:manage` (approved at once).
#[utoipa::path(post, path = "/api/v1/workforce/people/{user_id}/time-off", tag = "workforce",
    params(("user_id" = Uuid, Path)), request_body = TimeOffRequest,
    responses((status = 201, body = TimeOff), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn request_time_off(
    State(state): State<AppState>,
    authed: Authed,
    Path(user_id): Path<Uuid>,
    Json(body): Json<TimeOffRequest>,
) -> ApiResult<(StatusCode, Json<TimeOff>)> {
    self_or_manager(&state, &authed, user_id).await?;
    let me = authed.user.auth.user.id;
    let approved_by = (me != user_id).then_some(me);
    let ctx = authed.user.ctx.clone();
    let t = system(&state, async |tx| {
        availability::request(tx, &ctx, user_id, &body, approved_by).await
    })
    .await?;
    Ok((StatusCode::CREATED, Json(t)))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeOffDecision {
    pub approve: bool,
    #[serde(default)]
    pub note: Option<String>,
}

/// Approves or declines requested time off. Needs `workforce:manage`, and
/// not for your own.
#[utoipa::path(post, path = "/api/v1/workforce/time-off/{id}/decision", tag = "workforce",
    params(("id" = Uuid, Path)), request_body = TimeOffDecision,
    responses((status = 200, body = TimeOff), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn decide_time_off(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
    Json(body): Json<TimeOffDecision>,
) -> ApiResult<Json<TimeOff>> {
    manager(&state, &authed).await?;
    let me = authed.user.auth.user.id;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| {
            availability::decide(tx, &ctx, id, body.approve, me, body.note.as_deref()).await
        })
        .await?,
    ))
}

/// Cancels time off that hasn't ended. Yours, or with `workforce:manage`.
#[utoipa::path(post, path = "/api/v1/workforce/time-off/{id}/cancel", tag = "workforce",
    params(("id" = Uuid, Path)),
    responses((status = 200, body = TimeOff), (status = 422, body = ApiError, content_type = "application/problem+json")))]
pub async fn cancel_time_off(
    State(state): State<AppState>,
    authed: Authed,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TimeOff>> {
    let owner = system(&state, async |tx| availability::time_off_one(tx, id).await)
        .await?
        .user_id;
    self_or_manager(&state, &authed, owner).await?;
    let ctx = authed.user.ctx.clone();
    Ok(Json(
        system(&state, async |tx| availability::cancel(tx, &ctx, id).await).await?,
    ))
}
