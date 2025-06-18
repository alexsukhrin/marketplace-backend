use actix_web::{HttpResponse, Responder, get, post, web};
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

use utoipa::ToSchema;
use uuid::Uuid;

use crate::handlers::auth::AuthenticatedUser;

#[derive(Debug, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[sqlx(type_name = "chat_status", rename_all = "snake_case")]
pub enum ChatStatus {
    Active,
    Inactive,
    Request,
}

#[derive(Debug, ToSchema, Serialize, Deserialize, FromRow)]
pub struct ChatResponse {
    id: i32,
    status: ChatStatus,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

#[derive(Debug, ToSchema, Serialize, Deserialize, FromRow)]
pub struct MessageResponse {
    id: i32,
    chat_id: i32,
    sender_id: Uuid,
    message: String,
    is_read: bool,
    sent_at: NaiveDateTime,
}

#[derive(Debug, ToSchema, Serialize, Deserialize)]
pub struct UnreadCountResponse {
    unread_count: i32,
}

#[derive(Debug, ToSchema, Serialize, Deserialize)]
pub struct CreateChatRequest {
    recipient_id: Uuid,
}

#[derive(Debug, ToSchema, Serialize, Deserialize)]
pub struct CreateMessageRequest {
    chat_id: i32,
    message: String,
}

async fn is_user_chat_participant(
    chat_id: i32,
    user_id: Uuid,
    db_pool: &PgPool,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1 FROM chat_users
            WHERE chat_id = $1 AND user_id = $2
        ) AS exists",
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_one(db_pool)
    .await
}

async fn is_chat_exist(chat_id: i32, db_pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM chats WHERE id = $1) AS exists")
        .bind(chat_id)
        .fetch_one(db_pool)
        .await
}

#[utoipa::path(
    tag = "chat",
    responses(
        (status = 200, description = "List of all chats", body = Vec<ChatResponse>),
        (status = 500, description = "Internal Server Error")
    )
)]
#[get("")]
pub async fn chat_list(
    user: AuthenticatedUser,
    db_pool: web::Data<PgPool>,
) -> Result<impl Responder, actix_web::Error> {
    // retrieve all chats
    let chats: Vec<ChatResponse> = sqlx::query_as(
        "SELECT
            c.id,
            c.status,
            c.created_at
        FROM
            chats c
        JOIN
            chat_users cu ON c.id = cu.chat_id
        JOIN
            users u ON cu.user_id = u.id
        WHERE
            u.id = $1;",
    )
    .bind(user.0.sub)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to fetch chats")
    })?;

    Ok(HttpResponse::Ok().json(chats))
}

#[utoipa::path(
    tag = "chat",
    responses(
        (status = 200, description = "Chat created", body = ChatResponse),
        (status = 409, description = "Chat already exists"),
        (status = 500, description = "Internal Server Error")
    ),
    request_body = CreateChatRequest
)]
#[post("")]
pub async fn chat_create(
    user: AuthenticatedUser,
    request: web::Json<CreateChatRequest>,
    db_pool: web::Data<PgPool>,
) -> Result<impl Responder, actix_web::Error> {
    let current_user_id = user.0.sub;
    let recipient_id = request.recipient_id;

    // cant create chat with yourself
    if current_user_id == recipient_id {
        return Ok(HttpResponse::BadRequest().body("Cannot create a chat with yourself"));
    }

    // begin transaction
    let mut tx = db_pool.begin().await.map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to begin transaction")
    })?;

    // check if a chat between users already exists
    let existing_chat_id: Option<i32> = sqlx::query_scalar(
        "SELECT
            c.id
        FROM
            chats c
        JOIN
            chat_users cu1 ON c.id = cu1.chat_id
        JOIN
            chat_users cu2 ON c.id = cu2.chat_id
        WHERE
            cu1.user_id = $1 AND cu2.user_id = $2",
    )
    .bind(current_user_id)
    .bind(recipient_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to check if chat exists")
    })?;

    if existing_chat_id.is_some() {
        return Ok(HttpResponse::Conflict().body("Chat already exists between these users"));
    }

    // create new chat
    let new_chat: ChatResponse = sqlx::query_as(
        "INSERT INTO chats (status)
        VALUES ($1::chat_status)
        RETURNING id, status, created_at",
    )
    .bind(ChatStatus::Request)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to create chat")
    })?;

    let chat_id = new_chat.id;

    // add chat connections for both users
    let _ = sqlx::query("INSERT INTO chat_users (chat_id, user_id) VALUES ($1, $2), ($1, $3)")
        .bind(chat_id)
        .bind(current_user_id)
        .bind(recipient_id)
        .execute(&mut *tx)
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to add users to chat")
        })?;

    // end transaction
    tx.commit().await.map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to commit changes")
    })?;

    Ok(HttpResponse::Ok().json(new_chat))
}

