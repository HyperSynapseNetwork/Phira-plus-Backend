//! Redemption-code administration and execution.
//! Frozen technical namespace remains `/admin/coupons` / `coupon:*`; product copy is “兑换码”.

use std::sync::Arc;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::extractors::{ApiJson, ApiPath, ApiQuery};
use crate::error::{ApiError, ErrorCode};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/coupons", get(list)).route("/coupons/create", post(create)).route("/coupons/{id}/revoke", post(revoke))
}
pub fn user_routes() -> Router<Arc<AppState>> { Router::new().route("/coupons/redeem", post(redeem)) }

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateCouponBody {
    #[serde(default)] pub code: String,
    pub action_type: String,
    #[serde(default, alias="payload")] pub args: Value,
    #[serde(default="manual_mode")] pub holder_mode: String,
    #[serde(default)] pub holder_phira_id: Option<i64>,
    #[serde(default)] pub note: String,
    #[serde(default)] pub max_uses: Option<i32>,
    #[serde(default)] pub expires_at: Option<DateTime<Utc>>,
}
fn manual_mode() -> String { "manual".into() }
#[derive(Debug, Deserialize, utoipa::ToSchema)] pub struct RedeemCodeBody { pub code: String }
#[derive(Debug, Serialize, utoipa::ToSchema)] pub struct RedeemCodeResponse { pub ok: bool, pub action_type: String, pub result: Value, pub redeemed_at: DateTime<Utc> }
#[derive(Debug, Deserialize)] pub struct CouponListParams { pub page: Option<i64>, #[serde(rename="pageNum")] pub page_num: Option<i64> }

#[utoipa::path(post,path="/api/v1/admin/coupons/create",operation_id="admin_coupons_create_post",request_body=CreateCouponBody,responses((status=200,description="redemption code created",body=serde_json::Value),(status=403,description="permission denied",body=ErrorEnvelope)),tag="admin")]
pub async fn create(auth: AuthPrincipal, State(state): State<Arc<AppState>>, ApiJson(body): ApiJson<CreateCouponBody>) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db,&auth,"coupon:create").await?; let db=state.require_db()?;
    validate_action(&body.action_type,&body.args)?;
    if !matches!(body.holder_mode.as_str(),"creator"|"manual") { return Err(ApiError::new(ErrorCode::ValidationFailed,"invalid holder mode")); }
    let max_uses=body.max_uses.unwrap_or(1); if max_uses<=0 { return Err(ApiError::new(ErrorCode::ValidationFailed,"max uses must be positive")); }
    if body.expires_at.is_some_and(|v| v<=Utc::now()) { return Err(ApiError::new(ErrorCode::ValidationFailed,"expiry must be in future")); }
    let holder_user_id = if body.holder_mode=="creator" {
        if auth.is_root() { return Err(ApiError::new(ErrorCode::ValidationFailed,"root cannot be redemption holder")); } Some(auth.sub)
    } else if let Some(pid)=body.holder_phira_id { Some(crate::users::repo::find_by_phira_id(db,pid).await?.ok_or_else(||ApiError::new(ErrorCode::UserNotFound,"user not found"))?.id) } else { None };
    let code=normalize(if body.code.trim().is_empty(){generate_code()}else{body.code});
    let args=if body.args.is_null(){json!({})}else{body.args};
    let row=sqlx::query_as::<_,(Uuid,String,DateTime<Utc>)>("INSERT INTO coupons(code,action_type,payload,max_uses,created_by,holder_mode,holder_user_id,note,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING id,code,created_at")
        .bind(&code).bind(&body.action_type).bind(args).bind(max_uses).bind(if auth.is_root(){None}else{Some(auth.sub)}).bind(&body.holder_mode).bind(holder_user_id).bind(body.note.trim()).bind(body.expires_at)
        .fetch_one(db).await.map_err(|e| if e.as_database_error().is_some_and(|d|d.is_unique_violation()){ApiError::new(ErrorCode::ResourceConflict,"redemption code already exists")}else{db_err(e)})?;
    Ok(Json(json!({"id":row.0,"code":row.1,"action_type":body.action_type,"holder_mode":body.holder_mode,"status":"active","note":body.note.trim(),"created_at":row.2})))
}

#[utoipa::path(post,path="/api/v1/admin/coupons/{id}/revoke",operation_id="admin_coupons_id_revoke_post",responses((status=204,description="revoked"),(status=403,description="permission denied",body=ErrorEnvelope)),tag="admin")]
pub async fn revoke(auth:AuthPrincipal,State(state):State<Arc<AppState>>,ApiPath(id):ApiPath<Uuid>)->Result<StatusCode,ApiError>{state.permissions.require(&state.db,&auth,"coupon:revoke").await?;let db=state.require_db()?;let r=sqlx::query("UPDATE coupons SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL").bind(id).execute(db).await.map_err(db_err)?;if r.rows_affected()==0{return Err(ApiError::new(ErrorCode::RedemptionCodeNotFound,"redemption code not found"));}Ok(StatusCode::NO_CONTENT)}

