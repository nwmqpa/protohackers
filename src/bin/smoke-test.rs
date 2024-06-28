use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const BUFFER_SIZE: usize = 1024;

#[derive(clap::Parser)]
struct Config {
    #[clap(short, long, default_value = "127.0.0.1:50051")]
    pub addr: String,
}

async fn process(mut socket: TcpStream) -> anyhow::Result<()> {
    let mut acc_data = Vec::new();
    let mut buf = [0; BUFFER_SIZE];

    loop {
        let bytes_read = socket.read(&mut buf[..]).await?;

        if bytes_read == 0 {
            break;
        }

        println!("Received {:x?}", &buf[..bytes_read]);

        acc_data.extend_from_slice(&buf[..bytes_read]);
    }

    socket.write(&acc_data).await?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    let listener = TcpListener::bind(&config.addr).await?;

    loop {
        let (socket, _) = listener.accept().await?;

        tokio::spawn(async move {
            if let Err(why) = process(socket).await {
                eprintln!("Error: {:?}", why);
            }
        });
    }
}
