use std::sync::Arc;
use std::{collections::HashMap, marker::PhantomData};

use axum::extract::ws::{self, WebSocket as AxumWebSocket};
use axum::extract::Query;
use axum::{extract::WebSocketUpgrade, response::IntoResponse, routing::get, Extension, Router};
use futures::{stream::SplitStream, Future, StreamExt};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

use crate::{
    utils, BroadcastMessage, BroadcastTarget, Channel, ConnectedSocket, Event, Message, MessageExt,
    Path, ShardSocket, Socket, Topic, WebSocketConnectCallback, WebSocketIdCallback, WsError,
    SENDER,
};

pub struct WebSocket<T> {
    path: Path,
    channels: HashMap<Topic, Arc<Channel>>,
    connect_callback: Option<Box<dyn WebSocketConnectCallback + Send + Sync>>,
    id_callback: Option<Box<dyn WebSocketIdCallback + Send + Sync>>,
    _tag: PhantomData<T>,
}

impl<T> WebSocket<T>
where
    T: 'static + Send + Sync,
{
    pub fn new() -> WebSocket<T> {
        Self {
            path: Default::default(),
            channels: Default::default(),
            connect_callback: None,
            id_callback: None,
            _tag: PhantomData,
        }
    }

    pub fn path(mut self, path: &str) -> Self {
        self.path = path.parse().unwrap();
        self
    }

    pub fn channel(mut self, topic: &str, channel: Channel) -> Self {
        self.channels
            .insert(topic.parse().unwrap(), Arc::new(channel));
        self
    }

    pub(crate) fn get_channel(&self, topic: &Topic) -> Option<Arc<Channel>> {
        for (key, channel) in &self.channels {
            if key.is_match(topic) {
                return Some(Arc::clone(channel));
            }
        }

        None
    }

    pub fn on_connect<F, R>(mut self, callback: F) -> Self
    where
        F: FnOnce(Value, ShardSocket) -> R + WebSocketConnectCallback + Send + Sync + 'static,
        R: Future<Output = ()>,
    {
        self.connect_callback = Some(Box::new(callback));
        self
    }

    pub fn id<F, R>(mut self, callback: F) -> Self
    where
        F: FnOnce(ShardSocket) -> R + WebSocketIdCallback + Send + Sync + 'static,
        R: Future<Output = String>,
    {
        self.id_callback = Some(Box::new(callback));
        self
    }

    async fn connect(
        websocket_upgrade: WebSocketUpgrade,
        Query(params): Query<Value>,
        Extension(websocket): Extension<Arc<WebSocket<T>>>,
    ) -> impl IntoResponse {
        websocket_upgrade.on_upgrade(|axum_websocket| async {
            let socket = Arc::new(RwLock::new(Socket::new()));

            if let Some(callback) = &websocket.connect_callback {
                callback.call(params, Arc::clone(&socket)).await;
            };

            if let Some(callback) = &websocket.id_callback {
                let id = callback.call(Arc::clone(&socket)).await;
                socket.write().await.id = Some(id);
            }

            let (sender, receiver) = axum_websocket.split();

            let connected_socket = Arc::new(Mutex::new(ConnectedSocket::new(
                websocket,
                socket,
                Some(sender),
            )));

            Self::websocket(connected_socket, receiver).await;
        })
    }

    async fn websocket(
        socket: Arc<Mutex<ConnectedSocket<T>>>,
        mut receiver: SplitStream<AxumWebSocket>,
    ) {
        let mut rx = SENDER.subscribe();
        let socket1 = Arc::clone(&socket);
        let socket2 = Arc::clone(&socket);

        let mut recv_task = tokio::spawn(async move {
            while let Some(Ok(ws::Message::Text(message))) = receiver.next().await {
                let message = Message::try_from(message).unwrap();
                let mut connected_socket = socket1.lock().await;
                connected_socket.message = Some(message.clone());

                match message.event {
                    Event::Heartbeat => {
                        connected_socket.reply_ok().await.unwrap();
                    }
                    Event::Join => {
                        let topic = message.topic().await;
                        let channel_socket = Arc::new(RwLock::new(Socket {
                            joined: true,
                            join_ref: message.join_ref().await,
                            message_ref: message.message_ref().await,
                            topic: Some(topic.clone()),
                            ..connected_socket.socket.read().await.clone()
                        }));

                        if let Some(channel) = connected_socket.get_channel(&topic) {
                            match channel
                                .join(message.payload, Arc::clone(&channel_socket))
                                .await
                            {
                                Ok(Some(response)) => {
                                    connected_socket.put_channel_socket(topic, channel_socket);
                                    connected_socket.reply("ok", response).await.unwrap();
                                }
                                Ok(None) => {
                                    connected_socket.put_channel_socket(topic, channel_socket);
                                    connected_socket.reply_ok().await.unwrap();
                                }
                                Err(error) => {
                                    connected_socket
                                        .reply_error(error.to_string().as_str())
                                        .await
                                        .unwrap();
                                }
                            }
                        } else {
                            connected_socket
                                .reply_error(WsError::UnmatchedTopic.to_string().as_str())
                                .await
                                .unwrap();
                        }
                    }
                    Event::Leave => {
                        connected_socket.reply_ok().await.unwrap();

                        if let Some(socket) = connected_socket.take_channel_socket(&message.topic) {
                            connected_socket.reply_close(socket).await.unwrap();
                        }
                    }
                    Event::Custom(ref _event) => {
                        drop(connected_socket); // 释放锁
                        Self::call(message, Arc::clone(&socket)).await;
                    }
                    _ => {}
                }
            }
        });

        let mut send_task = tokio::spawn(async move {
            while let Ok(message) = rx.recv().await {
                let mut connected_socket = socket2.lock().await;

                match message.target {
                    BroadcastTarget::All => {
                        if connected_socket.is_join_topic(&message.topic) {
                            connected_socket.send(message.into()).await.unwrap();
                        }
                    }
                    BroadcastTarget::Other => {
                        let socket = Arc::clone(&connected_socket.socket);
                        let socket = socket.read().await;
                        if connected_socket.is_join_topic(&message.topic)
                            && socket.id.as_deref() != message.from.as_deref()
                        {
                            connected_socket.send(message.into()).await.unwrap();
                        }
                    }
                }
            }
        });

        tokio::select! {
            _ = (&mut send_task) => recv_task.abort(),
            _ = (&mut recv_task) => send_task.abort(),
        };
    }

    async fn call(message: Message, socket: Arc<Mutex<ConnectedSocket<T>>>) {
        let topic = message.topic().await;
        let mut connected_socket = socket.lock().await;

        if let (Some(channel), Some(socket)) = (
            connected_socket.get_channel(&topic),
            connected_socket.get_channel_socket(&topic),
        ) {
            for (status, value) in channel.call(message, socket).await {
                connected_socket.reply(status, value).await.unwrap();
            }
        } else {
            connected_socket
                .reply_error(WsError::UnmatchedTopic.to_string().as_str())
                .await
                .unwrap();
        }
    }

    pub fn broadcast(topic: &str, event: &str, payload: Value) -> anyhow::Result<()> {
        let message = BroadcastMessage {
            topic: topic.into(),
            event: event.into(),
            payload: payload.into(),
            from: None,
            target: BroadcastTarget::All,
        };

        utils::broadcast(message)
    }

    pub fn broadcast_from(
        from: &str,
        topic: &str,
        event: &str,
        payload: Value,
    ) -> anyhow::Result<()> {
        let message = BroadcastMessage {
            topic: topic.into(),
            event: event.into(),
            payload: payload.into(),
            from: Some(from.into()),
            target: BroadcastTarget::Other,
        };

        utils::broadcast(message)
    }
}

impl<T> From<WebSocket<T>> for Router
where
    T: Default + 'static + Send + Sync,
{
    fn from(socket: WebSocket<T>) -> Self {
        Router::new()
            .route(
                &format!("{}/websocket", socket.path),
                get(WebSocket::<T>::connect),
            )
            .layer(Extension(Arc::new(socket)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    pub struct UserSocket;

    #[derive(Debug, Default)]
    pub struct RoomSocket;

    #[test]
    fn test_socket() {
        let user_socket = WebSocket::<UserSocket>::new().path("/socket");
        let room_socket = WebSocket::<RoomSocket>::new().path("/room");

        assert_eq!(user_socket.path.0, "/socket");
        assert_eq!(room_socket.path.0, "/room");
    }

    #[test]
    fn test_get_channel() {
        let user_channel = Channel::new();
        let user_socket = WebSocket::<UserSocket>::new().channel("user:*", user_channel);

        let user_channel1 = user_socket.get_channel(&"user:1".into());
        assert!(user_channel1.is_some());

        let user_channel2 = user_socket.get_channel(&"room:1".into());
        assert!(user_channel2.is_none());
    }
}
