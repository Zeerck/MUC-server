mod commands;
mod config;
mod db;
mod logger;

use commands::Command;
use dotenvy::dotenv;

use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    sync::{Arc, LazyLock, Mutex},
    thread,
};

use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;

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

fn load_tls_config() -> Arc<ServerConfig> {
    let cert_file = &mut BufReader::new(File::open(&CONFIG.tls_cert_path).expect("Failed to open cert file"));
    let key_file = &mut BufReader::new(File::open(&CONFIG.tls_key_path).expect("Failed to open key file"));

    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_reader_iter(cert_file)
        .collect::<Result<_, _>>()
        .expect("Failed to parse certs");
        
    let key: PrivateKeyDer<'static> = PrivateKeyDer::from_pem_reader(key_file)
        .expect("Failed to parse key");

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("Failed to build TLS config");

    Arc::new(config)
}

fn main() {
    dotenv().ok();
    logger::init("MUC-server", logger::LogLevel::Trace);

    let _ = ctrlc::set_handler(move || {
        info!("Program exit with CTRL+C");
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::process::exit(0);
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
    let tls_config = load_tls_config();

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                let db_clone = db_arc.clone();
                let tls_config_clone = tls_config.clone();
                thread::spawn(move || {
                    handle_client(stream, db_clone, tls_config_clone);
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}

fn handle_client(mut stream: TcpStream, db: Arc<Mutex<rusqlite::Connection>>, tls_config: Arc<ServerConfig>) {
    let peer_address = match stream.peer_addr() {
        Ok(address) => address,
        Err(e) => {
            error!("Failed to get peer address: {e}");
            return;
        }
    };

    trace!("Handling connection from: {peer_address}");

    let _ = stream.set_read_timeout(Some(CONFIG.handshake_timeout));

    let mut conn = match rustls::ServerConnection::new(tls_config) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to create TLS connection: {e}");
            return;
        }
    };

    if let Err(e) = conn.complete_io(&mut stream) {
        warning!("TLS handshake failed for {peer_address}: {e}");
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }

    let mut tls_stream = rustls::StreamOwned::new(conn, stream);

    let user_handshake_data = match get_user_handshake_data(&mut tls_stream, peer_address) {
        Ok(data) => data,
        Err(e) => {
            warning!("Handshake failed for {peer_address}: {e}");
            let _ = tls_stream.write_all(e.to_string().as_bytes());
            let _ = tls_stream.flush();
            let _ = tls_stream.sock.shutdown(Shutdown::Both);
            return;
        }
    };

    let connection = db.lock().unwrap();

    let user_result = match user_handshake_data.auth_type {
        AuthType::Register => register_user(&connection, &user_handshake_data.login, &user_handshake_data.password, peer_address),
        AuthType::Login => login_user(&connection, &user_handshake_data.login, &user_handshake_data.password, peer_address),
    };

    let user = match user_result {
        Ok(u) => {
            info!("User '{}' successfully authenticated", u.login);
            u
        }
        Err(err_msg) => {
            warning!("Authentication failed for {peer_address}: {}", err_msg);
            let _ = tls_stream.write_all(format!("{}\n", err_msg).as_bytes());
            let _ = tls_stream.flush();
            let _ = tls_stream.sock.shutdown(Shutdown::Both);
            return;
        }
    };

    let _ = tls_stream.get_ref().set_read_timeout(Some(CONFIG.read_timeout));

    let user_login = user.login.clone();
    info!("User {user_login} ({peer_address}) entered main loop");

    let mut buffer = [0; 1024];
    loop {
        match tls_stream.read(&mut buffer) {
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
                    let should_continue = handle_command(&mut tls_stream, command);
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
    
    let _ = tls_stream.conn.send_close_notify();
    let _ = tls_stream.flush();
    let _ = tls_stream.sock.shutdown(Shutdown::Both);
    
    trace!("Connection finished for: {peer_address} ({user_login})");
}

fn register_user(
    connection: &rusqlite::Connection,
    login: &str,
    password: &str,
    address: SocketAddr,
) -> Result<db::User, String> {
    trace!("Checking if user exists in database...");
    if db::get_user_by_login(connection, login).map_err(|e| e.to_string())?.is_some() {
        return Err("Login already taken! Disconnecting.".to_string());
    }

    trace!("Trying add user with login: '{login}' and address: {address} to Database...");
    let user = db::add_user(connection, login, password).map_err(|e| e.to_string())?;
    trace!("User inserted with ID: '{}' and login '{}'", user.id, user.login);
    Ok(user)
}

fn login_user(
    connection: &rusqlite::Connection,
    login: &str,
    password: &str,
    address: SocketAddr,
) -> Result<db::User, String> {
    trace!("Trying to find user '{login}' ({address}) in Database...");

    if let Some(existing) = db::get_user_by_login(connection, login).map_err(|e| e.to_string())? {
        if db::verify_password(password, &existing.password) {
            return Ok(existing);
        }
    }

    Err("Wrong login or password! Disconnecting.".to_string())
}

fn get_user_handshake_data<S: Read + Write>(
    stream: &mut S,
    address: SocketAddr,
) -> Result<HandshakeData, String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    match reader.read_line(&mut line) {
        Ok(0) => return Err("Client disconnected before sending handshake\n".to_string()),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::TimedOut => return Err("Handshake timeout\n".to_string()),
        Err(e) => return Err(format!("Failed to read handshake: {}\n", e)),
    }

    let line = line.trim();
    trace!("Received handshake: {}", line);

    let mut parts = line.splitn(3, ' ');
    
    let cmd = parts.next().ok_or("Empty handshake\n".to_string())?;
    let login = parts.next().ok_or("Missing login in handshake\n".to_string())?;
    let password = parts.next().ok_or("Missing password in handshake\n".to_string())?;

    let auth_type = match cmd.to_uppercase().as_str() {
        "REGISTER" => AuthType::Register,
        "LOGIN" => AuthType::Login,
        _ => return Err("Invalid handshake command\n".to_string()),
    };

    Ok(HandshakeData {
        auth_type,
        login: login.to_string(),
        password: password.to_string(),
        address,
    })
}