use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws;
use futures::stream::SplitSink;
use futures::SinkExt;
use serde_json::{json, Value};

use crate::websocket::WebSocket;
use crate::{Channel, Event, Message, MessageExt, Payload, ShardSocket, Status, Topic};

pub struct ConnectedSocket<T> {
    pub(crate) websocket: Arc<WebSocket<T>>, // 存储了channel信息
    pub(crate) socket: ShardSocket,          // socket状态
    pub(crate) channel_sockets: HashMap<Topic, ShardSocket>, // channel状态, 由socket 克隆而来
    pub(crate) message: Option<Message>,
    pub(crate) sender: Option<SplitSink<ws::WebSocket, ws::Message>>, // 推送给用户
}

impl<T> ConnectedSocket<T> {
    #[warn(private_interfaces)]
    pub fn new(
        websocket: Arc<WebSocket<T>>,
        socket: ShardSocket,
        sender: Option<SplitSink<ws::WebSocket, ws::Message>>,
    ) -> Self {
        Self {
            websocket,
            sender,
            socket,
            channel_sockets: HashMap::new(),
            message: None,
        }
    }

    pub(crate) fn get_channel_socket(&self, topic: &Topic) -> Option<ShardSocket> {
        self.channel_sockets.get(topic).map(|socket| socket.clone())
    }

    pub(crate) fn put_channel_socket(&mut self, topic: Topic, socket: ShardSocket) {
        self.channel_sockets.insert(topic, socket);
    }

    pub(crate) fn take_channel_socket(&mut self, topic: &Topic) -> Option<ShardSocket> {
        self.channel_sockets.remove(topic)
    }

    pub(crate) fn is_join_topic(&self, other: &Topic) -> bool {
        self.channel_sockets.contains_key(other)
    }

    pub(crate) async fn reply(
        &mut self,
        status: impl ToString,
        response: Value,
    ) -> anyhow::Result<()> {
        self.push(
            Event::Reply,
            Payload::new(status, response),
            self.message.clone().unwrap(),
        )
        .await?;

        Ok(())
    }

    pub(crate) async fn reply_ok(&mut self) -> anyhow::Result<()> {
        self.reply(Status::Ok, json!({})).await?;

        Ok(())
    }

    pub(crate) async fn reply_error(&mut self, reason: &str) -> anyhow::Result<()> {
        self.reply(Status::Error, json!({ "reason": reason }))
            .await?;

        Ok(())
    }

    pub(crate) async fn reply_close(&mut self, data: impl MessageExt) -> anyhow::Result<()> {
        self.push(Event::Close, Payload::empty(), data).await?;

        Ok(())
    }

    pub(crate) async fn push(
        &mut self,
        event: Event,
        payload: Payload,
        data: impl MessageExt,
    ) -> anyhow::Result<()> {
        let message = Message {
            join_ref: data.join_ref().await,
            message_ref: data.message_ref().await,
            topic: data.topic().await,
            event,
            payload,
        };

        self.send(message).await?;

        Ok(())
    }

    pub(crate) async fn send(&mut self, message: Message) -> anyhow::Result<()> {
        if let Some(sender) = &mut self.sender {
            let value: Value = message.into();
            sender.send(value.to_string().into()).await?;
        }

        Ok(())
    }
}

impl<T> ConnectedSocket<T>
where
    T: Send + Sync + 'static,
{
    pub(crate) fn get_channel(&self, topic: &Topic) -> Option<Arc<Channel>> {
        self.websocket.get_channel(topic)
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::RwLock;

    use super::*;
    use crate::Socket;

    #[derive(Debug, Default)]
    pub struct UserSocket;

    #[test]
    fn test_get_channel() {
        let user_channel = Channel::new();
        let user_socket = WebSocket::<UserSocket>::new().channel("user:*", user_channel);
        let socket = Arc::new(RwLock::new(Socket::new()));

        let connected_socket = ConnectedSocket::new(Arc::new(user_socket), socket, None);

        let channel = connected_socket.get_channel(&Topic("user:123".to_string()));
        assert!(channel.is_some());

        let channel = connected_socket.get_channel(&Topic("room:456".to_string()));
        assert!(channel.is_none());
    }
}
