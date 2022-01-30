
use std::{collections::HashMap, sync::{Arc, Mutex}};

use futures::{StreamExt, TryStreamExt, channel::mpsc::{self, UnboundedReceiver, UnboundedSender}, future, pin_mut};
use protobuf::Message;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WSMessage};
use once_cell::sync::OnceCell;

use crate::protocol;

pub struct Client {
    url: url::Url,
    auth_token: String,
    queues: Arc<Mutex<HashMap<String, UnboundedSender<Vec<u8>>>>>,
    send_channel: OnceCell<UnboundedSender<WSMessage>>
}

impl Client {
    pub fn new(url: String, auth_token: String) -> Client {
        return Client{
            url: url::Url::parse(&url).unwrap(),
            auth_token,
            queues: Arc::new(Mutex::new(HashMap::new())),
            send_channel: OnceCell::new(),
        };
    }

    pub async fn close(&mut self) {
        self.send_channel.get().unwrap().unbounded_send(WSMessage::Close(None)).unwrap();
    }

    pub async fn connection(&mut self) -> bool {
        let (ws_stream, _) = connect_async(self.url.clone()).await.expect("Failed to connect");
        let (outgoing, incoming) = ws_stream.split();

        let (tx, rx) = mpsc::unbounded();
        self.send_channel.set(tx.clone()).unwrap();

        // login
        let mut req = protocol::Request::new();
        req.set_command(protocol::Commands::LOGIN);

        let mut login_req = protocol::LoginRequest::new();
        login_req.set_token(self.auth_token.clone());

        req.set_data(login_req.write_to_bytes().unwrap());

        tx.unbounded_send(WSMessage::Binary(req.write_to_bytes().unwrap())).unwrap();

        let (wait_login_tx, mut wait_login_rx) = mpsc::unbounded();

        let queues_clone = self.queues.clone();
        tokio::spawn(async move {
            let broadcast_incoming = incoming.try_for_each(|msg| {
                match msg {
                    WSMessage::Binary(msg) => {
                        let res = protocol::Response::parse_from_bytes(&msg);
                        if res.is_err() {
                            println!("{:?}", res.err().unwrap());

                            tx.unbounded_send(WSMessage::Close(None)).unwrap();
                            return future::ok(());
                        }

                        let res = res.unwrap();
                        match res.get_command() {
                            protocol::Commands::HEARTBEAT => {
                                // 心跳包
                            },
                            protocol::Commands::LOGIN => {
                                let login_res = protocol::LoginResponse::parse_from_bytes(res.get_data()).unwrap();

                                if login_res.get_status() {
                                    wait_login_tx.unbounded_send(true).unwrap();
                                    return future::ok(());
                                }
                                wait_login_tx.unbounded_send(false).unwrap();
                            },
                            protocol::Commands::SUBSCRIBE => {
                                let subscribe_res = protocol::SubscribeResponse::parse_from_bytes(res.get_data()).unwrap();
                                println!("{:?}", subscribe_res);
                            },
                            protocol::Commands::SUBSCRIBE_CALLBACK => {
                                let callback = protocol::SubscribeCallback::parse_from_bytes(res.get_data()).unwrap();

                                for (token, queue) in queues_clone.lock().unwrap().iter() {
                                    if callback.get_token().eq(token) {
                                        queue.unbounded_send(callback.get_data().to_vec()).unwrap();
                                    }
                                }
                            },
                            _ => {}
                        }
                    }
                    _ => {},
                }
                future::ok(())
            });

            let receive_from_others = rx.map(Ok).forward(outgoing);
            pin_mut!(broadcast_incoming, receive_from_others);
            future::select(broadcast_incoming, receive_from_others).await;
        });

        let result = wait_login_rx.next().await.unwrap();
        wait_login_rx.close();
        result
    }

    pub async fn subscribe(&mut self, token: String, keys: Vec<String>) -> UnboundedReceiver<Vec<u8>> {
        let (tx, rx) = mpsc::unbounded();
        self.queues.clone().lock().unwrap().insert(token.clone(), tx);

        let mut req = protocol::Request::new();
        req.set_command(protocol::Commands::SUBSCRIBE);

        let mut sub_req = protocol::SubscribeRequest::new();
        sub_req.set_token(token);
        sub_req.mut_keys().extend(keys);

        req.set_data(sub_req.write_to_bytes().unwrap());

        self.send_channel.get().unwrap().unbounded_send(WSMessage::Binary(req.write_to_bytes().unwrap())).unwrap();

        return rx;
    }

    pub fn send(&mut self, token: String, key: String, data: Vec<u8>) {
        let mut req = protocol::Request::new();
        req.set_command(protocol::Commands::SEND_MESSAGE);

        let mut send_req = protocol::SendRequest::new();
        send_req.set_token(token);
        send_req.set_key(key);
        send_req.set_data(data);

        req.set_data(send_req.write_to_bytes().unwrap());

        self.send_channel.get().unwrap().unbounded_send(WSMessage::Binary(req.write_to_bytes().unwrap())).unwrap();
    }
}