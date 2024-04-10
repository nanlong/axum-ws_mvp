use axum::{response::Html, routing::get, Router};
use axum_ws::{Channel, Payload, Response, ShardSocket, WebSocket};
use serde_json::{json, Value};
use tower_http::{services::ServeDir, trace::TraceLayer};

#[derive(Default)]
struct UserSocket;

async fn on_websocket_connect(query: Value, socket: ShardSocket) {
    let mut socket = socket.write().await;
    socket.assign("connected".into(), true.into());
    println!("Connected Query: {:?}", query);
    println!("Socket: {:?}", socket);
}

async fn websocket_id(_socket: ShardSocket) -> String {
    format!("user_socket:{}", 123)
}

async fn on_channel_join(payload: Payload, socket: ShardSocket) -> Response {
    let mut socket = socket.write().await;
    socket.assign("joined".into(), true.into());
    // Maby: 验证用户是否有权限加入channel
    println!("Join Query: {:?}", payload);
    println!("Socket: {:?}", socket);
    Ok(Some(json!({"text": "你好"})))
}

async fn test(payload: Payload, socket: ShardSocket) -> Response {
    let mut socket = socket.write().await;
    socket.assign("key".into(), json!({}));

    // 推送消息给全部用户
    socket
        .broadcast("test3", json!({"text": "你好"}))
        .await
        .unwrap();

    // 推送消息给除了自己以外的用户
    socket
        .broadcast_from("test3", json!({"text": "你不好"}))
        .await
        .unwrap();

    println!("Received a message: {:?}", payload);
    println!("Socket: {:?}", socket);
    Ok(Some(json!({})))
}

async fn test2(payload: Payload, socket: ShardSocket) -> Response {
    println!("Received a message: {:?}", payload);
    println!("Socket: {:?}", socket);

    // 推送消息给全部用户
    WebSocket::<UserSocket>::broadcast("room:1", "test3", json!({"text": "UserSocket broadcast"}))?;

    // 推送消息给除了自己以外的用户
    WebSocket::<UserSocket>::broadcast_from(
        "user_socket:1234",
        "room:1",
        "test3",
        json!({"text": "UserSocket broadcast_from"}),
    )?;

    Ok(Some(payload.into()))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let user_channel = Channel::new()
        .on_join(on_channel_join)
        .on("test", test)
        .on("test2", test2);

    let user_socket = WebSocket::<UserSocket>::new()
        .on_connect(on_websocket_connect)
        .id(websocket_id)
        .channel("room:*", user_channel)
        .path("/socket");

    let app = Router::new()
        .route("/", get(index))
        .nest_service("/assets", ServeDir::new("priv/static/assets"))
        .merge(user_socket)
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    tracing::info!(
        "listening on http://localhost:{}",
        listener.local_addr().unwrap().port()
    );

    axum::serve(listener, app).await.unwrap();
}

async fn index() -> Html<&'static str> {
    Html(std::include_str!("../templates/index.html"))
}
