use clap::Parser;
use is_prime::is_prime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const BUFFER_SIZE: usize = 1024;

#[derive(clap::Parser)]
struct Config {
    #[clap(short, long, default_value = "127.0.0.1:50051")]
    pub addr: String,
}

#[derive(serde::Deserialize)]
struct Request {
    pub method: String,
    pub number: isize,
}

#[derive(serde::Serialize, Debug)]
struct Response {
    pub method: String,
    pub prime: bool,
}

#[derive(serde::Serialize, Debug)]
struct MalformedResponse {
    pub error: String,
}

impl MalformedResponse {
    pub fn as_bytes(&self) -> serde_json::Result<Vec<u8>> {
        let value = serde_json::to_string(self)?;

        Ok(format!("{value}\n").as_bytes().to_vec())
    }
}

impl Response {
    pub fn as_bytes(&self) -> serde_json::Result<Vec<u8>> {
        let value = serde_json::to_string(self)?;

        Ok(format!("{value}\n").as_bytes().to_vec())
    }
}

async fn decode_request(socket: &mut TcpStream, data: &[u8]) -> anyhow::Result<Request> {
    let request = serde_json::from_slice(data);

    match request {
        Ok(request) => Ok(request),
        Err(why) => {
            let malformed_response = MalformedResponse {
                error: format!("{:?}", why),
            };

            send_malformed_response(socket, malformed_response).await?;

            Err(why.into())
        }
    }
}

async fn send_malformed_response(
    socket: &mut TcpStream,
    response: MalformedResponse,
) -> anyhow::Result<()> {
    eprintln!("Sending: {:?}", response);

    socket.write(&response.as_bytes()?).await?;

    Ok(())
}

async fn process(mut socket: TcpStream) -> anyhow::Result<()> {
    let mut acc_data = Vec::new();
    let mut buf = [0; BUFFER_SIZE];

    loop {
        let bytes_read = socket.read(&mut buf[..]).await?;

        if bytes_read == 0 {
            break;
        }

        acc_data.extend_from_slice(&buf[..bytes_read]);

        let has_newline = acc_data.iter().enumerate().find(|(_, b)| **b == '\n' as u8);

        if let Some((index, _)) = has_newline {
            let Request { method, number } =
                decode_request(&mut socket, &acc_data[..=index]).await?;

            if method != "isPrime" {
                let malformed_response = MalformedResponse {
                    error: format!("Method not found: {method:?}"),
                };

                send_malformed_response(&mut socket, malformed_response).await?;
                break;
            }

            println!("Analyzing number: {number}");

            let response = Response {
                method,
                prime: is_prime::is_prime(&number.to_string()),
            };

            socket.write(&response.as_bytes()?).await?;

            let (_, right) = acc_data.split_at(index + 1);

            acc_data = right.to_vec();
        }
    }

    socket.write(&acc_data).await?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    let listener = TcpListener::bind(&config.addr).await?;

    loop {
        tokio::select! {
            socket = listener.accept() => {
                if let Ok((socket, _)) = socket {
                    if let Err(why) = process(socket).await {
                        eprintln!("Error: {:?}", why);
                    }
                }
            },
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    Ok(())
}
