use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, Utc};
use zxcvbn::Score;
use std::{fs, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};
use uuid::{Uuid};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use sqlx::Row;
use sqlx::sqlite::{SqlitePoolOptions, SqlitePool, SqliteRow};

#[derive(Debug)]
pub struct User {
    pub id: Uuid,
    pub chat_id: i64,
    pub login: String,
    pub password: String,
    pub created_at: DateTime<Local>,
}

pub async fn init_database(db_path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await?;

    sqlx::query("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
        .execute(&pool)
        .await?;

    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id BLOB PRIMARY KEY NOT NULL,
                chat_id INTEGER NOT NULL UNIQUE,
                login TEXT NOT NULL UNIQUE,
                password TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sessions (
                token TEXT PRIMARY KEY NOT NULL,
                user_id BLOB NOT NULL,
                expires_at INTEGER NOT NULL,
                FOREIGN KEY(user_id) REFERENCES users(id)
            );

            CREATE TABLE IF NOT EXISTS chats (
                id BLOB PRIMARY KEY NOT NULL,
                type TEXT NOT NULL,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_members (
                chat_id BLOB NOT NULL,
                user_id BLOB NOT NULL,
                joined_at INTEGER NOT NULL,
                PRIMARY KEY(chat_id, user_id),
                FOREIGN KEY(chat_id) REFERENCES chats(id),
                FOREIGN KEY(user_id) REFERENCES users(id)
            );

            CREATE TABLE IF NOT EXISTS messages (
                id BLOB PRIMARY KEY NOT NULL,
                chat_id BLOB NOT NULL,
                sender_id BLOB NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                is_read INTEGER DEFAULT 0,
                FOREIGN KEY(chat_id) REFERENCES chats(id),
                FOREIGN KEY(sender_id) REFERENCES users(id)
            );

            CREATE TABLE IF NOT EXISTS friends (
                user_id BLOB NOT NULL,
                friend_id BLOB NOT NULL,
                status TEXT NOT NULL,
                PRIMARY KEY(user_id, friend_id),
                FOREIGN KEY(user_id) REFERENCES users(id),
                FOREIGN KEY(friend_id) REFERENCES users(id)
            );
            "#,
        ).execute(pool).await?;

    Ok(())
}

pub fn get_db_path() -> Result<PathBuf> {
    let base_dir = if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").context("APPDATA not set")?;
        PathBuf::from(appdata.replace("Roaming", "LocalLow"))
    } else {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(xdg)
        } else {
            let home = std::env::var("HOME").context("HOME not set")?;
            PathBuf::from(home).join(".local/share")
        }
    };
    let db_dir = base_dir.join("Zeerck Inc").join("MUC-server").join("db");
    Ok(db_dir.join("database.sqlite"))
}

async fn generate_unique_chat_id(pool: &SqlitePool) -> i64 {
    loop {
        let id = (rand::random::<u32>() % 9000000 + 1000000) as i64;

        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE chat_id = ?)")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap_or(true);

        if !exists {
            return id;
        }
    }
}

pub async fn add_user(pool: &SqlitePool, login: &str, raw_password: &str) -> Result<User> {
    if !validate_login(login) {
        anyhow::bail!("invalid login: must be 3-32 letters, numbers or underscores");
    }

    let entropy = zxcvbn::zxcvbn(raw_password, &[]);
    if entropy.score() < Score::Two {
        anyhow::bail!("password is too weak");
    }

    let id = Uuid::new_v4();
    let chat_id = generate_unique_chat_id(pool).await;
    let hashed_password = hash_password(raw_password).expect("Error while hashing password!");
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("System time before epoch").as_secs() as i64;

    sqlx::query("INSERT INTO users (id, chat_id, login, password, created_at) VALUES (?, ?, ?, ?, ?)")
        .bind(id)
        .bind(chat_id)
        .bind(login)
        .bind(&hashed_password)
        .bind(now)
        .execute(pool)
        .await?;

    Ok(User {
        id,
        chat_id,
        login: login.to_string(),
        password: hashed_password,
        created_at: DateTime::from(SystemTime::now()),
    })
}

pub async fn get_user_by_login(pool: &SqlitePool, login: &str) -> Result<Option<User>> {
    let row = sqlx::query("SELECT id, chat_id, login, password, created_at FROM users WHERE login = ?")
        .bind(login)
        .fetch_optional(pool)
        .await?;

    if let Some(user_row) = row {
        Ok(Some(map_row_to_user(&user_row).expect("Error mapping User data")))
    } else {
        Ok(None)
    }
}