#[utoipa::path(get,path="/api/v1/admin/coupons",operation_id="admin_coupons_get",responses((status=200,description="redemption code list",body=serde_json::Value),(status=403,description="permission denied",body=ErrorEnvelope)),tag="admin")]
pub async fn list(auth:AuthPrincipal,State(state):State<Arc<AppState>>,ApiQuery(q):ApiQuery<CouponListParams>)->Result<Json<Value>,ApiError>{state.permissions.require(&state.db,&auth,"coupon:view").await?;let db=state.require_db()?;let page=q.page.unwrap_or(1).max(1);let n=q.page_num.unwrap_or(50).clamp(1,100);let total=sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM coupons").fetch_one(db).await.map_err(db_err)?;let rows=sqlx::query_as::<_,(Uuid,String,String,String,i32,i32,Option<DateTime<Utc>>,Option<DateTime<Utc>>,String,DateTime<Utc>)>("SELECT id,code,action_type,holder_mode,max_uses,used_count,revoked_at,expires_at,note,created_at FROM coupons ORDER BY created_at DESC LIMIT $1 OFFSET $2").bind(n).bind((page-1)*n).fetch_all(db).await.map_err(db_err)?;let now=Utc::now();let items=rows.into_iter().map(|(id,code,action_type,holder_mode,max_uses,used_count,revoked_at,expires_at,note,created_at)|{let status=if revoked_at.is_some(){"revoked"}else if expires_at.is_some_and(|v|v<=now){"expired"}else if used_count>=max_uses{"redeemed"}else{"active"};json!({"id":id,"code":code,"action_type":action_type,"holder_mode":holder_mode,"status":status,"max_uses":max_uses,"used_count":used_count,"expires_at":expires_at,"note":note,"created_at":created_at})}).collect::<Vec<_>>();Ok(Json(json!({"items":items,"total":total,"page":page,"pageNum":n})))}

#[utoipa::path(post,path="/api/v1/coupons/redeem",operation_id="coupons_redeem_post",request_body=RedeemCodeBody,responses((status=200,description="redemption completed",body=RedeemCodeResponse),(status=409,description="code unavailable",body=ErrorEnvelope)),tag="coupons")]
pub async fn redeem(auth:AuthPrincipal,State(state):State<Arc<AppState>>,ApiJson(body): ApiJson<RedeemCodeBody>)->Result<Json<RedeemCodeResponse>,ApiError>{
    if auth.is_root(){return Err(ApiError::permission_denied());} let code=normalize(body.code); if code.is_empty(){return Err(ApiError::new(ErrorCode::ValidationFailed,"code required"));}
    let db=state.require_db()?; let mut tx=db.begin().await.map_err(db_err)?;
    type Row=(Uuid,String,Value,i32,i32,Option<DateTime<Utc>>,Option<DateTime<Utc>>,String,Option<Uuid>);
    let (coupon_id,action_type,payload,max_uses,used_count,revoked_at,expires_at,holder_mode,holder_user_id)=sqlx::query_as::<_,Row>("SELECT id,action_type,payload,max_uses,used_count,revoked_at,expires_at,holder_mode,holder_user_id FROM coupons WHERE code=$1 FOR UPDATE").bind(&code).fetch_optional(&mut *tx).await.map_err(db_err)?.ok_or_else(||ApiError::new(ErrorCode::RedemptionCodeNotFound,"redemption code not found"))?;
    if revoked_at.is_some(){return Err(ApiError::new(ErrorCode::RedemptionCodeRevoked,"redemption code revoked"));}
    if expires_at.is_some_and(|v|v<=Utc::now()){return Err(ApiError::new(ErrorCode::RedemptionCodeExpired,"redemption code expired"));}
    if used_count>=max_uses{return Err(ApiError::new(ErrorCode::RedemptionCodeLimitReached,"redemption code limit reached"));}
    if holder_user_id.is_some() && holder_user_id!=Some(auth.sub){return Err(ApiError::permission_denied());}
    if holder_mode=="creator" && holder_user_id!=Some(auth.sub){return Err(ApiError::permission_denied());}
    let prior=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM coupon_redemptions WHERE coupon_id=$1 AND user_id=$2)").bind(coupon_id).bind(auth.sub).fetch_one(&mut *tx).await.map_err(db_err)?; if prior{return Err(ApiError::new(ErrorCode::RedemptionCodeAlreadyUsed,"redemption code already used"));}
    validate_action(&action_type,&payload)?; let result=execute_action(&mut tx,auth.sub,coupon_id,&action_type,&payload).await?; let redeemed_at=Utc::now();
    sqlx::query("UPDATE coupons SET used_count=used_count+1 WHERE id=$1").bind(coupon_id).execute(&mut *tx).await.map_err(db_err)?;
    sqlx::query("INSERT INTO coupon_redemptions(coupon_id,user_id,action_type,result,redeemed_at) VALUES($1,$2,$3,$4,$5)").bind(coupon_id).bind(auth.sub).bind(&action_type).bind(&result).bind(redeemed_at).execute(&mut *tx).await.map_err(db_err)?;
    tx.commit().await.map_err(db_err)?; Ok(Json(RedeemCodeResponse{ok:true,action_type,result,redeemed_at}))
}

