mod commands;
mod config;
mod db;
mod logger;

use commands::Command;
use dotenvy::dotenv;

use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream},
    str::from_utf8,
    sync::{Arc, LazyLock, Mutex},
    thread,
    time::{Duration, Instant},
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

// Простая структура для Rate Limiting
struct RateLimiter {
    attempts: HashMap<IpAddr, Vec<Instant>>,
    window: Duration,
    max_attempts: usize,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            attempts: HashMap::new(),
            window: Duration::from_secs(300), // 5 минут
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
        // Чистим старые записи, чтобы не течь по памяти
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
    
    // Генерируем фейковый хэш для защиты от Timing Attacks
    let fake_hash = db::hash_password("fake_password_for_timing_attack").expect("Failed to generate fake hash");
    let fake_hash_arc = Arc::new(fake_hash);
    
    let rate_limiter = Arc::new(Mutex::new(RateLimiter::new()));

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                let db_clone = db_arc.clone();
                let tls_config_clone = tls_config.clone();
                let fake_hash_clone = fake_hash_arc.clone();
                let rate_limiter_clone = rate_limiter.clone();
                
                thread::spawn(move || {
                    handle_client(stream, db_clone, tls_config_clone, fake_hash_clone, rate_limiter_clone);
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}

fn handle_client(
    mut stream: TcpStream, 
    db: Arc<Mutex<rusqlite::Connection>>, 
    tls_config: Arc<ServerConfig>,
    fake_hash: Arc<String>,
    rate_limiter: Arc<Mutex<RateLimiter>>,
) {
    let peer_address = match stream.peer_addr() {
        Ok(address) => address,
        Err(e) => {
            error!("Failed to get peer address: {e}");
            return;
        }
    };

    let peer_ip = peer_address.ip();

    // 0. Rate Limit Check
    {
        let limiter = rate_limiter.lock().unwrap();
        if limiter.is_blocked(&peer_ip) {
            warning!("Connection from {peer_address} blocked due to rate limit");
            // Просто рвем соединение без объяснений
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    }

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
        AuthType::Login => login_user(&connection, &user_handshake_data.login, &user_handshake_data.password, peer_address, &fake_hash),
    };

    let user = match user_result {
        Ok(u) => {
            info!("User '{}' successfully authenticated", u.login);
            // Сбрасываем счетчик неудачных попыток при успехе
            rate_limiter.lock().unwrap().clear_attempts(&peer_ip);
            u
        }
        Err(err_msg) => {
            warning!("Authentication failed for {peer_address}: {}", err_msg);
            // Записываем неудачную попытку
            rate_limiter.lock().unwrap().record_failure(peer_ip);
            
            // Отправляем универсальную ошибку клиенту
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
    
    // Проверяем сложность пароля до проверки существования логина,
    // чтобы не выдавать, занят логин или нет, если пароль все равно мусор.
    match db::add_user(connection, login, password) {
        Ok(user) => {
            trace!("User inserted with ID: '{}' and login '{}'", user.id, user.login);
            Ok(user)
        }
        Err(e) => {
            // Если ошибка из-за уникальности логина (SQLite возвращает SQLITE_CONSTRAINT_UNIQUE)
            // или из-за слабого пароля, мы возвращаем одинаковую нейтральную ошибку.
            // Не светим, что именно пошло не так.
            warning!("Registration failed for '{}': {}", login, e);
            Err("Registration failed. Check login format and password strength.".to_string())
        }
    }
}

fn login_user(
    connection: &rusqlite::Connection,
    login: &str,
    password: &str,
    address: SocketAddr,
    fake_hash: &str,
) -> Result<db::User, String> {
    trace!("Trying to find user '{login}' ({address}) in database...");

    if let Some(existing) = db::get_user_by_login(connection, login).map_err(|e| e.to_string())? {
        if db::verify_password(password, &existing.password) {
            return Ok(existing);
        }
    } else {
        // TIMING ATTACK MITIGATION
        // Если логина нет в базе, мы всё равно вызываем verify_password для фейкового хэша.
        // Это занимает те же ~50-100мс, что и проверка реального пароля.
        // Атакующий не сможет понять по времени ответа, существует ли логин.
        let _ = db::verify_password(password, fake_hash);
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