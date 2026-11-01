//! One pending snapshot before the serial command queue, never one payload per event.
use std::sync::Mutex;

pub struct LatestSnapshot<T> {
    state: Mutex<State<T>>,
}

struct State<T> {
    next_ticket: u64,
    pending: Option<(u64, String, T)>,
}

impl<T> Default for LatestSnapshot<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                next_ticket: 0,
                pending: None,
            }),
        }
    }
}

impl<T> LatestSnapshot<T> {
    /// Drop the replaced payload BEFORE making its replacement. Only lightweight tickets queue.
    pub fn submit(
        &self,
        account: &str,
        snapshot: impl FnOnce() -> T,
        enqueue: impl FnOnce(u64) -> bool,
    ) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let previous = state.pending.take();
        let reuse = previous
            .as_ref()
            .filter(|(_, owner, _)| owner == account)
            .map(|(ticket, _, _)| *ticket);
        drop(previous);
        let ticket = reuse.unwrap_or_else(|| {
            state.next_ticket = state.next_ticket.wrapping_add(1);
            state.next_ticket
        });
        state.pending = Some((ticket, account.to_owned(), snapshot()));
        if reuse.is_some() || enqueue(ticket) {
            true
        } else {
            state.pending = None;
            false
        }
    }

    pub fn take(&self, ticket: u64) -> Option<(String, T)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .pending
            .as_ref()
            .is_some_and(|(pending, _, _)| *pending == ticket)
        {
            state
                .pending
                .take()
                .map(|(_, account, snapshot)| (account, snapshot))
        } else {
            None
        }
    }

    pub fn invalidate(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).pending = None;
    }
}

/// Hash serialized fields directly into a fixed-size sink, including raw history payloads.
/// Unlike to_vec/to_value this does not allocate another history-sized representation.
#[cfg(test)]
pub(super) fn fingerprint(value: &impl serde::Serialize) -> Option<u64> {
    use std::hash::Hasher;
    struct Sink(std::collections::hash_map::DefaultHasher);
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.write(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Sink(std::collections::hash_map::DefaultHasher::new());
    serde_json::to_writer(&mut sink, value).ok()?;
    Some(sink.0.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Payload {
        revision: usize,
        live: Arc<AtomicUsize>,
    }
    impl Payload {
        fn new(revision: usize, live: &Arc<AtomicUsize>) -> Self {
            let count = live.fetch_add(1, Ordering::SeqCst) + 1;
            assert!(count <= 2, "more than one writing and one pending snapshot");
            Self {
                revision,
                live: live.clone(),
            }
        }
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            self.live.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn burst_is_bounded_before_enqueue_and_keeps_latest() {
        let mailbox = LatestSnapshot::default();
        let live = Arc::new(AtomicUsize::new(0));
        let mut tickets = Vec::new();
        mailbox.submit(
            "A",
            || Payload::new(0, &live),
            |id| {
                tickets.push(id);
                true
            },
        );
        let writing = mailbox.take(tickets.remove(0)).unwrap();
        for revision in 1..10_000 {
            mailbox.submit(
                "A",
                || Payload::new(revision, &live),
                |id| {
                    tickets.push(id);
                    true
                },
            );
        }
        assert_eq!(tickets.len(), 1);
        assert_eq!(live.load(Ordering::SeqCst), 2);
        drop(writing);
        let (_, newest) = mailbox.take(tickets[0]).unwrap();
        assert_eq!(newest.revision, 9_999);
        drop(newest);
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn account_switch_logout_and_failed_enqueue_release_pending() {
        let mailbox = LatestSnapshot::default();
        let a = Cell::new(0);
        let b = Cell::new(0);
        mailbox.submit(
            "A",
            || 1,
            |id| {
                a.set(id);
                true
            },
        );
        mailbox.submit(
            "B",
            || 2,
            |id| {
                b.set(id);
                true
            },
        );
        assert!(mailbox.take(a.get()).is_none());
        assert_eq!(mailbox.take(b.get()), Some(("B".into(), 2)));
        mailbox.submit(
            "A",
            || 3,
            |id| {
                a.set(id);
                true
            },
        );
        mailbox.invalidate();
        assert!(mailbox.take(a.get()).is_none());
        let stale_ticket = a.get();
        mailbox.submit(
            "A",
            || 5,
            |id| {
                a.set(id);
                true
            },
        );
        assert_ne!(a.get(), stale_ticket);
        assert!(mailbox.take(stale_ticket).is_none());
        assert_eq!(mailbox.take(a.get()), Some(("A".into(), 5)));
        assert!(!mailbox.submit(
            "B",
            || 4,
            |id| {
                b.set(id);
                false
            }
        ));
        assert!(mailbox.take(b.get()).is_none());
    }

    #[test]
    fn logout_barrier_invalidates_old_tickets_before_same_account_login() {
        use std::collections::VecDeque;
        enum Command {
            Persist(u64),
            Logout,
        }
        let mailbox = LatestSnapshot::default();
        let mut commands = VecDeque::new();
        mailbox.submit(
            "A",
            || 1,
            |id| {
                commands.push_back(Command::Persist(id));
                true
            },
        );
        let Command::Persist(first) = commands.pop_front().unwrap() else {
            panic!("expected snapshot");
        };
        let writing = mailbox.take(first).unwrap();
        mailbox.submit(
            "A",
            || 2,
            |id| {
                commands.push_back(Command::Persist(id));
                true
            },
        );
        commands.push_back(Command::Logout);
        mailbox.submit(
            "A",
            || 3,
            |_| panic!("pending snapshot must reuse its FIFO ticket"),
        );
        let mut disk = Some(writing.1);
        let mut writes = vec![disk.unwrap()];
        while let Some(command) = commands.pop_front() {
            match command {
                Command::Persist(id) => {
                    if let Some((_, value)) = mailbox.take(id) {
                        disk = Some(value);
                        writes.push(value);
                    }
                }
                Command::Logout => {
                    mailbox.invalidate();
                    disk = None;
                }
            }
        }
        assert_eq!(writes, vec![1, 3]);
        assert_eq!(disk, None);
        mailbox.submit(
            "A",
            || 4,
            |id| {
                commands.push_back(Command::Persist(id));
                true
            },
        );
        assert!(mailbox.take(first).is_none());
        let Command::Persist(new_ticket) = commands.pop_front().unwrap() else {
            panic!("expected snapshot");
        };
        assert_eq!(mailbox.take(new_ticket), Some(("A".into(), 4)));
    }

    #[test]
    fn fingerprints_include_history_and_avoid_unchanged_snapshots() {
        let original = serde_json::json!({"messages": [{"history_source": {"body": "old"}}]});
        let changed = serde_json::json!({"messages": [{"history_source": {"body": "new"}}]});
        assert_eq!(fingerprint(&original), fingerprint(&original));
        assert_ne!(fingerprint(&original), fingerprint(&changed));
    }
}
