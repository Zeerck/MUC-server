mod config;
mod db;
mod logger;
mod hub;

use dotenvy::dotenv;

use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    net::{IpAddr, SocketAddr},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio::sync::mpsc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};

use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use sha2::{Digest, Sha256};
use rand::RngExt;
use crate::config::Config;
use crate::hub::Hub;

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
}

struct RateLimiter {
    attempts: HashMap<IpAddr, Vec<Instant>>,
    window: Duration,
    max_attempts: usize,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            attempts: HashMap::new(),
            window: Duration::from_secs(300),
            max_attempts: 5,
        }
    }

    fn is_blocked(&self, ip: &IpAddr) -> bool {
        if let Some(times) = self.attempts.get(ip) {
            let now = Instant::now();
            let recent: Vec<_> = times.iter().filter(|&&t| now.duration_since(t) < self.window).collect();
            return recent.len() >= self.max_attempts;
        }
        false
    }

    fn record_failure(&mut self, ip: IpAddr) {
        let now = Instant::now();
        let times = self.attempts.entry(ip).or_insert_with(Vec::new);
        times.push(now);
        times.retain(|&t| now.duration_since(t) < self.window);
    }

    fn clear_attempts(&mut self, ip: &IpAddr) {
        self.attempts.remove(ip);
    }
}

