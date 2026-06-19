use chrono::Local;
use colored::Colorize;
use std::fmt::{self, Write};

pub struct Logger;

#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        $crate::logger::log(format!($($arg)*), $crate::logger::LogLevel::Trace)
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::logger::log(format!($($arg)*), $crate::logger::LogLevel::Info)
    };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::logger::log(format!($($arg)*), $crate::logger::LogLevel::Debug)
    };
}

#[macro_export]
macro_rules! warning {
    ($($arg:tt)*) => {
        $crate::logger::log(format!($($arg)*), $crate::logger::LogLevel::Warning)
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::logger::log(format!($($arg)*), $crate::logger::LogLevel::Error)
    };
}

#[macro_export]
macro_rules! fatal {
    ($($arg:tt)*) => {
        $crate::logger::log(format!($($arg)*), $crate::logger::LogLevel::Fatal)
    };
}

impl Logger {
    /// Логирует сообщение с указанным уровнем.
    ///
    /// # Пример
    /// ```
    /// mod logger::*;
    /// 
    /// fn main() {
    ///     let text = "Some information";
    ///     info!("Content: {text}");
    /// }
    /// ```
    fn log(content: impl AsRef<str>, log_level: LogLevel) {
        let date = Local::now().format("%Y-%m-%d %H:%M:%S.%f");
        let mut buf = String::new();

        write!(
            buf,
            "{:<10} {date}:",
            format!("[{log_level}]")
        )
        .unwrap();

        let plain = format!("{} {}", buf.bold(), content.as_ref());

        match log_level {
            LogLevel::Info    =>  println!("{}", plain.bright_blue()),
            LogLevel::Trace   =>  println!("{}", plain.bright_black()),
            LogLevel::Debug   =>  println!("{}", plain.bright_green()),
            LogLevel::Warning =>  println!("{}", plain.yellow()),
            LogLevel::Error   =>  eprintln!("{}", plain.bright_red()),
            LogLevel::Fatal   =>  eprintln!("{}", plain.red().bold().on_black()),
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
    Debug,
    Warning,
    Error,
    Fatal,
}

impl LogLevel {
    fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Warning => "WARNING",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
