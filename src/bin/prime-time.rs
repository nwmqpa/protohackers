use clap::Parser;
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

#[derive(serde::Serialize)]
struct Response {
    pub method: String,
    pub prime: bool,
}

#[derive(serde::Serialize)]
struct MalformedResponse {
    pub error: String,
}

impl MalformedResponse {
    pub fn as_bytes(&self) -> serde_json::Result<Vec<u8>> {
        let vec = serde_json::to_vec(self)?;

        Ok(format!("{:?}\n", vec).as_bytes().to_vec())
    }
}

impl Response {
    pub fn as_bytes(&self) -> serde_json::Result<Vec<u8>> {
        let vec = serde_json::to_vec(self)?;

        Ok(format!("{:?}\n", vec).as_bytes().to_vec())
    }

}

fn primality_test(number: isize) -> bool {
    if number < 2 {
        return false;
    }

    for i in 2..number {
        if number % i == 0 {
            return false;
        }
    }

    true
}

async fn decode_request(socket: &mut TcpStream, data: &[u8]) -> anyhow::Result<Request> {
    let request = serde_json::from_slice(data);

    match request {
        Ok(request) => Ok(request),
        Err(why) => {
            let malformed_response = MalformedResponse {
                error: format!("{:?}", why),
            };

            socket.write(&malformed_response.as_bytes()?).await?;

            Err(why.into())
        }
    }
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

        let has_newline = acc_data.iter().enumerate().find(|(_, b)| **b == '\n' as u8);

        if let Some((index, _)) = has_newline {
            let Request { method, number } = decode_request(&mut socket, &acc_data[..=index]).await?;

            if method != "is_prime" {
                let malformed_response = MalformedResponse {
                    error: "Method not found".to_string(),
                };

                socket.write(&malformed_response.as_bytes()?).await?;
                break;
            }

            let response = Response {
                method,
                prime: primality_test(number),
            };


            socket.write(&response.as_bytes()?).await?;

            let (_, right) = acc_data.split_at(index);

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
        let (socket, _) = listener.accept().await?;

        tokio::spawn(async move {
            if let Err(why) = process(socket).await {
                eprintln!("Error: {:?}", why);
            }
        });
    }
}
