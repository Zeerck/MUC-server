use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::{fs, net::SocketAddr, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};
use uuid::{Uuid};

/// Структура пользователя, соответствует таблице `users`
#[derive(Debug)]
pub struct User {
    pub id: Uuid,
    pub address: String,
    pub nickname: String,
    pub created_at: DateTime<Local>,
}

/// Инициализирует пул соединений с SQLite (файл или in-memory)
pub fn init_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
    Ok(conn)
}

/// Возвращает путь к файлу БД, создавая все нужные папки.
/// Для Windows использует `%APPDATA%/LocalLow/Zeerck Inc/<app_name>/db/database.sqlite`
/// Для других ОС – `$XDG_DATA_HOME/zeerck-inc/<app_name>/db/database.sqlite`
pub fn get_db_path(app_name: &str) -> Result<PathBuf> {
    let base_dir = if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA")
            .context("APPDATA not set")?;
        PathBuf::from(appdata.replace("Roaming", "LocalLow"))
    } else {
        // Unix: XDG_DATA_HOME или ~/.local/share
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(xdg)
        } else {
            let home = std::env::var("HOME").context("HOME not set")?;
            PathBuf::from(home).join(".local/share")
        }
    };
    let db_dir = base_dir.join("Zeerck Inc").join(app_name).join("db");
    Ok(db_dir.join("database.sqlite"))
}

/// Создаёт таблицу users, если она не существует
pub fn migrate(connection: &Connection) -> Result<()> {
    connection.execute(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id BLOB PRIMARY KEY NOT NULL,
            address TEXT NOT NULL UNIQUE,
            nickname TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )
        "#,
        [],
    )?;

    Ok(())
}

/// Добавляет нового пользователя в базу
pub fn add_user(connection: &Connection, address: SocketAddr, nickname: &str) -> Result<User> {
    if !validate_nickname(nickname) {
        anyhow::bail!("invalid nickname: must be 3-32 letters, numbers or underscores");
    }

    let id = Uuid::new_v4();
    let address_str = address.ip().to_string();
    // let now = Local::now().naive_local().format(DATE_FORMAT).to_string();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("System time before epoch").as_secs() as i64;

    connection.execute(
        "INSERT INTO users (id, address, nickname, created_at) VALUES (?, ?, ?, ?)",
        params![&id, &address_str, &nickname, &now],
    )?;

    let user = connection.query_row(
        "SELECT id, address, nickname, created_at FROM users WHERE id = ?",
        params![&id],
        map_row_to_user,
    )?;

    Ok(user)
}

/// Получает пользователя по никнейму
pub fn get_user_by_nickname(connection: &Connection, nickname: &str) -> Result<Option<User>> {
    let mut stmt = connection
        .prepare("SELECT id, address, nickname, created_at FROM users WHERE nickname = ?")?;
    let user = stmt
        .query_row(params![nickname], map_row_to_user)
        .optional()?;

    Ok(user)
}

/// Получает пользователя по адресу
pub fn get_user_by_address(connection: &Connection, address: &SocketAddr) -> Result<Option<User>> {
    let address_str = address.ip().to_string();
    let mut stmt = connection
        .prepare("SELECT id, address, nickname, created_at FROM users WHERE address = ?")?;
    let user = stmt
        .query_row(params![&address_str], map_row_to_user)
        .optional()?;

    Ok(user)
}

/// Возвращает всех пользователей в базе
pub fn get_all_users(connection: &Connection) -> Result<Vec<User>> {
    let mut stmt = connection
        .prepare("SELECT id, address, nickname, created_at FROM users ORDER BY created_at")?;
    let users = stmt
        .query_map([], map_row_to_user)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(users)
}

/// Удаляет пользователя по ID
pub fn delete_user(connection: &Connection, id: &Uuid) -> Result<bool> {
    let rows = connection.execute("DELETE FROM users where id = ?", params![id])?;
    Ok(rows > 0)
}

pub fn validate_nickname(nick: &str) -> bool {
    let len = nick.len();
    if len < 3 || len > 32 {
        return false;
    }

    nick.chars()
        .all(|char| char.is_ascii_alphanumeric() || char == '_')
}

fn map_row_to_user(row: &Row) -> rusqlite::Result<User> {
    let id: Uuid = row.get(0)?;
    let address: String = row.get(1)?;
    let nickname: String = row.get(2)?;
    let timestamp: i64 = row.get(3)?;
    let dt_utc = match chrono::DateTime::from_timestamp_secs(timestamp) {
        Some(dt) => dt,
        None => return Err(rusqlite::Error::FromSqlConversionFailure(
           3, rusqlite::types::Type::Integer,
           Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid timestamp")), 
        )),
    };
    let created_at = dt_utc.with_timezone(&Local);
    // let naive = NaiveDateTime::parse_from_str(&created_at_str, DATE_FORMAT)
    //     .map_err(|e| rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e)))?;
    // let created_at = Local.from_local_datetime(&naive).single().unwrap();
    Ok(User {
        id,
        address,
        nickname,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn test_db_operations() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        let nickname = "Alice";

        let user = add_user(&conn, addr, nickname)?;
        assert!(!user.id.is_nil());

        let fetched = get_user_by_nickname(&conn, nickname)?;
        assert!(fetched.is_some());
        let user = fetched.unwrap();
        assert_eq!(user.nickname, nickname);
        assert_eq!(user.address, addr.ip().to_string());

        let all = get_all_users(&conn)?;
        assert_eq!(all.len(), 1);

        let deleted = delete_user(&conn, &user.id)?;
        assert!(deleted);

        let after_delete = get_user_by_nickname(&conn, nickname)?;
        assert!(after_delete.is_none());

        Ok(())
    }
}
