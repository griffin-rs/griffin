//! The socket transport: Phoenix Channels' envelope and V2 JSON serializer over a
//! WebSocket (the wire protocol is Phoenix's, unchanged). It exists to carry LiveViews and is not a public Channels API.

use crate::live::{self, Channel, Failed, Incoming};
use crate::token::SigningKey;
use axum::Router;
use axum::extract::ws::{self, WebSocket};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::{self, JoinSet};

/// One frame: the JSON array `[join_ref, ref, topic, event, payload]`.
///
/// The two refs are the client's and are sent back as they came, whatever JSON they are.
#[derive(Debug, Deserialize)]
pub(crate) struct Message {
    pub(crate) join_ref: Value,
    pub(crate) message_ref: Value,
    pub(crate) topic: String,
    pub(crate) event: String,
    pub(crate) payload: Value,
}

impl Message {
    /// The frame that answers this one: `status` is `ok` or `error`.
    pub(crate) fn reply(&self, status: &str, response: Value) -> String {
        json!([
            self.join_ref,
            self.message_ref,
            self.topic,
            "phx_reply",
            {"status": status, "response": response}
        ])
        .to_string()
    }

    /// The answer to a message for a topic nothing is joined on.
    fn unmatched(&self) -> String {
        self.reply("error", json!({"reason": "unmatched topic"}))
    }

    /// For a join: a frame the client did not ask for, as Phoenix pushes one (`push`
    /// in `channel.ex`). It has no ref, as it answers nothing.
    pub(crate) fn push(&self, event: &str, payload: Value) -> String {
        json!([self.join_ref, null, self.topic, event, payload]).to_string()
    }

    /// For a join: the frame that tells the client its channel has closed.
    pub(crate) fn close(&self) -> String {
        json!([self.join_ref, self.join_ref, self.topic, "phx_close", {}]).to_string()
    }

    /// For a join: the frame that tells the client its channel has crashed, as Phoenix
    /// sends it (`encode_on_exit` in `socket.ex`). The client shows the page as broken
    /// and joins again.
    fn crashed(&self) -> String {
        let (join, reason) = (&self.join_ref, json!({"reason": "channel_crash"}));
        json!([join, join, self.topic, "phx_error", reason]).to_string()
    }
}

/// Serves one socket until the browser goes away. Every LiveView joined over it is a
/// task owned here, so none outlives the connection (structured concurrency, no actor runtime).
pub(crate) async fn connection(mut socket: WebSocket, mut pages: Router, key: SigningKey) {
    // Everything sent to the browser goes through this queue, so frames leave in the
    // order they were produced. Frames are only added in answer to frames read, and
    // reading stops while a write waits.
    let (outgoing, mut outbox) = mpsc::unbounded_channel::<String>();
    // A LiveView's queue has no bound, like a Phoenix mailbox, so this loop
    // never waits for a handler and keeps answering heartbeats. A client that sends
    // faster than a handler works grows the queue: bound it and hang up when full if
    // that is ever seen.
    let mut channels: HashMap<String, mpsc::UnboundedSender<Incoming>> = HashMap::new();
    let mut tasks: JoinSet<Result<(), Failed>> = JoinSet::new();
    // For each LiveView's task: its LiveView, its topic, and the frame that says it
    // crashed.
    let mut joined: HashMap<task::Id, (String, String, String)> = HashMap::new();

    loop {
        let frame = tokio::select! {
            frame = socket.recv() => frame,
            Some(text) = outbox.recv() => {
                if socket.send(ws::Message::Text(text.into())).await.is_err() {
                    break;
                }
                continue;
            }
            // A LiveView's task ended. This is the one place where the client is told
            // of an unexpected failure (the failure policy). One the task returned, it
            // has logged. A panic unwound the task, which dropped its state, and is
            // caught here, at the task's boundary (structured concurrency, no actor runtime): this loop and the
            // socket's other LiveViews carry on.
            Some(ended) = tasks.join_next_with_id() => {
                let (id, failed, panicked) = match ended {
                    Ok((id, outcome)) => (id, outcome.is_err(), false),
                    Err(error) => (error.id(), error.is_panic(), error.is_panic()),
                };
                if let (Some((live_view, topic, crashed)), true) = (joined.remove(&id), failed) {
                    if panicked {
                        // Only the LiveView is named: a panic's message may hold values
                        // of the state or of what the browser sent.
                        tracing::error!(live_view, "a LiveView panicked: it ends");
                    }
                    // Unless the topic has been joined again since: that one lives.
                    if channels.get(&topic).is_some_and(|queue| queue.is_closed()) {
                        channels.remove(&topic);
                    }
                    let _ = outgoing.send(crashed);
                }
                continue;
            }
        };
        let Some(Ok(frame)) = frame else { break };
        // Binary frames carry uploads only; add them with uploads.
        let ws::Message::Text(text) = frame else {
            continue;
        };
        // A frame that is not an envelope is not from the Phoenix client: hang up.
        let Ok(message) = serde_json::from_str::<Message>(&text) else {
            break;
        };

        let reply = match (message.topic.as_str(), message.event.as_str()) {
            ("phoenix", "heartbeat") => message.reply("ok", json!({})),
            (_, "phx_join") => match live::join(&mut pages, &key, &message).await {
                Ok((live_view, start)) => {
                    let (sender, incoming) = mpsc::unbounded_channel();
                    let queue = sender.downgrade();
                    // A second join of a topic replaces the first, whose task ends
                    // when its queue closes.
                    channels.insert(message.topic.clone(), sender);
                    let crashed = (live_view, message.topic.clone(), message.crashed());
                    let task = tasks.spawn(start(Channel {
                        join: message,
                        incoming,
                        queue,
                        outgoing: outgoing.clone(),
                        pages: pages.clone(),
                    }));
                    joined.insert(task.id(), crashed);
                    continue;
                }
                Err(reason) => message.reply("error", json!({"reason": reason})),
            },
            (topic, event) => {
                // A leave frees the topic at once. The LiveView answers it and ends.
                let channel = if event == "phx_leave" {
                    channels.remove(topic)
                } else {
                    channels.get(topic).cloned()
                };
                match channel {
                    Some(channel) => match channel.send(Incoming::Frame(message)) {
                        Ok(()) => continue,
                        // The LiveView's task has ended.
                        Err(mpsc::error::SendError(Incoming::Frame(message))) => {
                            channels.remove(&message.topic);
                            message.unmatched()
                        }
                        Err(_) => unreachable!("what was sent is a frame"),
                    },
                    None => message.unmatched(),
                }
            }
        };
        let _ = outgoing.send(reply);
    }

    // Cancels every LiveView still running and waits until each is gone.
    tasks.shutdown().await;
}
