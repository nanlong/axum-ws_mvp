use std::{fmt::Display, ops::Deref, str::FromStr, sync::Arc};

use serde_json::{json, Value};
use thiserror::Error;
use tokio::sync::{broadcast, RwLock};

mod callback;
mod channel;
mod connected_socket;
mod socket;
mod utils;
mod websocket;

use callback::{Callback, WebSocketConnectCallback, WebSocketIdCallback};
use connected_socket::ConnectedSocket;
use socket::Socket;

pub use channel::Channel;
pub use websocket::WebSocket;

pub type ShardSocket = Arc<RwLock<Socket>>;
pub type Response = anyhow::Result<Option<Value>>;

lazy_static::lazy_static! {
    static ref SENDER: broadcast::Sender<BroadcastMessage> = broadcast::channel(1024).0;
}

#[derive(Debug, Default)]
struct Path(String);

impl FromStr for Path {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct Topic(String);

impl Topic {
    fn is_match(&self, topic: &Topic) -> bool {
        let mut self_parts = self.split(':');
        let mut topic_parts = topic.split(':');

        loop {
            match (self_parts.next(), topic_parts.next()) {
                (Some("*"), _) => return true,
                (Some(self_part), Some(topic_part)) if self_part == topic_part => continue,
                (None, None) => return true,
                _ => return false,
            }
        }
    }
}

impl FromStr for Topic {
    type Err = ();

