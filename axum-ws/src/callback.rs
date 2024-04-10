use futures::{future::BoxFuture, Future};
use serde_json::Value;

use crate::{Payload, Response, ShardSocket};

pub trait Callback {
    fn call(&self, payload: Payload, socket: ShardSocket) -> BoxFuture<'static, Response>;
}

impl<F, R> Callback for F
where
    F: Fn(Payload, ShardSocket) -> R,
    R: Future<Output = Response> + 'static + Send + Sync,
{
    fn call(&self, payload: Payload, socket: ShardSocket) -> BoxFuture<'static, Response> {
        Box::pin(self(payload, socket))
    }
}

pub trait WebSocketConnectCallback {
    fn call(&self, params: Value, socket: ShardSocket) -> BoxFuture<'static, ()>;
}

impl<F, R> WebSocketConnectCallback for F
where
    F: Fn(Value, ShardSocket) -> R,
    R: Future<Output = ()> + 'static + Send + Sync,
{
    fn call(&self, params: Value, socket: ShardSocket) -> BoxFuture<'static, ()> {
        Box::pin(self(params, socket))
    }
}

pub trait WebSocketIdCallback {
    fn call(&self, socket: ShardSocket) -> BoxFuture<'static, String>;
}

impl<F, R> WebSocketIdCallback for F
where
    F: Fn(ShardSocket) -> R,
    R: Future<Output = String> + 'static + Send + Sync,
{
    fn call(&self, socket: ShardSocket) -> BoxFuture<'static, String> {
        Box::pin(self(socket))
    }
}
