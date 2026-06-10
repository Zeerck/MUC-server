use chrono::Utc;
use colored::Colorize;
use std::fmt::{self, Write};

// в lib.rs или logger.rs
pub mod prelude {
    pub use super::log;
    pub use super::LogLevel::*;
}

pub struct Logger;

impl Logger {
    /// Логирует сообщение с указанным уровнем.
    ///
    /// # Пример
    /// ```
    /// use my_crate::{Logger, LogLevel};
    /// Logger::log("Сервис запущен", LogLevel::Info);
    /// ```
    pub fn log(content: impl AsRef<str>, log_level: LogLevel) {
        let date = Utc::now().format("%Y-%m-%d %H:%M:%S");
        let mut buf = String::new();

        // Префикс: [INFO]     2026-06-10 10:10:01:
        write!(
            buf,
            "{:<10} {date}:",
            format!("[{log_level}]")
        )
        .unwrap();

        let plain = format!("{} {}", buf.bold(), content.as_ref());

        match log_level {
            LogLevel::Info    => println!("{}", plain.bright_blue()),
            LogLevel::Trace   => println!("{}", plain.bright_cyan()),
            LogLevel::Warning => println!("{}", plain.yellow()),
            LogLevel::Error   => eprintln!("{}", plain.bright_red()),
        };
    }
}

pub fn log(content: impl AsRef<str>, log_level: LogLevel) {
    Logger::log(content, log_level);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Trace,
    Warning,
    Error,
}

impl LogLevel {
    fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Trace => "TRACE",
            LogLevel::Warning => "WARNING",
            LogLevel::Error => "ERROR",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