fn load_tls_config() -> Arc<ServerConfig> {
    let cert_file = &mut BufReader::new(File::open(&CONFIG.tls_cert_path).expect("Failed to open cert file"));
    let key_file = &mut BufReader::new(File::open(&CONFIG.tls_key_path).expect("Failed to open key file"));

    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_reader_iter(cert_file)
        .collect::<Result<_, _>>()
        .expect("Failed to parse certs");

    let key: PrivateKeyDer<'static> = PrivateKeyDer::from_pem_reader(key_file).expect("Failed to parse key");

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("Failed to build TLS config");

    Arc::new(config)
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    logger::init("MUC-server", logger::LogLevel::Trace);

    let _ = ctrlc::set_handler(move || {
        info!("Program exit with CTRL+C");
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::process::exit(0);
    });

    let listener = TcpListener::bind(&CONFIG.server_address).await.expect("Failed to bind listener");
    info!("Server listening on {}", &CONFIG.server_address);
    info!("Database path: {}", CONFIG.db_path.display());

    let connection = db::init_database(&CONFIG.db_path).expect("Failed to open database");

    if let Err(e) = db::migrate(&connection) {
        fatal!("Migration failed: {e}");
        return;
    }

    let db_arc = Arc::new(Mutex::new(connection));
    let tls_acceptor = TlsAcceptor::from(load_tls_config());

    let fake_hash = db::hash_password("fake_password_for_timing_attack").expect("Failed to generate fake hash");
    let fake_hash_arc = Arc::new(fake_hash);

    let rate_limiter = Arc::new(Mutex::new(RateLimiter::new()));
    let hub: Hub = Hub::new();

    loop {
        match listener.accept().await {
            Ok((stream, peer_address)) => {
                let db_clone = db_arc.clone();
                let tls_acceptor_clone = tls_acceptor.clone();
                let fake_hash_clone = fake_hash_arc.clone();
                let rate_limiter_clone = rate_limiter.clone();
                let hub_clone = hub.clone();

                tokio::spawn(async move {
                    handle_client(
                        stream,
                        peer_address,
                        db_clone,
                        tls_acceptor_clone,
                        fake_hash_clone,
                        rate_limiter_clone,
                        hub_clone
                    ).await;
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}

async fn handle_client(
    stream: TcpStream,
    peer_address: SocketAddr,
    db: Arc<Mutex<rusqlite::Connection>>,
    tls_acceptor: TlsAcceptor,
    fake_hash: Arc<String>,
    rate_limiter: Arc<Mutex<RateLimiter>>,
    hub: Hub,
) {
    let peer_ip = peer_address.ip();

    {
        let limiter = rate_limiter.lock().unwrap();
        if limiter.is_blocked(&peer_ip) {
            warning!("Connection from {peer_address} blocked due to rate limit");
            return;
        }
    }

    trace!("Handling connection from: {peer_address}");

    stream.set_nodelay(true).ok();

    let mut tls_stream = match tls_acceptor.accept(stream).await {
        Ok(s) => s,
        Err(e) => {
            warning!("TLS handshake failed for {peer_address}: {e}");
            return;
        }
    };

    let user_handshake_data = match get_user_handshake_data_async(&mut tls_stream, peer_address).await {
        Ok(data) => data,
        Err(e) => {
            warning!("Auth handshake failed for {peer_address}: {e}");
            let _ = tls_stream.write_all(e.to_string().as_bytes()).await;
            let _ = tls_stream.flush().await;
            return;
        }
    };

    let (user_result, undelivered_messages) = {
        let connection = db.lock().unwrap();
        let result = match user_handshake_data.auth_type {
            AuthType::Register => register_user(&connection, &user_handshake_data.login, &user_handshake_data.password),
            AuthType::Login => login_user(&connection, &user_handshake_data.login, &user_handshake_data.password, &fake_hash),
        };

        let messages = if let Ok(ref u) = result {
            db::get_undelivered_messages(&connection, &u.id).unwrap_or_default()
        } else {
            Vec::new()
        };

        (result, messages)
    };

    let user = match user_result {
        Ok(u) => {
            info!("User '{}' successfully authenticated", u.login);
            rate_limiter.lock().unwrap().clear_attempts(&peer_ip);
            u
        },
        Err(error_message) => {
            warning!("Authentication failed for {peer_address}: {error_message}");
            rate_limiter.lock().unwrap().record_failure(peer_ip);
            // ТЕПЕРЬ пишем в сокет безопасно, никаких локов БД не удерживается
            let _ = tls_stream.write_all(format!("{}\n", error_message).as_bytes()).await;
            let _ = tls_stream.flush().await;
            return;
        }
    };

    for (sender_login, message) in undelivered_messages {
        let offline_message = format!("HISTORY {} {}\n", sender_login, message);
        let _ = tls_stream.write_all(offline_message.as_bytes()).await;
    }

    let _ = tls_stream.flush().await;

    let (reader, mut writer) = tokio::io::split(tls_stream);
    let mut reader = AsyncBufReader::new(reader);

    let user_login = user.login.clone();
    let user_id = user.id.clone();
    info!("User {user_login} ({peer_address}) entered main loop");

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    hub.register(user_id, tx);

    let write_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if writer.write_all(message.as_bytes()).await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
    });

    let mut buf = String::new();

    loop {
        buf.clear();
        match tokio::time::timeout(CONFIG.read_timeout, reader.read_line(&mut buf)).await {
            Ok(Ok(0)) => {
                trace!("Client {peer_address} ({user_login}) closed connection");
                break;
            }
            Ok(Ok(_)) => {
                let message = buf.trim().to_string();

                if message.is_empty() {
                    continue;
                }

                info!("Message from {user_login} ({peer_address}): {message})");

                if let Some(rest) = message.strip_prefix("SEND_MSG ") {
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();

                    if parts.len() == 2 {
                        let target_login = parts[0];
                        let message_content = parts[1];

                        let target_user_id = {
                            let conn = db.lock().unwrap();
                            if let Ok(Some(target_user)) = db::get_user_by_login(&conn, target_login) {
                                let _ = db::save_message(&conn, &user_id, &target_user.id, message_content);
                                Some(target_user.id)
                            } else {
                                None
                            }
                        };

                        if let Some(target_id) = target_user_id {
                            let formatted_msg = format!("RECV_MSG {} {}\n", user_login, message_content);

                            if !hub.send_to(&target_id, &formatted_msg) {
                                let _ = hub.send_to(&user_id, &format!("INFO User '{}' is offline. Message saved.\n", target_login));
                            } else {
                                let _ = hub.send_to(&user_id, &format!("INFO Message delivered to {}.\n", target_login));
                            }
                        } else {
                            let _ = hub.send_to(&user_id, &format!("ERROR User '{}' not found.\n", target_login));
                        }
                    } else {
                        let _ = hub.send_to(&user_id, "ERROR Invalid format. Use: SEND_MSG <login> <message>\n");
                    }
                } else {
                    let _ = hub.send_to(&user_id, "ERROR Unknown command.\n");
                }
            }
            Ok(Err(e)) => {
                warning!("Error reading from {peer_address} ({user_login}): {e}");
                break;
            }
            Err(_) => {
                warning!("Read timeout for {peer_address} ({user_login})");
                break;
            }
        }
    }

    hub.unregister(&user_id);
    write_task.abort();
    trace!("Connection finished for: {peer_address} ({user_login})");
}

fn register_user(
    connection: &rusqlite::Connection,
    login: &str,
    password: &str,
) -> Result<db::User, String> {
    match db::add_user(connection, login, password) {
        Ok(user) => Ok(user),
        Err(e) => {
            warning!("Registration failed for {login}: {e}");
            Err("Registration failed. Check login format and password strength.".to_string())
        }
    }
}

fn login_user(
    connection: &rusqlite::Connection,
    login: &str,
    password: &str,
    fake_hash: &str,
) -> Result<db::User, String> {
    if let Some(existing) = db::get_user_by_login(connection, login).map_err(|e| e.to_string())? {
        if db::verify_password(password, &existing.password) {
            return Ok(existing);
        }
    } else {
        let _ = db::verify_password(password, fake_hash);
    }
    Err("Wrong login or password! Disconnecting.".to_string())
}

async fn get_user_handshake_data_async<S: AsyncBufReadExt + AsyncWriteExt + Unpin>(
    stream: &mut S,
    address: SocketAddr,
) -> Result<HandshakeData, String> {
    let mut line = String::new();

    match tokio::time::timeout(CONFIG.handshake_timeout, stream.read_line(&mut line)).await {
        Ok(Ok(0)) => return Err("Client disconnected before sending handshake\n".to_string()),
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(format!("Failed to read handshake: {e}\n")),
        Err(_) => return Err("Handshake timeout\n".to_string()),
    }

    let line = line.trim();
    trace!("Received handshake from {address}: {line}");

    let mut parts = line.splitn(3, ' ');
    let command = parts.next().ok_or("Missing command\n".to_string())?;
    let login = parts.next().ok_or("Missing login\n".to_string())?;
    let password = parts.next().ok_or("Missing password\n".to_string())?;

    let auth_type = match command.to_uppercase().as_str() {
        "REGISTER" => AuthType::Register,
        "LOGIN" => AuthType::Login,
        _ => return Err("Invalid command\n".to_string()),
    };

    if auth_type == AuthType::Register {
        let challenge: String = (0..16)
            .map(|_| format!("{:x}", rand::rng().random_range(0..16)))
            .collect();

        let difficulty = CONFIG.pow_difficulty;

        stream.write_all(format!("SOLVE {} {}\n", challenge, difficulty).as_bytes())
            .await.map_err(|e| format!("Write error: {e}\n"))?;
        stream.flush().await.map_err(|e| format!("Flush error: {e}\n"))?;

        let mut solve_line = String::new();
        match tokio::time::timeout(CONFIG.read_timeout, stream.read_line(&mut solve_line)).await {
            Ok(Ok(0)) => return Err("Client disconnected before sending PoW\n".to_string()),
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(format!("Failed to read PoW: {e}\n")),
            Err(_) => return Err("PoW timeout\n".to_string()),
        }

        let solve_line = solve_line.trim();

        if let Some(nonce_str) = solve_line.strip_prefix("SOLVED ") {
            if let Ok(nonce) = nonce_str.parse::<u64>() {
                if !verify_pow(&challenge, nonce, difficulty) {
                    return Err("Invalid PoW solution\n".to_string());
                }
            } else {
                return Err("Invalid PoW format\n".to_string());
            }
        } else {
            return Err("Did not receive PoW solution\n".to_string());
        }
    }

    Ok(HandshakeData {
        auth_type,
        login: login.to_string(),
        password: password.to_string(),
    })
}

fn verify_pow(challenge: &str, nonce: u64, difficulty: usize) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(challenge.as_bytes());
    hasher.update(nonce.to_string().as_bytes());
    let result = hasher.finalize();

    match difficulty {
        4 => result[30] == 0 && result[31] == 0,
        5 => result[30] == 0 && result[31] == 0 && result[29] < 0x10,
        6 => result[29] == 0 && result[30] == 0 && result[31] == 0,
        _ => false,
    }
}