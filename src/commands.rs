use std::{io::Write, net::TcpStream};
use crate::{error, trace};

#[derive(Debug, PartialEq)]
pub enum Command {
    Ping,
    Exit,
}

impl Command {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "/ping" => Some(Command::Ping),
            "/exit" => Some(Command::Exit),
            _ => None
        }
    }
}

pub fn handle_command<W: Write>(stream: &mut W, command: Command) -> bool {
    match command {
        Command::Ping => {
            if let Err(e) = stream.write_all(b"Server: PONG\n") {
                error!("Failed to write PING: {e}");
            };
            true
        }
        Command::Exit => { 
            if let Err(e) = stream.write_all(b"Disconnected from server\n") {
                error!("Failed to write EXIT: {e}");
            };
            false
        }
    }
}