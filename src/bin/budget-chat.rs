use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, RwLock};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const BUFFER_SIZE: usize = 1024;

#[derive(clap::Parser)]
struct Config {
    #[clap(short, long, default_value = "127.0.0.1:50051")]
    pub addr: String,

    #[clap(long, default_value_t = 6669)]
    pub tokio_console_port: u16,
}

#[derive(Clone, Debug)]
enum Messages {
    UserJoining(String),
    UserMessage(String, String),
    UserDisconnecting(String),
}

struct ValueHolder<T>(RwLock<Option<T>>);

impl<T: Clone + std::fmt::Debug> ValueHolder<T> {
    pub fn new() -> Self {
        Self(RwLock::new(None))
    }

    #[tracing::instrument(skip(self), ret)]
    pub async fn read(&self) -> Option<T> {
        let read_guard = self.0.read().await;

        read_guard.clone()
    }

    #[tracing::instrument(skip(self), ret)]
    pub async fn write(&self, value: T) -> Option<T> {
        let mut write_guard = self.0.write().await;

        let previous_value = write_guard.clone();

        *write_guard = Some(value);

        previous_value
    }
}

#[tracing::instrument(skip(socket, tx, accounts))]
async fn process(
    mut socket: TcpStream,
    addr: String,
    tx: broadcast::Sender<Messages>,
    accounts: Arc<RwLock<Vec<String>>>,
) -> anyhow::Result<()> {
    let name = Arc::new(ValueHolder::new());

    socket
        .write("Welcome to budgetchat! What shall I call you?\n".as_bytes())
        .await?;

    let socket_holder = Arc::new(RwLock::new(socket));

    let mut acc_data = Vec::new();
    let mut buf = [0; BUFFER_SIZE];

    loop {
        let bytes_read = {
            let mut socket = socket_holder.write().await;

            socket.read(&mut buf[..]).await?
        };

        if bytes_read == 0 {
            break;
        }

        tracing::debug!("Received {bytes_read} bytes");

        acc_data.extend_from_slice(&buf[..bytes_read]);

        'inner: loop {
            let newline_index = if let Some((index, _)) =
                acc_data.iter().enumerate().find(|(_, b)| **b == '\n' as u8)
            {
                index
            } else {
                tracing::debug!("No newline found");
                break 'inner;
            };

            let message = String::from_utf8_lossy(&acc_data[..=newline_index]);

            tracing::debug!("Parsed message: {}", message);

            if let Some(name) = name.read().await {
                tx.send(Messages::UserMessage(name, message.trim().to_string()))?;
            } else {
                let local_name = message.trim().to_string();

                name.write(local_name.clone()).await;

                tracing::info!("Connection from {local_name}");

                let other_accounts = { accounts.read().await.clone() };

                tx.send(Messages::UserJoining(local_name.clone()))?;

                let mut rx = tx.subscribe();

                socket_holder
                    .write()
                    .await
                    .write(format!("* The room contains: {}\n", other_accounts.join(", ")).as_bytes())
                    .await?;

                let socket_holder = socket_holder.clone();

                accounts.write().await.push(local_name.clone());

                tokio::task::Builder::new()
                    .name(&format!("Receive Loop: {local_name}"))
                    .spawn(async move {
                        while let Ok(data) = rx.recv().await {
                            match data {
                                Messages::UserJoining(name) => {
                                    if name != local_name {
                                        if let Err(why) = socket_holder
                                            .write()
                                            .await
                                            .write(
                                                format!("* {name} has entered the room\n")
                                                    .as_bytes(),
                                            )
                                            .await
                                        {
                                            tracing::error!("{why:?}");
                                        }
                                    }
                                }
                                Messages::UserMessage(name, message) => {
                                    if name != local_name {
                                        if let Err(why) = socket_holder
                                            .write()
                                            .await
                                            .write(format!("[{name}] {message}\n").as_bytes())
                                            .await
                                        {
                                            tracing::error!("{why:?}");
                                        }
                                    }
                                }
                                Messages::UserDisconnecting(name) => {
                                    if name != local_name {
                                        if let Err(why) = socket_holder
                                            .write()
                                            .await
                                            .write(
                                                format!("* {name} has left the room\n").as_bytes(),
                                            )
                                            .await
                                        {
                                            tracing::error!("{why:?}");
                                        }
                                    }
                                }
                            }
                        }
                    })?;
            }

            let (_, right) = acc_data.split_at(newline_index + 1);

            acc_data = right.to_vec();
        }
    }

    if let Some(name) = name.read().await {
        tx.send(Messages::UserDisconnecting(name))?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    let console_layer = console_subscriber::ConsoleLayer::builder()
        .retention(Duration::from_secs(60))
        .server_addr(([0, 0, 0, 0], config.tokio_console_port))
        .spawn();

    tracing_subscriber::registry()
        .with(console_layer)
        .with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env()))
        .init();

    let listener = TcpListener::bind(&config.addr).await?;

    let (tx, mut rx) = tokio::sync::broadcast::channel::<Messages>(1000);

    let accounts = Arc::new(RwLock::new(Vec::<String>::new()));

    loop {
        tokio::select! {
            socket = listener.accept() => {
                if let Ok((socket, addr)) = socket {
                    let addr = format!("{}:{}", addr.ip(), addr.port());

                    let tx = tx.clone();
                    let accounts = accounts.clone();

                    tokio::task::Builder::new().name(
                        &format!("Processing socket: {}", &addr)
                    ).spawn(async move {
                        if let Err(why) = process(socket, addr, tx, accounts).await {
                            tracing::error!("Error: {:?}", why);
                        }
                    })?;
                }
            },
            msg = rx.recv() => {
                tracing::debug!("Message transmitted: {msg:?}");
            },
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    Ok(())
}
