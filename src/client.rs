
use std::{collections::HashMap, sync::{Arc, Mutex}};

use futures::{StreamExt, TryStreamExt, channel::mpsc::{self, UnboundedReceiver, UnboundedSender}, future, pin_mut};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use once_cell::sync::OnceCell;

static SEND_CHANNEL: OnceCell<UnboundedSender<Message>> = OnceCell::new();

pub struct Client {
    url: url::Url,
    auth_token: String,
    queues: Arc<Mutex<HashMap<String, UnboundedSender<String>>>>
}

impl Client {
    pub fn new(url: String, auth_token: String) -> Client {
        return Client{
            url: url::Url::parse(&url).unwrap(),
            auth_token,
            queues: Arc::new(Mutex::new(HashMap::new()))
        };
    }

    pub async fn close(&mut self) {
        SEND_CHANNEL.get().unwrap().unbounded_send(Message::Close(None)).unwrap();
    }

    pub async fn connection(&mut self) {
        let (ws_stream, _) = connect_async(self.url.clone()).await.expect("Failed to connect");
        let (outgoing, incoming) = ws_stream.split();

        let (tx, rx) = mpsc::unbounded();
        SEND_CHANNEL.set(tx.clone()).unwrap();

        tx.unbounded_send(Message::Text(serde_json::to_string(&Request{
            cmd_id: 0,
            args: Value::Array(vec![Value::String(self.auth_token.clone())])
        }).unwrap())).unwrap();

        let queues_clone = self.queues.clone();
        tokio::spawn(async move {
            let broadcast_incoming = incoming.try_for_each(|msg| {
                //println!("{:?}", msg);
                if msg.is_text() {
                    let json = serde_json::from_str::<Response>(&msg.to_string());
                    if !json.is_ok() {
                        println!("{:?}", json.err().unwrap());
    
                        tx.unbounded_send(Message::Close(None)).unwrap();
                        return future::ok(());
                    }
    
                    let res = json.unwrap();
                    match res.cmd_id {
                        0 => {
                            if res.data.is_boolean() && res.data.as_bool().unwrap() {
                                //_
                            }
                        },
                        2 => {
                            if res.data.is_object() {
                                let data = res.data.as_object().unwrap();
                                if data.contains_key("token") && data.contains_key("value") {
                                    for (token, queue) in queues_clone.lock().unwrap().iter() {
                                        if data.get("token").unwrap().as_str().unwrap().eq(token) {
                                            queue.unbounded_send(data.get("value").unwrap().as_str().unwrap().to_string()).unwrap();
                                        }
                                    }
                                }
                            }
                        },
                        _ => {}
                    }
                }
                future::ok(())
            });
        
            let receive_from_others = rx.map(Ok).forward(outgoing);
            pin_mut!(broadcast_incoming, receive_from_others);
            future::select(broadcast_incoming, receive_from_others).await;
        });
    }

    pub async fn subscribe(&mut self, token: String, keys: Vec<String>) -> UnboundedReceiver<String> {
        let (tx, rx) = mpsc::unbounded();
        self.queues.clone().lock().unwrap().insert(token.clone(), tx);

        let mut args = Map::new();
        args.insert("token".to_string(), Value::String(token));
        args.insert("keys".to_string(), Value::Array(keys.iter().map(|x| Value::String(x.to_string())).collect()));

        let req = Request{
            cmd_id: 2,
            args: Value::Object(args)
        };

        SEND_CHANNEL.get().unwrap().unbounded_send(Message::Text(serde_json::to_string(&req).unwrap())).unwrap();

        return rx;
    }

    pub fn send(&mut self, token: String, key: String, data: String) {
        let mut args = Map::new();
        args.insert("token".to_string(), Value::String(token));
        args.insert("key".to_string(), Value::String(key));
        args.insert("value".to_string(), Value::String(data));
        let req = Request{
            cmd_id: 1,
            args: Value::Object(args)
        };

        SEND_CHANNEL.get().unwrap().unbounded_send(Message::Text(serde_json::to_string(&req).unwrap())).unwrap();
    }
}

#[derive(Debug, Clone, Serialize)]
struct Request {
    #[serde(rename = "cmdId")]
    cmd_id: i32,
    args: Value
}


#[derive(Debug, Clone, Deserialize)]
struct Response {
    #[serde(rename = "cmdId")]
    cmd_id: i32,
    #[serde(default = "fnull")]
    data: Value,
    #[serde(default = "fnull")]
    message: Value
}

fn fnull() -> Value {
    return Value::Null;
}