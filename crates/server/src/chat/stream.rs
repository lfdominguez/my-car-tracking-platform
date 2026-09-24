//! Live fan-out from a detached generation task to any number of SSE readers.
//!
//! The generation task is *not* tied to the HTTP connection that started it: closing
//! the tab does not abort an answer, and reopening the page reattaches to one in
//! flight. That is the whole reason a hub exists rather than generating inline.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

use crate::analysis::jobs::CancelFlag;

/// Buffered events per in-flight message. Deltas are small and consumers are fast;
/// this only needs to absorb a slow reader's scheduling jitter.
const CHANNEL_CAPACITY: usize = 256;

/// Registry of in-flight generations, keyed by assistant message id.
#[derive(Default)]
pub struct ChatHub {
    channels: RwLock<HashMap<Uuid, Generation>>,
}

struct Generation {
    tx: broadcast::Sender<ai::ChatEvent>,
    cancel: CancelFlag,
}

impl ChatHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a channel for a generation that is about to start, plus the flag that
    /// stops it.
    pub async fn register(
        &self,
        message_id: Uuid,
    ) -> (broadcast::Sender<ai::ChatEvent>, CancelFlag) {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        let cancel = CancelFlag::new();
        self.channels.write().await.insert(
            message_id,
            Generation {
                tx: tx.clone(),
                cancel: cancel.clone(),
            },
        );
        (tx, cancel)
    }

    /// Ask a running generation to stop. `false` when none is running here.
    pub async fn cancel(&self, message_id: Uuid) -> bool {
        match self.channels.read().await.get(&message_id) {
            Some(generation) => {
                generation.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Subscribe to a generation, if it is still running.
    pub async fn subscribe(&self, message_id: Uuid) -> Option<broadcast::Receiver<ai::ChatEvent>> {
        self.channels
            .read()
            .await
            .get(&message_id)
            .map(|generation| generation.tx.subscribe())
    }

    /// Close a channel once its generation has ended. Must run on every exit path,
    /// or the map grows for the life of the process.
    pub async fn unregister(&self, message_id: Uuid) {
        self.channels.write().await.remove(&message_id);
    }

    /// Test-only accessor; `is_empty` would be dead weight beside it.
    #[cfg(test)]
    #[allow(clippy::len_without_is_empty)]
    pub async fn len(&self) -> usize {
        self.channels.read().await.len()
    }
}

/// Bridges [`ai::ChatSink`] to a broadcast channel.
///
/// `send` fails when nobody is listening, which is the normal case for a user who
/// closed the tab — the generation continues and is persisted regardless, so the
/// error is deliberately ignored.
pub struct BroadcastSink {
    tx: broadcast::Sender<ai::ChatEvent>,
}

impl BroadcastSink {
    pub fn new(tx: broadcast::Sender<ai::ChatEvent>) -> Self {
        Self { tx }
    }
}

impl ai::ChatSink for BroadcastSink {
    fn emit(&self, event: ai::ChatEvent) {
        let _ = self.tx.send(event);
    }
}

/// A sink that also accumulates content, so the generation task can flush partial
/// text to the database without re-deriving it from the event stream.
pub struct TeeSink {
    inner: BroadcastSink,
    content: Arc<std::sync::Mutex<String>>,
}

impl TeeSink {
    pub fn new(tx: broadcast::Sender<ai::ChatEvent>) -> Self {
        Self {
            inner: BroadcastSink::new(tx),
            content: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    /// Handle to the text accumulated so far, for periodic persistence.
    pub fn content_handle(&self) -> Arc<std::sync::Mutex<String>> {
        Arc::clone(&self.content)
    }
}

impl ai::ChatSink for TeeSink {
    fn emit(&self, event: ai::ChatEvent) {
        if let ai::ChatEvent::Delta { text, .. } = &event
            && let Ok(mut buf) = self.content.lock()
        {
            buf.push_str(text);
        }
        self.inner.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai::ChatSink;

    #[tokio::test]
    async fn register_subscribe_unregister_leaves_no_entry() {
        let hub = ChatHub::new();
        let id = Uuid::new_v4();
        assert_eq!(hub.len().await, 0);

        let (tx, _cancel) = hub.register(id).await;
        assert_eq!(hub.len().await, 1);
        assert!(hub.subscribe(id).await.is_some());

        hub.unregister(id).await;
        assert_eq!(hub.len().await, 0);
        assert!(hub.subscribe(id).await.is_none());
        drop(tx);
    }

    #[tokio::test]
    async fn cancel_reaches_only_a_registered_generation() {
        let hub = ChatHub::new();
        let id = Uuid::new_v4();
        assert!(!hub.cancel(id).await);
        let (_tx, flag) = hub.register(id).await;
        assert!(hub.cancel(id).await);
        assert!(flag.is_cancelled());
        hub.unregister(id).await;
        assert!(!hub.cancel(id).await);
    }

    #[tokio::test]
    async fn subscribing_to_an_unknown_message_is_none() {
        let hub = ChatHub::new();
        assert!(hub.subscribe(Uuid::new_v4()).await.is_none());
    }

    #[tokio::test]
    async fn broadcast_sink_does_not_fail_without_listeners() {
        let hub = ChatHub::new();
        let id = Uuid::new_v4();
        let (tx, _cancel) = hub.register(id).await;
        let sink = BroadcastSink::new(tx);
        // No receiver: emitting must still be a no-op rather than a panic.
        sink.emit(ai::ChatEvent::ToolStarted {
            name: "list_trips".into(),
        });
    }

    #[tokio::test]
    async fn tee_sink_accumulates_delta_text_in_order() {
        let hub = ChatHub::new();
        let (tx, _cancel) = hub.register(Uuid::new_v4()).await;
        let sink = TeeSink::new(tx);
        let handle = sink.content_handle();

        sink.emit(ai::ChatEvent::Delta {
            offset: 0,
            text: "Your least ".into(),
        });
        sink.emit(ai::ChatEvent::ToolStarted {
            name: "get_trip".into(),
        });
        sink.emit(ai::ChatEvent::Delta {
            offset: 11,
            text: "efficient trip".into(),
        });

        assert_eq!(&*handle.lock().unwrap(), "Your least efficient trip");
    }

    #[tokio::test]
    async fn subscribers_receive_events_emitted_after_they_subscribe() {
        let hub = ChatHub::new();
        let id = Uuid::new_v4();
        let (tx, _cancel) = hub.register(id).await;
        let mut rx = hub.subscribe(id).await.unwrap();
        BroadcastSink::new(tx).emit(ai::ChatEvent::Done {
            content: "done".into(),
        });
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, ai::ChatEvent::Done { .. }));
    }
}