    fn from_str(s: &str) -> anyhow::Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

impl From<&str> for Topic {
    fn from(s: &str) -> Self {
        s.parse().unwrap()
    }
}

impl From<Topic> for Value {
    fn from(topic: Topic) -> Self {
        Value::String(topic.0)
    }
}

impl Deref for Topic {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
enum Event {
    Join,
    Leave,
    Close,
    Reply,
    Heartbeat,
    Custom(String),
}

impl From<&str> for Event {
    fn from(s: &str) -> Self {
        match s {
            "phx_join" => Self::Join,
            "phx_leave" => Self::Leave,
            "phx_close" => Self::Close,
            "phx_reply" => Self::Reply,
            "heartbeat" => Self::Heartbeat,
            custom => Self::Custom(custom.to_owned()),
        }
    }
}

impl From<Event> for Value {
    fn from(event: Event) -> Self {
        match event {
            Event::Join => Value::String("phx_join".to_string()),
            Event::Leave => Value::String("phx_leave".to_string()),
            Event::Close => Value::String("phx_close".to_string()),
            Event::Reply => Value::String("phx_reply".to_string()),
            Event::Heartbeat => Value::String("heartbeat".to_string()),
            Event::Custom(custom) => Value::String(custom),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct Payload(Value);

impl Payload {
    fn new(status: impl ToString, response: Value) -> Self {
        Self(json!({
            "status": status.to_string(),
            "response": response,
        }))
    }

    fn empty() -> Self {
        Self(json!({}))
    }
}

impl Default for Payload {
    fn default() -> Self {
        Self::new(Status::Ok, Value::Object(Default::default()))
    }
}

impl From<Value> for Payload {
    fn from(value: Value) -> Self {
        Self(value)
    }
}

impl From<Payload> for Value {
    fn from(payload: Payload) -> Self {
        payload.0.clone()
    }
}

impl Deref for Payload {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct Message {
    join_ref: Option<String>,
    message_ref: Option<String>,
    topic: Topic,
    event: Event,
    payload: Payload,
}

impl TryFrom<String> for Message {
    type Error = ();

    fn try_from(value: String) -> anyhow::Result<Self, Self::Error> {
        let value: Vec<Value> = serde_json::from_str(&value).map_err(|_| ())?;

        if value.len() != 5 {
            return Err(());
        }

        let join_ref = value[0].as_str().map(ToOwned::to_owned);
        let message_ref = value[1].as_str().map(ToOwned::to_owned);
        let topic = value[2].as_str().ok_or(())?.into();
        let event = value[3].as_str().ok_or(())?.into();
        let payload = value[4].clone().into();

        Ok(Self {
            join_ref,
            message_ref,
            topic,
            event,
            payload,
        })
    }
}

impl From<Message> for Value {
    fn from(message: Message) -> Self {
        let join_ref = message.join_ref.map(Value::String).unwrap_or(Value::Null);
        let message_ref = message
            .message_ref
            .map(Value::String)
            .unwrap_or(Value::Null);
        let topic = message.topic.into();
        let event = message.event.into();
        let payload = message.payload.into();

        Value::Array(vec![join_ref, message_ref, topic, event, payload])
    }
}

#[derive(Debug)]
pub enum Status {
    Ok,
    Error,
}

impl ToString for Status {
    fn to_string(&self) -> String {
        match self {
            Self::Ok => "ok".to_string(),
            Self::Error => "error".to_string(),
        }
    }
}

impl From<&str> for Status {
    fn from(s: &str) -> Self {
        match s {
            "ok" => Self::Ok,
            "error" => Self::Error,
            _ => unreachable!(),
        }
    }
}

#[derive(Debug, Clone)]
enum BroadcastTarget {
    All,
    Other,
}

#[derive(Debug, Clone)]
struct BroadcastMessage {
    topic: Topic,
    event: Event,
    payload: Payload,
    from: Option<String>,
    target: BroadcastTarget,
}

impl From<BroadcastMessage> for Message {
    fn from(message: BroadcastMessage) -> Self {
        Self {
            join_ref: None,
            message_ref: None,
            topic: message.topic,
            event: message.event,
            payload: message.payload,
        }
    }
}

#[warn(async_fn_in_trait)]
trait MessageExt {
    async fn join_ref(&self) -> Option<String>;
    async fn message_ref(&self) -> Option<String>;
    async fn topic(&self) -> Topic;
}

impl MessageExt for Message {
    async fn join_ref(&self) -> Option<String> {
        self.join_ref.clone()
    }

    async fn message_ref(&self) -> Option<String> {
        self.message_ref.clone()
    }

    async fn topic(&self) -> Topic {
        self.topic.clone()
    }
}

#[derive(Error, Debug)]
pub(crate) enum WsError {
    #[error("unmatched topic")]
    UnmatchedTopic,
    #[error("unmatched event")]
    UnmatchedEvent,
    // #[error("autherization failed")]
    // AutherizationFailed,
    // #[error("unknown error")]
    // Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_try_from_with_heartbeat() {
        let message = r#"[null,"123","phoenix","heartbeat",{}]"#;
        let message: Message = message.to_string().try_into().unwrap();
        assert_eq!(message.join_ref, None);
        assert_eq!(message.message_ref, Some("123".to_string()));
        assert_eq!(message.topic, Topic("phoenix".to_string()));
        assert_eq!(message.event, Event::Heartbeat);
        assert_eq!(message.payload, Payload(Value::Object(Default::default())));
    }

    #[test]
    fn test_message_try_from_with_join() {
        let message = r#"["118","118","room:1","phx_join",{}]"#;
        let message: Message = message.to_string().try_into().unwrap();
        assert_eq!(message.join_ref, Some("118".to_string()));
        assert_eq!(message.message_ref, Some("118".to_string()));
        assert_eq!(message.topic, Topic("room:1".to_string()));
        assert_eq!(message.event, Event::Join);
        assert_eq!(message.payload, Payload(Value::Object(Default::default())));
    }

    #[test]
    fn test_message_to_json() {
        let message = Message {
            join_ref: Some("118".to_string()),
            message_ref: Some("118".to_string()),
            topic: Topic("room:1".to_string()),
            event: Event::Join,
            payload: Payload(Value::Object(Default::default())),
        };

        let value: Value = message.into();
        let expected = r#"["118","118","room:1","phx_join",{}]"#;
        assert_eq!(value.to_string(), expected);
    }

    #[test]
    fn test_topic_is_match() {
        let topic1: Topic = "room:*".into();
        let topic2: Topic = "room:123".into();
        let topic3: Topic = "user:123".into();
        let topic4: Topic = "room:123".into();

        assert!(topic1.is_match(&topic2));
        assert!(!topic1.is_match(&topic3));
        assert!(topic2.is_match(&topic4));
    }
}
