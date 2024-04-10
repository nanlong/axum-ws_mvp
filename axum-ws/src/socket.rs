use serde_json::{Map, Value};

use crate::{utils, BroadcastMessage, BroadcastTarget, MessageExt, ShardSocket, Topic};

#[derive(Default, Clone, Debug)]
pub struct Socket {
    pub id: Option<String>,          // 用于标记socket的唯一id
    pub joined: bool,                // 当channel成功加入后，joined为true
    pub assigns: Map<String, Value>, // 存储用户自定义的数据
    pub join_ref: Option<String>,    // 用于标记join请求的唯一id
    pub message_ref: Option<String>, // 用于标记消息的唯一id
    pub topic: Option<Topic>,        // 不同的topic对应不同的channel
}

impl Socket {
    pub fn new() -> Self {
        Self {
            id: Some(nanoid::nanoid!()),
            ..Default::default()
        }
    }

    pub fn assign(&mut self, key: String, value: Value) {
        self.assigns.insert(key, value);
    }

    pub async fn broadcast(&self, event: &str, payload: Value) -> anyhow::Result<()> {
        self.do_broadcast(event, payload, BroadcastTarget::All)
    }

    pub async fn broadcast_from(&self, event: &str, payload: Value) -> anyhow::Result<()> {
        self.do_broadcast(event, payload, BroadcastTarget::Other)
    }

    fn do_broadcast(
        &self,
        event: &str,
        payload: Value,
        target: BroadcastTarget,
    ) -> anyhow::Result<()> {
        let message = BroadcastMessage {
            topic: self.topic.clone().unwrap(),
            event: event.into(),
            payload: payload.into(),
            from: self.id.clone(),
            target,
        };

        utils::broadcast(message)
    }
}

impl MessageExt for ShardSocket {
    async fn join_ref(&self) -> Option<String> {
        self.read().await.join_ref.clone()
    }

    async fn message_ref(&self) -> Option<String> {
        self.read().await.message_ref.clone()
    }

    async fn topic(&self) -> Topic {
        self.read().await.topic.clone().unwrap()
    }
}
