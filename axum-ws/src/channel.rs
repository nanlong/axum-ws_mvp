use std::collections::HashMap;
use std::sync::Arc;

use futures::Future;
use serde_json::{json, Value};

use crate::{Callback, Event, Message, Payload, Response, ShardSocket, Status, WsError};

pub struct Channel {
    callbacks: HashMap<Event, Vec<Box<dyn Callback + Send + Sync>>>,
}

impl Channel {
    pub fn new() -> Self {
        Self {
            callbacks: Default::default(),
        }
    }

    pub fn on<F, R>(mut self, event: &str, callback: F) -> Self
    where
        F: Fn(Payload, ShardSocket) -> R + Callback + Send + Sync + 'static,
        R: Future<Output = Response>,
    {
        self.callbacks
            .entry(event.into())
            .or_insert_with(Vec::new)
            .push(Box::new(callback));

        self
    }

    pub fn on_join<F, R>(self, callback: F) -> Self
    where
        F: Fn(Payload, ShardSocket) -> R + Callback + Send + Sync + 'static,
        R: Future<Output = Response>,
    {
        self.on("phx_join", callback)
    }

    pub(crate) async fn join(&self, payload: Payload, socket: ShardSocket) -> Response {
        self.run(&Event::Join, payload, socket).await
    }

    pub(crate) async fn run(
        &self,
        event: &Event,
        payload: Payload,
        socket: ShardSocket,
    ) -> Response {
        if let Some(callbacks) = self.callbacks.get(&event) {
            if callbacks.len() == 0 {
                return Ok(None);
            }

            callbacks[0].call(payload, socket).await
        } else {
            Ok(None)
        }
    }

    pub async fn call(&self, message: Message, socket: ShardSocket) -> Vec<(Status, Value)> {
        let mut resp = Vec::new();

        if let Some(callbacks) = self.callbacks.get(&message.event) {
            for callback in callbacks {
                let result = callback
                    .call(message.payload.clone(), Arc::clone(&socket))
                    .await;

                match result {
                    Ok(Some(response)) => resp.push(("ok".into(), response)),
                    Err(error) => resp.push(("error".into(), error.to_string().into())),
                    _ => {}
                };
            }
        } else {
            resp.push((
                "error".into(),
                json!({ "reason": WsError::UnmatchedEvent.to_string() }),
            ))
        }

        resp
    }
}
