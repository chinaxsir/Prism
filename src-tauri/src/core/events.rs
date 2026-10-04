//! 内核事件流订阅
//!
//! Rust 作为 WebSocket 客户端连接内核的 Clash API 数据流，
//! 将消息原样转为 Tauri 事件推送给前端：
//!
//!   ws://{api}/traffic     → "traffic://tick"     { up, down }
//!   ws://{api}/connections → "connections://tick" { downloadTotal, uploadTotal, connections: [...] }
//!   ws://{api}/memory      → "memory://tick"      { inuse, ... }
//!
//! spawn_all 返回所有订阅任务的 JoinHandle，内核停止时必须 abort，
//! 否则重连循环永不退出，stop/start 后订阅者翻倍导致事件重复。

use std::time::Duration;

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter};
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

/// 启动全部内核事件流订阅，返回任务句柄（调用方在停止内核时 abort）
pub fn spawn_all(
    api_addr: &str,
    api_secret: &str,
    app: AppHandle,
) -> Vec<JoinHandle<()>> {
    vec![
        spawn_stream(api_addr, api_secret, "/traffic", "traffic://tick", app.clone()),
        spawn_stream(api_addr, api_secret, "/connections", "connections://tick", app.clone()),
        spawn_stream(api_addr, api_secret, "/memory", "memory://tick", app),
    ]
}

fn spawn_stream(
    api_addr: &str,
    api_secret: &str,
    path: &'static str,
    event_name: &'static str,
    app: AppHandle,
) -> JoinHandle<()> {
    let url = format!("ws://{api_addr}{path}");
    let api_secret = api_secret.to_string();

    tokio::spawn(async move {
        // 断线自动重连
        loop {
            // 构造带 Bearer 鉴权头的 WS 请求（clash_api 配置了 secret 时必须携带）
            let connect = async {
                let mut request = url.as_str().into_client_request()?;
                if !api_secret.is_empty()
                    && let Ok(value) = HeaderValue::from_str(&format!("Bearer {api_secret}"))
                {
                    request.headers_mut().insert(AUTHORIZATION, value);
                }
                connect_async(request).await
            };

            match connect.await {
                Ok((ws, _)) => {
                    tracing::info!("subscribed kernel stream: {}", path);
                    let (_, mut read) = ws.split();

                    while let Some(Ok(msg)) = read.next().await {
                        if let Message::Text(text) = msg
                            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
                            && app.emit(event_name, value).is_err()
                        {
                            tracing::debug!("emit {} failed (no listeners)", event_name);
                        }
                    }
                    tracing::warn!("kernel stream closed: {}", path);
                }
                Err(e) => {
                    tracing::debug!("subscribe {} failed: {}", path, e);
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
}
