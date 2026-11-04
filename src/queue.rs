//! Immutable, ECID-addressed restore plans and sequential batch execution.
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug)]
pub struct RestoreRequest {
    pub key: String,
    pub name: String,
    pub ecid: u64,
    pub identifier: String,
    pub version: String,
    pub buildid: String,
    pub erase: bool,
}
impl RestoreRequest {
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · ECID {:x}\n{} ({}) · {}",
            self.name,
            self.identifier,
            self.ecid,
            self.version,
            self.buildid,
            if self.erase {
                "ERASE ALL DATA"
            } else {
                "Keep Data"
            }
        )
    }
}
#[derive(Clone, Debug)]
pub struct QueueItem {
    pub request: RestoreRequest,
    pub status: String,
}

/// A failed device does not prevent other queued devices from running.
/// Cancellation stops the current executor and marks remaining items skipped.
pub fn execute(
    requests: &[RestoreRequest],
    cancel: &AtomicBool,
    mut run: impl FnMut(&RestoreRequest) -> Result<(), String>,
    mut state: impl FnMut(&RestoreRequest, &str),
) {
    for request in requests {
        if cancel.load(Ordering::Relaxed) {
            state(request, "Skipped (queue cancelled)");
            continue;
        }
        state(request, "Running");
        match run(request) {
            Ok(()) => state(request, "Completed"),
            Err(error) => state(request, &error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn request(ecid: u64) -> RestoreRequest {
        RestoreRequest {
            key: format!("ecid:{ecid:x}"),
            name: "Test device".into(),
            ecid,
            identifier: "iPhone15,2".into(),
            version: "test".into(),
            buildid: "test".into(),
            erase: true,
        }
    }
    #[test]
    fn preserves_targets_and_continues_after_failure() {
        let mut targets = Vec::new();
        let mut states = Vec::new();
        execute(
            &[request(11), request(22)],
            &AtomicBool::new(false),
            |r| {
                targets.push(r.ecid);
                if r.ecid == 11 {
                    Err("Disconnected".into())
                } else {
                    Ok(())
                }
            },
            |r, s| states.push((r.ecid, s.to_string())),
        );
        assert_eq!(targets, vec![11, 22]);
        assert_eq!(
            states,
            vec![
                (11, "Running".into()),
                (11, "Disconnected".into()),
                (22, "Running".into()),
                (22, "Completed".into())
            ]
        );
    }
    #[test]
    fn cancellation_skips_remaining_devices() {
        let cancel = AtomicBool::new(false);
        let mut targets = Vec::new();
        let mut states = Vec::new();
        execute(
            &[request(11), request(22)],
            &cancel,
            |r| {
                targets.push(r.ecid);
                cancel.store(true, Ordering::Relaxed);
                Err("Cancelled".into())
            },
            |_, s| states.push(s.to_string()),
        );
        assert_eq!(targets, vec![11]);
        assert_eq!(
            states,
            vec!["Running", "Cancelled", "Skipped (queue cancelled)"]
        );
    }
}
