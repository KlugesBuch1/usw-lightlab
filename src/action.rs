use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Connect,
    Disconnect,
    Preview,
    Test,
    Keep,
    Kill,
    Stop,
}

pub struct ActionGate {
    busy: Mutex<Option<ActionKind>>,
}

impl Default for ActionGate {
    fn default() -> Self {
        Self {
            busy: Mutex::new(None),
        }
    }
}

impl ActionGate {
    pub fn try_begin(&self, kind: ActionKind) -> bool {
        let mut busy = self.busy.lock().unwrap();
        if busy.is_some() {
            return false;
        }
        *busy = Some(kind);
        true
    }

    pub fn finish(&self) {
        *self.busy.lock().unwrap() = None;
    }

    pub fn current(&self) -> Option<ActionKind> {
        *self.busy.lock().unwrap()
    }

    pub fn is_busy(&self) -> bool {
        self.current().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_one_action_at_a_time() {
        let gate = ActionGate::default();
        assert!(gate.try_begin(ActionKind::Connect));
        assert!(!gate.try_begin(ActionKind::Test));
        assert_eq!(gate.current(), Some(ActionKind::Connect));
        gate.finish();
        assert!(gate.try_begin(ActionKind::Test));
    }
}
