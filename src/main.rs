mod commands;
mod config;
mod db;
mod logger;

use commands::Command;
use dotenvy::dotenv;

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    sync::{Arc, LazyLock, Mutex},
    thread,
};

use crate::{commands::handle_command, config::Config};

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config::from_env());

#[derive(Debug, PartialEq)]
enum AuthType {
    Register,
    Login,
}

#[derive(Debug)]
struct HandshakeData {
    auth_type: AuthType,
    login: String,
    password: String,
    address: SocketAddr,
}

fn main() {
    dotenv().ok();
    logger::init(&CONFIG.app_name, logger::LogLevel::Trace);

    let _ = ctrlc::set_handler(move || {
        info!("Program exit with CTRL+C");
    });

    let listener = TcpListener::bind(&CONFIG.server_address).expect("Failed to bind listener");
    info!("Server listening on {}", &CONFIG.server_address);
    info!("Database path: {}", CONFIG.db_path.display());

    let connection = db::init_database(&CONFIG.db_path).expect("Failed to open database");

    if let Err(e) = db::migrate(&connection) {
        fatal!("Migration failed: {e}");
        return;
    }

    let db_arc = Arc::new(Mutex::new(connection));

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                let db_clone = db_arc.clone();
                thread::spawn(move || {
                    handle_client(stream, db_clone);
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}

fn handle_client(mut stream: TcpStream, db: Arc<Mutex<rusqlite::Connection>>) {
    let peer_address = match stream.peer_addr() {
        Ok(address) => address,
        Err(e) => {
            error!("Failed to get peer address: {e}");
            return;
        }
    };

    trace!("Handling connection from: {peer_address}");

    let user_handshake_data = match get_user_handshake_data(&mut stream, peer_address) {
        Ok(data) => data,
        Err(e) => {
            warning!("Handshake failed for {peer_address}: {e}");
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    };

    let connection = db.lock().unwrap();

    let user_result = match user_handshake_data.auth_type {
        AuthType::Register => register_user(
            &connection,
            &mut stream,
            &user_handshake_data.login,
            &user_handshake_data.password,
            user_handshake_data.address,
        ),
        AuthType::Login => login_user(
            &connection,
            &mut stream,
            &user_handshake_data.login,
            &user_handshake_data.password,
            user_handshake_data.address,
        ),
    };

    let user = match user_result {
        Ok(u) => {
            info!("User '{}' successfully authenticated", u.login);
            u
        }
        Err(e) => {
            warning!("Authentication failed for {peer_address}: {e}");
            // Ошибку уже отправили клиенту внутри функций register/login
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    };

    // Сбрасываем таймаут на стандартное значение для чтения сообщений
    let _ = stream.set_read_timeout(Some(CONFIG.read_timeout));

    let user_login = user.login.clone();
    info!("User {user_login} ({peer_address}) entered main loop");

    let mut buffer = [0; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                trace!("Client {peer_address} ({user_login}) closed connection");
                break;
            }
            Ok(n) => {
                let msg = from_utf8(&buffer[0..n])
                    .unwrap_or("Invalid UTF-8")
                    .trim_end();
                info!("Message from {user_login} ({peer_address}): {msg}");

                if let Some(command) = Command::from_str(msg) {
                    // Передаем ссылку, а не клон
                    let should_continue = handle_command(&stream, command);

                    if !should_continue {
                        break;
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                warning!("Error reading from {peer_address} ({user_login}): {e}");
                break;
            }
        }
    }
    trace!("Connection finished for: {peer_address} ({user_login})");
}

fn register_user(
    connection: &rusqlite::Connection,
    stream: &mut TcpStream,
    login: &str,
    password: &str,
    address: SocketAddr,
) -> Result<db::User, anyhow::Error> {
    trace!("Checking if user exists in database...");
    if db::get_user_by_login(connection, login)?.is_some() {
        let _ = stream.write_all(b"Login already taken! Disconnecting.\n");
        let _ = stream.shutdown(Shutdown::Both);
        anyhow::bail!("Login already taken!");
    }

    trace!("Trying add user with login: '{login}' and address: {address} to Database...");
    let user = db::add_user(connection, login, password)?;
    trace!(
        "User inserted with ID: '{}' and login '{}'",
        user.id, user.login
    );
    Ok(user)
}

fn login_user(
    connection: &rusqlite::Connection,
    stream: &mut TcpStream,
    login: &str,
    password: &str,
    address: SocketAddr,
) -> Result<db::User, anyhow::Error> {
    trace!("Trying to find user '{login}' ({address}) in Database...");

    if let Some(existing) = db::get_user_by_login(connection, login)? {
        if db::verify_password(password, &existing.password) {
            return Ok(existing);
        }
    }

    let _ = stream.write_all(b"Wrong login or password! Disconnecting.\n");
    let _ = stream.shutdown(Shutdown::Both);
    anyhow::bail!("Wrong login or password!")
}

fn get_user_handshake_data(
    stream: &mut TcpStream,
    address: SocketAddr,
) -> Result<HandshakeData, anyhow::Error> {
    stream.set_read_timeout(Some(CONFIG.handshake_timeout))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();

    match reader.read_line(&mut line) {
        Ok(0) => anyhow::bail!("Client disconnected before sending handshake"),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::TimedOut => {
            anyhow::bail!("Handshake timeout")
        }
        Err(e) => anyhow::bail!("Failed to read handshake: {}", e),
    }

    let line = line.trim();
    trace!("Received handshake: {}", line);

    let mut parts = line.splitn(3, ' ');

    let cmd = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Empty handshake"))?;
    let login = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing login in handshake"))?;
    let password = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing password in handshake"))?;

    let auth_type = match cmd.to_uppercase().as_str() {
        "REGISTER" => AuthType::Register,
        "LOGIN" => AuthType::Login,
        _ => {
            let _ = stream.write_all(b"Invalid handshake format. Use: REGISTER <login> <password> or LOGIN <login> <password>\n");
            let _ = stream.shutdown(Shutdown::Both);
            anyhow::bail!("Invalid handshake command: {}", cmd);
        }
    };

    Ok(HandshakeData {
        auth_type,
        login: login.to_string(),
        password: password.to_string(),
        address,
    })
}
