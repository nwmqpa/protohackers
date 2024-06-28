use std::collections::HashMap;
use std::time::Duration;

use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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

#[tracing::instrument(skip(socket))]
async fn process(mut socket: TcpStream, addr: String) -> anyhow::Result<()> {
    let mut acc_data = Vec::new();
    let mut buf = [0; BUFFER_SIZE];

    let mut account = HashMap::<i32, i32>::new();

    loop {
        let bytes_read = socket.read(&mut buf[..]).await?;

        if bytes_read == 0 {
            break;
        }

        acc_data.extend_from_slice(&buf[..bytes_read]);

        tracing::debug!("Received {bytes_read} bytes");

        let mut chunks = acc_data.chunks_exact(9);

        
        for data in &mut chunks {
            tracing::debug!("Chunk: {data:?}");

            match data[0] as char {
                'I' => {
                    let timestamp = i32::from_be_bytes(data[1..=4].try_into()?);
                    let value = i32::from_be_bytes(data[5..=8].try_into()?);

                    tracing::debug!("I: {timestamp}, {value}");

                    account.insert(timestamp, value);
                }
                'Q' => {
                    let mintime = i32::from_be_bytes(data[1..=4].try_into()?);
                    let maxtime = i32::from_be_bytes(data[5..=8].try_into()?);

                    tracing::debug!("Q: {mintime}, {maxtime}");

                    let keys = account
                        .keys()
                        .filter(|k| (mintime <= **k) && (**k <= maxtime))
                        .collect::<Vec<_>>();

                    let values = keys
                        .into_iter()
                        .filter_map(|k| account.get(k))
                        .collect::<Vec<&i32>>();

                    let values_len = values.len() as i32;

                    let mean = if values_len == 0 {
                        0
                    } else {
                        let values_sum = values.into_iter().sum::<i32>();

                        println!("values_sum: {values_sum}, values_len: {values_len}");

                        values_sum / values_len
                    };

                    socket.write(&mean.to_be_bytes()).await?;
                }
                _ => {}
            }
        }

        acc_data = chunks.remainder().to_vec();
    }

    socket.write(&acc_data).await?;

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

    loop {
        tokio::select! {
            socket = listener.accept() => {
                if let Ok((socket, addr)) = socket {
                    let addr = format!("{}:{}", addr.ip(), addr.port());

                    tokio::task::Builder::new().name(
                        &format!("Processing socket: {}", &addr)
                    ).spawn(async move {
                        if let Err(why) = process(socket, addr).await {
                            tracing::error!("Error: {:?}", why);
                        }
                    })?;
                }
            },
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    Ok(())
}