pub async fn create_session(pool: &SqlitePool, user_id: Uuid, duration_hours: f64) -> Result<(String, i64)> {
    let token = Uuid::new_v4().to_string();
    let now = Utc::now().timestamp();
    let expires_at = now + (duration_hours * 3600.0) as i64;

    sqlx::query("INSERT INTO sessions (token, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(&token)
        .bind(user_id)
        .bind(expires_at)
        .execute(pool)
        .await?;

    Ok((token, expires_at))
}

pub async fn validate_session(pool: &SqlitePool, token: &str, duration_hours: f64) -> Result<Option<User>> {
    let now = Utc::now().timestamp();

    let session_row = sqlx::query("SELECT user_id FROM sessions WHERE token = ? AND expires_at > ?")
        .bind(token)
        .bind(now)
        .fetch_optional(pool)
        .await?;

    if let Some(session_row) = session_row {
        let user_id: Uuid = session_row.try_get("user_id")?;
        let new_expires_at = now + (duration_hours * 3600.0) as i64;

        sqlx::query("UPDATE sessions SET expires_at = ? WHERE token = ?")
            .bind(new_expires_at)
            .bind(token)
            .execute(pool)
            .await?;

        let user_row = sqlx::query("SELECT id, chat_id, login, password, created_at FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(pool)
            .await?;

        return Ok(Some(map_row_to_user(&user_row)?));
    }
    Ok(None)
}

pub async fn get_user_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<User>> {
    let row = sqlx::query("SELECT id, chat_id, login, password, created_at FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;

    if let Some(user_row) = row {
        Ok(Some(map_row_to_user(&user_row).expect("Error mapping User data")))
    } else {
        Ok(None)
    }
}

pub async fn delete_user(pool: &SqlitePool, id: &Uuid) -> Result<bool> {
    let rows = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(rows.rows_affected() > 0)
}

pub fn validate_login(nick: &str) -> bool {
    let len = nick.len();
    if len < 3 || len > 32 {
        return false;
    }
    nick.chars().all(|char| char.is_ascii_alphanumeric() || char == '_')
}

fn map_row_to_user(row: &SqliteRow) -> Result<User> {
    let id = row.try_get("id")?;
    let chat_id: i64 = row.try_get("chat_id")?;
    let login: String = row.try_get("login")?;
    let password: String = row.try_get("password")?;
    let timestamp: i64 = row.try_get("created_at")?;
    let dt_utc = match DateTime::from_timestamp_secs(timestamp) {
        Some(dt) => dt,
        None => return Err(anyhow!("invalid timestamp")),
    };
    let created_at = dt_utc.with_timezone(&Local);

    Ok(User { id, chat_id, login, password, created_at })
}

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2.hash_password(password.as_bytes(), &salt)?;
    Ok(password_hash.to_string())
}

pub fn verify_password(password: &str, phc_hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(phc_hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default().verify_password(password.as_bytes(), &parsed_hash).is_ok()
}

pub async fn get_or_create_private_chat(pool: &SqlitePool, user1_id: &Uuid, user2_id: &Uuid) -> Result<Uuid> {
    let existing_chat: Option<(Uuid,)> = sqlx::query_as(
        "SELECT c.id FROM chats c
            JOIN chat_members cm1 ON c.id = cm1.chat_id AND cm1.user_id = ?
            JOIN chat_members cm2 ON c.id = cm2.chat_id AND cm2.user_id = ?
            WHERE c.type = 'private'"
    )
        .bind(user1_id)
        .bind(user2_id)
        .fetch_optional(pool)
        .await?;

    if let Some((chat_id,)) = existing_chat {
        return Ok(chat_id);
    }

    let chat_id = Uuid::new_v4();
    let now = Utc::now().timestamp();

    sqlx::query("INSERT INTO chats (id, type, created_at) VALUES (?, 'private', ?)")
        .bind(chat_id)
        .bind(now)
        .execute(pool)
        .await?;

    sqlx::query("INSERT INTO chat_members (chat_id, user_id, joined_at) VALUES (?, ?, ?)")
        .bind(chat_id)
        .bind(user1_id)
        .bind(now)
        .execute(pool)
        .await?;

    sqlx::query("INSERT INTO chat_members (chat_id, user_id, joined_at) VALUES (?, ?, ?)")
        .bind(chat_id)
        .bind(user2_id)
        .bind(now)
        .execute(pool)
        .await?;

    Ok(chat_id)
}

pub async fn save_chat_message(
    pool: &SqlitePool,
    message_id: &Uuid,
    chat_id: &Uuid,
    sender_id: &Uuid,
    content: &str,
) -> Result<()> {
    let now = Utc::now().timestamp();

    sqlx::query("INSERT INTO messages (id, chat_id, sender_id, content, timestamp, is_read) VALUES (?, ?, ?, ?, ?, 0)")
    .bind(message_id)
    .bind(chat_id)
    .bind(sender_id)
    .bind(content)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_chat_history(pool: &SqlitePool, chat_id: &Uuid, limit: i64) -> Result<Vec<(Uuid, Uuid, String, i64)>> {
    let rows = sqlx::query_as(
        "SELECT id, sender_id, content, timestamp FROM messages
            WHERE chat_id = ? ORDER BY timestamp DESC LIMIT ?"
    )
    .bind(chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

pub async fn mark_message_as_read(pool: &SqlitePool, message_id: &Uuid) -> Result<()> {
    sqlx::query("UPDATE messages SET is_read = 1 WHERE id = ?")
        .bind(message_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_user_by_chat_id(pool: &SqlitePool, chat_id: &i64) -> Result<Option<User>> {
    let row = sqlx::query("SELECT id, chat_id, login, password, created_at FROM users WHERE chat_id = ?")
        .bind(chat_id)
        .fetch_optional(pool)
        .await?;

    if let Some(user_row) = row {
        Ok(Some(map_row_to_user(&user_row)?))
    } else {
        Ok(None)
    }
}

pub async fn accept_friend_request(pool: &SqlitePool, user_id: &Uuid, friend_id: &Uuid) -> Result<()> {
    sqlx::query("UPDATE friends SET status = 'accepted' WHERE user_id = ? AND friend_id = ?")
        .bind(friend_id)
        .bind(user_id)
        .execute(pool)
        .await?;

    sqlx::query("INSERT OR IGNORE INTO friends (user_id, friend_id, status) VALUES (?, ?, 'accepted')")
        .bind(user_id)
        .bind(friend_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_friends_list(pool: &SqlitePool, user_id: &Uuid) -> Result<Vec<(i64, String)>> {
    let rows = sqlx::query(
        "SELECT u.chat_id, u.login FROM friends f
         JOIN users u ON f.friend_id = u.id
         WHERE f.user_id = ? AND f.status = 'accepted'"
    )
        .bind(user_id)
        .fetch_all(pool)
        .await?;

    let mut friends = Vec::new();
    for row in rows {
        let chat_id: i64 = row.try_get("chat_id")?;
        let login: String = row.try_get("login")?;
        friends.push((chat_id, login));
    }

    Ok(friends)
}

pub async fn get_pending_requests(pool: &SqlitePool, user_id: &Uuid) -> Result<Vec<(i64, String)>> {
    let rows = sqlx::query(
        "SELECT u.chat_id, u.login FROM friends f
        JOIN users u ON f.user_id = u.id
        WHERE f.friend_id = ? AND f.status = 'pending'"
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    let mut reqs = Vec::new();
    for row in rows {
        let chat_id: i64 = row.try_get("chat_id")?;
        let login: String = row.try_get("login")?;
        reqs.push((chat_id, login));
    }

    Ok(reqs)
}

pub async fn get_message_sender(pool: &SqlitePool, message_id: &Uuid) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT sender_id FROM messages WHERE id = ?")
        .bind(message_id)
        .fetch_optional(pool)
        .await?;

    if let Some(row) = row {
        let sender_id: Uuid = row.try_get("sender_id")?;
        let sender = get_user_by_id(pool, sender_id).await?;

        Ok(sender.map(|u| u.chat_id))
    } else {
        Ok(None)
    }
}

pub async fn add_friend_request(pool: &SqlitePool, user_id: &Uuid, friend_id: &Uuid) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO friends (user_id, friend_id, status) VALUES (?, ?, 'pending')")
        .bind(user_id)
        .bind(friend_id)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_db_operations() -> Result<()> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;

        migrate(&pool).await?;

        let login = "Alice";
        let password = "best_password_123_A!";

        let user = add_user(&pool, login, &password).await?;
        assert!(!user.id.is_nil());

        let fetched = get_user_by_login(&pool, login).await?;
        assert!(fetched.is_some());
        let user = fetched.unwrap();
        assert_eq!(user.login, login);

        let deleted = delete_user(&pool, &user.id).await?;
        assert!(deleted);

        let after_delete = get_user_by_login(&pool, login).await?;
        assert!(after_delete.is_none());

        Ok(())
    }
}