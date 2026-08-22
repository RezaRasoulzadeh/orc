use std::sync::{Arc, Mutex, mpsc};

use crate::storage::db::LifecycleEvent;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "event", rename_all = "snake_case")]
pub enum AppEvent {
    RunStarted(LifecycleEvent),
    WorkerOutput(LifecycleEvent),
    RunPhaseChanged(LifecycleEvent),
    ValidationStarted(LifecycleEvent),
    ValidationCompleted(LifecycleEvent),
    TaskLifecycle(LifecycleEvent),
    ApprovalChanged(LifecycleEvent),
    AgentChanged(LifecycleEvent),
}

#[derive(Clone)]
pub struct EventHub {
    subscribers: Arc<Mutex<Vec<mpsc::Sender<AppEvent>>>>,
}

pub struct EventSubscription {
    receiver: mpsc::Receiver<AppEvent>,
}

impl EventHub {
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub fn subscribe(&self) -> EventSubscription {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.push(sender);
        }
        EventSubscription { receiver }
    }
    pub fn publish(&self, event: AppEvent) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.retain(|sender| sender.send(event.clone()).is_ok());
        }
    }
}

impl AppEvent {
    pub fn from_lifecycle(event: LifecycleEvent) -> Self {
        match event.kind.as_str() {
            "dispatch_start" => Self::RunStarted(event),
            "worker_output" => Self::WorkerOutput(event),
            "run_phase_changed" => Self::RunPhaseChanged(event),
            "validation_started" => Self::ValidationStarted(event),
            "validation_completed" | "validation_result" => Self::ValidationCompleted(event),
            "approval_created" | "approval_resolved" => Self::ApprovalChanged(event),
            "agent_changed" | "quota_changed" => Self::AgentChanged(event),
            _ => Self::TaskLifecycle(event),
        }
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSubscription {
    pub fn recv(&self) -> Result<AppEvent, mpsc::RecvError> {
        self.receiver.recv()
    }
    pub fn try_recv(&self) -> Result<AppEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}
