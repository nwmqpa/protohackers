use std::io::{Error, ErrorKind};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, RwLock};
use tracing::Value;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

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

    pub async fn read(&self) -> Option<T> {
        let read_guard = self.0.read().await;

        read_guard.clone()
    }

    pub async fn write(&self, value: T) -> Option<T> {
        let mut write_guard = self.0.write().await;

        let previous_value = write_guard.clone();

        *write_guard = Some(value);

        previous_value
    }
}

struct Connection {
    name_holder: Arc<ValueHolder<String>>,
    buffer: RwLock<Vec<u8>>,
    socket: RwLock<TcpStream>,
}

impl Connection {
    const BUFFER_SIZE: usize = 1024;

    pub fn new(socket: TcpStream, name_holder: Arc<ValueHolder<String>>) -> Self {
        Self {
            buffer: RwLock::new(Vec::new()),
            socket: RwLock::new(socket),
            name_holder
        }
    }

    #[tracing::instrument(skip(self))]
    async fn get_next_newline_index(&self) -> Option<usize> {
        let buffer = self.buffer.read().await;

        buffer.iter().position(|b| *b == '\n' as u8)
    }

    #[tracing::instrument(skip(self), err)]
    async fn read_packet_to_buffer(&self) -> std::io::Result<bool> {
        let mut buf = [0; Self::BUFFER_SIZE];

        let bytes_read = self.socket.write().await.read(&mut buf).await?;

        if bytes_read == 0 {
            return Ok(false);
        }

        tracing::debug!("Received {bytes_read} bytes");

        self.buffer
            .write()
            .await
            .extend_from_slice(&buf[..bytes_read]);

        Ok(true)
    }

    #[tracing::instrument(skip(self), ret)]
    pub async fn read_next_chunk(&self) -> std::io::Result<Option<String>> {
        while self.get_next_newline_index().await.is_none() {
            if !self.read_packet_to_buffer().await? {
                return Ok(None);
            }
        }

        if let Some(index) = self.get_next_newline_index().await {
            let mut buffer = self.buffer.write().await;

            let chunk = String::from_utf8_lossy(&buffer[..=index]).to_string();

            let (_, right) = buffer.split_at(index + 1);

            *buffer = right.to_vec();

            Ok(Some(chunk))
        } else {
            Err(Error::new(ErrorKind::NotFound, "Impossible to get newline"))
        }
    }

    #[tracing::instrument(skip(self, data), err)]
    pub async fn send<S: AsRef<str>>(&self, data: S) -> std::io::Result<usize> {
        let name = self.name_holder.read().await.unwrap_or("Unknown".to_string());

        tracing::debug!("Sending {:?} to {name}", data.as_ref());

        self.socket
            .write()
            .await
            .write(data.as_ref().as_bytes())
            .await
    }
}

#[tracing::instrument(skip(socket, tx, accounts))]
async fn process(
    socket: TcpStream,
    addr: String,
    tx: broadcast::Sender<Messages>,
    accounts: Arc<RwLock<Vec<String>>>,
) -> anyhow::Result<()> {
    let name = Arc::new(ValueHolder::new());

    let connection = Arc::new(Connection::new(socket));

    let new_name = name.clone();
    let mut rx = tx.subscribe();

    let inner_connection = connection.clone();

    tokio::task::Builder::new()
        .name(&format!("Receive Loop: {addr}"))
        .spawn(async move {
            while let Ok(data) = rx.recv().await {
                if let Some(local_name) = new_name.read().await {
                    match data {
                        Messages::UserJoining(name) => {
                            if name != local_name {
                                if let Err(why) = inner_connection
                                    .send(format!("* {name} has entered the room\n"))
                                    .await
                                {
                                    tracing::error!("{why:?}");
                                }
                            }
                        }
                        Messages::UserMessage(name, message) => {
                            if name != local_name {
                                if let Err(why) =
                                    inner_connection.send(format!("[{name}] {message}\n")).await
                                {
                                    tracing::error!("{why:?}");
                                }
                            }
                        }
                        Messages::UserDisconnecting(name) => {
                            if name != local_name {
                                if let Err(why) = inner_connection
                                    .send(format!("* {name} has left the room\n"))
                                    .await
                                {
                                    tracing::error!("{why:?}");
                                }
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        })?;

    connection
        .send("Welcome to budgetchat! What shall I call you?\n")
        .await?;

    while let Some(message) = connection.read_next_chunk().await? {
        if let Some(name) = name.read().await {
            tx.send(Messages::UserMessage(name, message.trim().to_string()))?;
        } else {
            let local_name = message.trim().to_string();

            name.write(local_name.clone()).await;

            tracing::info!("Connection from {local_name}");

            let other_accounts = { accounts.read().await.clone() };

            tx.send(Messages::UserJoining(local_name.clone()))?;

            connection
                .send(format!(
                    "* The room contains: {}\n",
                    other_accounts.join(", ")
                ))
                .await?;

            accounts.write().await.push(local_name.clone());
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
