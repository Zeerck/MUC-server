use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use dirs;
use sqlx::{FromRow, SqlitePool};
use std::{fs, net::SocketAddr, path::PathBuf};
use uuid::Uuid;

/// Структура пользователя, соответствует таблице `users`
#[derive(Debug, FromRow)]
pub struct User {
    pub id: Uuid,
    pub address: String,
    pub nickname: String,
    pub created_at: DateTime<Local>,
}

/// Инициализирует пул соединений с SQLite (файл или in-memory)
pub async fn init_database(app_name: &str) -> Result<SqlitePool> {
    let db_path = get_db_path(app_name)?;
    let db_url = &format!("sqlite:{}?mode=rwc", db_path.to_str().unwrap()).to_string();

    let pool = SqlitePool::connect(db_url).await?;
    // Включаем поддержку UUID в SQLite (сохраняется как BLOB)
    sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&pool)
        .await?;
    Ok(pool)
}

/// Возвращает путь к файлу БД, создавая все нужные папки.
/// Для Windows использует `%APPDATA%/LocalLow/Zeerck Inc/<app_name>/db/database.sqlite`
/// Для других ОС – `$XDG_DATA_HOME/zeerck-inc/<app_name>/db/database.sqlite`
pub fn get_db_path(app_name: &str) -> Result<PathBuf> {
    let base_dir = if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").context("APPDATA environment variable not found")?;
        // Заменяем "Roaming" на "LocalLow"
        let local_low = appdata.replace("Roaming", "LocalLow");
        PathBuf::from(local_low)
    } else {
        dirs::data_local_dir().context("Could not determine local data directory")?
    };

    let db_dir = base_dir.join("Zeerck Inc").join(app_name).join("db");

    fs::create_dir_all(&db_dir)
        .with_context(|| format!("Failed to create directory: {:?}", db_dir))?;

    Ok(db_dir.join("database.sqlite"))
}

/// Создаёт таблицу users, если она не существует
pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id BLOB PRIMARY KEY NOT NULL,
            address TEXT NOT NULL UNIQUE,
            nickname TEXT NOT NULL,
            created_at TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Добавляет нового пользователя в базу
pub async fn add_user(pool: &SqlitePool, address: SocketAddr, nickname: &str) -> Result<User> {
    let id = Uuid::new_v4();
    let address_str = address.ip().to_string();
    let now = Local::now().to_string();

    sqlx::query("INSERT INTO users (id, address, nickname, created_at) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(&address_str)
        .bind(nickname)
        .bind(&now)
        .execute(pool)
        .await?;

    let user = sqlx::query_as::<_, User>(
        "SELECT id, address, nickname, created_at FROM users WHERE id =?",
    )
        .bind(&id)
        .fetch_one(pool)
        .await?;

    Ok(user)
}

/// Получает пользователя по никнейму
pub async fn get_user_by_nickname(pool: &SqlitePool, nickname: &str) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, address, nickname, created_at FROM users WHERE nickname = ?",
    )
    .bind(nickname)
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

/// Получает пользователя по адресу
pub async fn get_user_by_address(pool: &SqlitePool, address: &SocketAddr) -> Result<Option<User>> {
    let address_str = address.ip().to_string();
    let user = sqlx::query_as::<_, User>(
        "SELECT id, address, nickname, created_at FROM users WHERE address = ?",
    )
    .bind(&address_str)
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

/// Возвращает всех пользователей в базе
pub async fn get_all_users(pool: &SqlitePool) -> Result<Vec<User>> {
    let users = sqlx::query_as::<_, User>(
        "SELECT id, address, nickname, created_at FROM users ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?;
    Ok(users)
}

/// Удаляет пользователя по ID
pub async fn delete_user(pool: &SqlitePool, id: &Uuid) -> Result<bool> {
    let rows = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(rows > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[tokio::test]
    async fn test_db_operations() {
        let pool = init_database("sqlite::memory:").await.unwrap();
        migrate(&pool).await.unwrap();

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        let nickname = "Alice";

        let user = add_user(&pool, addr, nickname).await.unwrap();
        assert!(!user.id.is_nil());

        let fetched = get_user_by_nickname(&pool, nickname).await.unwrap();
        assert!(fetched.is_some());
        let user = fetched.unwrap();
        assert_eq!(user.nickname, nickname);
        assert_eq!(user.address, addr.to_string());

        let all = get_all_users(&pool).await.unwrap();
        assert_eq!(all.len(), 1);

        let deleted = delete_user(&pool, &user.id).await.unwrap();
        assert!(deleted);

        let after_delete = get_user_by_nickname(&pool, nickname).await.unwrap();
        assert!(after_delete.is_none());
    }
}