#[utoipa::path(
    tag = "chat",
    responses(
        (status = 200, description = "List of all messages in the chat", body = Vec<MessageResponse>),
        (status = 403, description = "User is not a participant in the chat"),
        (status = 404, description = "Chat does not exist"),
        (status = 500, description = "Internal Server Error"),
    ),
    params(
        ("chat_id" = i32, Path),
    )
)]
#[get("/{chat_id}")]
pub async fn message_list(
    user: AuthenticatedUser,
    chat_id: web::Path<i32>,
    db_pool: web::Data<PgPool>,
) -> Result<impl Responder, actix_web::Error> {
    let current_user_id = user.0.sub;
    let chat_id = chat_id.into_inner();

    // verify if chat exists
    let chat_exists = is_chat_exist(chat_id, db_pool.get_ref())
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to verify chat existence")
        })?;

    if !chat_exists {
        return Ok(HttpResponse::NotFound().body("Chat not found"));
    }

    // verify if the user is a participant of the chat
    let is_participant = is_user_chat_participant(chat_id, current_user_id, db_pool.get_ref())
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to verify user participation")
        })?;

    if !is_participant {
        return Ok(HttpResponse::Forbidden().body("You are not a participant in this chat"));
    }

    // fetch messages for the given chat_id, ordered by creation time
    let messages: Vec<MessageResponse> = sqlx::query_as(
        "SELECT
            id,
            chat_id,
            sender_id,
            content,
            is_read,
            created_at
        FROM
            messages
        WHERE
            chat_id = $1
        ORDER BY
            created_at ASC",
    )
    .bind(chat_id)
    .fetch_all(db_pool.get_ref())
    .await
    .map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to retrieve messages")
    })?;

    Ok(HttpResponse::Ok().json(messages))
}

#[utoipa::path(
    tag = "chat",
    responses(
        (status = 200, description = "Message created", body = MessageResponse),
        (status = 400, description = "Invalid input (e.g., empty message)"),
        (status = 403, description = "User is not a participant in the chat"),
        (status = 404, description = "Chat does not exist"),
        (status = 500, description = "Internal Server Error")
    ),
    request_body = CreateMessageRequest
)]
#[post("")]
pub async fn message_create(
    user: AuthenticatedUser,
    request: web::Json<CreateMessageRequest>,
    db_pool: web::Data<PgPool>,
) -> Result<impl Responder, actix_web::Error> {
    let sender_id = user.0.sub;
    let chat_id = request.chat_id;
    let message = &request.message.trim();

    if message.is_empty() {
        return Ok(HttpResponse::BadRequest().body("Message content cannot be empty"));
    }

    // verify if chat exists
    let chat_exists = is_chat_exist(chat_id, db_pool.get_ref())
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to verify chat existence")
        })?;

    if !chat_exists {
        return Ok(HttpResponse::NotFound().body("Chat not found"));
    }

    // verify if the user is a participant of the chat
    let is_participant = is_user_chat_participant(chat_id, sender_id, db_pool.get_ref())
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to verify user participation")
        })?;

    if !is_participant {
        return Ok(HttpResponse::Forbidden().body("You are not a participant in this chat"));
    }

    // store massage
    let new_message: MessageResponse = sqlx::query_as(
        "INSERT INTO
            messages (chat_id, sender_id, content, is_read)
        VALUES
            ($1, $2, $3, $4)
        RETURNING
            id, chat_id, sender_id, content, is_read, created_at",
    )
    .bind(chat_id)
    .bind(sender_id)
    .bind(message)
    .bind(false) // is_read defaults to false
    .fetch_one(db_pool.get_ref())
    .await
    .map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to create message")
    })?;

    Ok(HttpResponse::Ok().json(new_message))
}