fn validate_action(kind:&str,args:&Value)->Result<(),ApiError>{if !args.is_object(){return Err(ApiError::new(ErrorCode::ValidationFailed,"redemption args must be object"));}match kind{"account_unlock"|"admin_alert"=>Ok(()),"account_role"=>if args.get("group_id").and_then(Value::as_str).is_some()||args.get("group_name").and_then(Value::as_str).is_some(){Ok(())}else{Err(ApiError::new(ErrorCode::ValidationFailed,"group target required"))},"custom_hook"=>if args.get("hook_id").and_then(Value::as_str)==Some("notification.self"){Ok(())}else{Err(ApiError::new(ErrorCode::RedemptionActionUnsupported,"unsupported redemption hook"))},_=>Err(ApiError::new(ErrorCode::RedemptionActionUnsupported,"unsupported redemption action"))}}
async fn execute_action(tx:&mut sqlx::Transaction<'_,sqlx::Postgres>,user_id:Uuid,coupon_id:Uuid,kind:&str,args:&Value)->Result<Value,ApiError>{match kind{
"account_unlock"=>{sqlx::query("UPDATE users SET status='active',updated_at=now() WHERE id=$1").bind(user_id).execute(&mut **tx).await.map_err(db_err)?;Ok(json!({"status":"active"}))},
"account_role"=>{let group=if let Some(raw)=args.get("group_id").and_then(Value::as_str){let id=Uuid::parse_str(raw).map_err(|_|ApiError::new(ErrorCode::ValidationFailed,"invalid group id"))?;sqlx::query_as::<_,(Uuid,String,Option<String>)>("SELECT id,name,system_kind FROM groups WHERE id=$1").bind(id).fetch_optional(&mut **tx).await.map_err(db_err)?}else{sqlx::query_as::<_,(Uuid,String,Option<String>)>("SELECT id,name,system_kind FROM groups WHERE name=$1").bind(args.get("group_name").and_then(Value::as_str).unwrap_or("")).fetch_optional(&mut **tx).await.map_err(db_err)?}.ok_or_else(||ApiError::new(ErrorCode::GroupNotFound,"group not found"))?;if group.2.as_deref()==Some("admin_scope")||group.1.eq_ignore_ascii_case("Administrators"){return Err(ApiError::permission_denied());}sqlx::query("INSERT INTO group_members(group_id,user_id) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(group.0).bind(user_id).execute(&mut **tx).await.map_err(db_err)?;Ok(json!({"group_id":group.0,"group_name":group.1}))},
"admin_alert"=>{let task_type=args.get("task_type").and_then(Value::as_str).unwrap_or("redemption_review");let id=sqlx::query_scalar::<_,Uuid>("INSERT INTO admin_tasks(source,source_id,task_type,payload) VALUES('coupon',$1,$2,$3) RETURNING id").bind(coupon_id).bind(task_type).bind(json!({"coupon_id":coupon_id,"redeemed_by":user_id,"args":args})).fetch_one(&mut **tx).await.map_err(db_err)?;Ok(json!({"admin_task_id":id,"task_type":task_type}))},
"custom_hook"=>{let title=args.get("title").and_then(Value::as_str).unwrap_or("兑换成功");let body=args.get("body").and_then(Value::as_str).unwrap_or("兑换码奖励已发放。");let event=sqlx::query_scalar::<_,Uuid>("INSERT INTO notification_events(type,actor_user_id,payload) VALUES('redemption.completed',NULL,$1) RETURNING id").bind(json!({"type":"redemption.completed","priority":"normal","title":title,"body":body,"actions":[]})).fetch_one(&mut **tx).await.map_err(db_err)?;sqlx::query("INSERT INTO user_notifications(event_id,user_id) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(event).bind(user_id).execute(&mut **tx).await.map_err(db_err)?;Ok(json!({"notification_event_id":event}))},
_=>Err(ApiError::new(ErrorCode::RedemptionActionUnsupported,"unsupported redemption action"))}}
fn normalize(v:impl Into<String>)->String{v.into().trim().to_ascii_uppercase()}
fn generate_code()->String{use rand::Rng;let mut r=rand::thread_rng();let c=b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";(0..12).map(|_|c[r.gen_range(0..c.len())] as char).collect()}
fn db_err(e:sqlx::Error)->ApiError{tracing::error!(error=%e,"redemption database error");ApiError::internal()}
