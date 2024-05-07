// For testing, use: $ websocat ws://127.0.0.1:3000/websocket

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::stream::{SplitSink, SplitStream};
use futures::{sink::SinkExt, stream::StreamExt};
use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

// Our shared state
struct AppState {
    // We require unique usernames. This tracks which usernames have been taken.
    user_set: Mutex<HashSet<String>>,
    // Channel used to send messages to all connected clients.
    tx: broadcast::Sender<String>,
}

#[tokio::main]
async fn main() {
    // Set up application state for use with with_state().
    let user_set = Mutex::new(HashSet::new());
    let (tx, _rx) = broadcast::channel(100);

    let app_state = Arc::new(AppState { user_set, tx });

    let app = Router::new()
        .route("/websocket", get(websocket_handler))
        .with_state(app_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    axum::serve(
        TcpListener::bind(&addr).await.unwrap(),
        app.into_make_service(),
    )
    .await
    .unwrap();
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| websocket(socket, state))
}

// This function deals with a single websocket connection, i.e., a single
// connected client / user, for which we will spawn two independent tasks (for
// receiving / sending chat messages).
async fn websocket(stream: WebSocket, state: Arc<AppState>) {
    println!("Connected");

    // By splitting, we can send and receive at the same time.
    let (mut sender, mut receiver) = stream.split();

    // Username gets set in the receive loop, if it's valid.
    let username = create_username(state.clone(), &mut sender, &mut receiver).await;

    // We subscribe *before* sending the "joined" message, so that we will also
    // display it to our client.
    let mut rx = state.tx.subscribe();

    // Now send the "joined" message to all subscribers.
    let msg = format!("{} joined.", username);
    let _ = state.tx.send(msg);

    // Spawn the first task that will receive broadcast messages and send text
    // messages over the websocket to our client.
    let name = username.clone();
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            println!("{name} receives a message...");
            // In any websocket error, break loop.
            if sender.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    // Clone things we want to pass (move) to the receiving task.
    let tx = state.tx.clone();

    // Spawn a task that takes messages from the websocket, prepends the username,
    // and sends them to all broadcast subscribers.
    let name = username.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = receiver.next().await {
            // Add username before message.
            println!("{name} sends a message...");
            let _ = tx.send(format!("{}: {}", name, text));
        }
    });

    // If any one of the tasks run to completion, we abort the other.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }

    // Send "user left" message (similar to "joined" above).
    let msg = format!("{} left.", username);
    let _ = state.tx.send(msg);

    // Remove username from map so new clients can take it again.
    state.user_set.lock().unwrap().remove(&username);

    println!("Disconnected");
}

async fn create_username(
    state: Arc<AppState>,
    sender: &mut SplitSink<WebSocket, Message>,
    receiver: &mut SplitStream<WebSocket>,
) -> String {
    // Username gets set in the receive loop, if it's valid.
    let mut username = String::new();

    // Loop until a text message is found.
    while let Some(Ok(message)) = receiver.next().await {
        if let Message::Text(name) = message {
            // If username that is sent by client is not taken, fill username string.
            check_username(&state, &mut username, &name);

            // If not empty we want to quit the loop else we want to quit function.
            if !username.is_empty() {
                break;
            } else {
                // Only send our client that username is taken.
                let _ = sender
                    .send(Message::Text(String::from("Username already taken.")))
                    .await;
            }
        }
    }

    username
}

fn check_username(state: &AppState, string: &mut String, name: &str) {
    let mut user_set = state.user_set.lock().unwrap();

    let name = name.trim();
    if !user_set.contains(name) {
        user_set.insert(name.to_owned());

        string.push_str(name);
    }
}