#[utoipa::path(
    tag = "chat",
    responses(
        (status = 200, description = "Count of unread messages in the chat for the user", body = UnreadCountResponse),
        (status = 403, description = "User is not a participant in the chat"),
        (status = 404, description = "Chat does not exist"),
        (status = 500, description = "Internal Server Error")
    ),
    params(
        ("chat_id" = i32, Path),
    )
)]
#[get("/unread_count/{chat_id}")]
pub async fn message_unread_count(
    user: AuthenticatedUser,
    chat_id: web::Path<i32>,
    db_pool: web::Data<PgPool>,
) -> Result<impl Responder, actix_web::Error> {
    let current_user_id = user.0.sub;
    let chat_id = chat_id.into_inner();

    // verify if chat exists
    let chat_exists = is_chat_exist(chat_id, db_pool.get_ref())
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to verify chat existence")
        })?;

    if !chat_exists {
        return Ok(HttpResponse::NotFound().body("Chat not found"));
    }

    // verify if the user is a participant of the chat
    let is_participant = is_user_chat_participant(chat_id, current_user_id, db_pool.get_ref())
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to verify user participation")
        })?;

    if !is_participant {
        return Ok(HttpResponse::Forbidden().body("You are not a participant in this chat"));
    }

    // count unread messages
    let unread_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(id) FROM messages
        WHERE
            chat_id = $1 AND
            is_read = FALSE AND
            sender_id != $2",
    )
    .bind(chat_id)
    .bind(current_user_id)
    .fetch_one(db_pool.get_ref())
    .await
    .map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to retrieve unread message count")
    })?;

    Ok(HttpResponse::Ok().json(UnreadCountResponse { unread_count }))
}

#[utoipa::path(
    tag = "chat",
    responses(
        (status = 200, description = "Messages are marked as read"),
        (status = 403, description = "User is not a participant in the chat"),
        (status = 404, description = "Chat does not exist"),
        (status = 500, description = "Internal Server Error")
    ),
    params(
        ("chat_id" = i32, Path),
    )
)]
#[get("/mark_read/{chat_id}")]
pub async fn message_mark_read(
    user: AuthenticatedUser,
    chat_id: web::Path<i32>,
    db_pool: web::Data<PgPool>,
) -> Result<impl Responder, actix_web::Error> {
    let current_user_id = user.0.sub;
    let chat_id = chat_id.into_inner();

    // verify if chat exists
    let chat_exists = is_chat_exist(chat_id, db_pool.get_ref())
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to verify chat existence")
        })?;

    if !chat_exists {
        return Ok(HttpResponse::NotFound().body("Chat not found"));
    }

    // verify if the user is a participant of the chat
    let is_participant = is_user_chat_participant(chat_id, current_user_id, db_pool.get_ref())
        .await
        .map_err(|_err| {
            // HACK: log an error here
            actix_web::error::ErrorInternalServerError("Failed to verify user participation")
        })?;

    if !is_participant {
        return Ok(HttpResponse::Forbidden().body("You are not a participant in this chat"));
    }

    // mark unread messages in this chat as read for the current user
    let _ = sqlx::query(
        "UPDATE messages
        SET is_read = TRUE
        WHERE
            chat_id = $1 AND
            sender_id != $2 AND
            is_read = FALSE",
    )
    .bind(chat_id)
    .bind(current_user_id)
    .execute(db_pool.get_ref())
    .await
    .map_err(|_err| {
        // HACK: log an error here
        actix_web::error::ErrorInternalServerError("Failed to mark messages as read")
    })?;

    // return 200 with an empty body on success
    Ok(HttpResponse::Ok().finish())
}
