use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;

pub type ClientTx = mpsc::UnboundedSender<String>;

#[derive(Clone, Default)]
pub struct Hub {
    users: Arc<Mutex<HashMap<Uuid, ClientTx>>>,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, id: Uuid, tx: ClientTx) {
        let mut users = self.users.lock().unwrap();
        users.insert(id, tx);
    }

    pub fn unregister(&self, id: &Uuid) {
        let mut users = self.users.lock().unwrap();
        users.remove(id);
    }

    pub fn send_to(&self, target_id: &Uuid, message: &str) -> bool {
        let users = self.users.lock().unwrap();

        if let Some(tx) = users.get(target_id) {
            tx.send(message.to_string()).is_ok()
        } else {
            false // Значит что пользователь не в сети
        }
    }
}