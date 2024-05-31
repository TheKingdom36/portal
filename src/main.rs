use axum::{
    extract::Request,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use http_body_util::Empty;
use hyper::StatusCode;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let mut routes: HashMap<String, OpenAPIObject> = HashMap::new();

    //Need to read the openapi spec and save the results into useable objects

    let app = Router::new().route("/*segements", get(handler_segments));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

struct OpenAPIObject {
    to: String,
}

async fn handler_segments(mut req: Request) -> Result<Response, StatusCode> {
    let url = "https://8.8.8.8".parse::<hyper::Uri>().unwrap();

    let host = url.host().expect("uri has no host");
    let port = url.port_u16().unwrap_or(80);

    let address = format!("{}:{}", host, port);

    println!("Address: {}", address);

    let stream = TcpStream::connect(address).await;

    // Use an adapter to access something implementing `tokio::io` traits as if they implement
    // `hyper::rt` IO traits.
    let io = TokioIo::new(stream.unwrap());

    let handshake_result = hyper::client::conn::http1::handshake(io).await;

    let (mut sender, conn) = handshake_result.unwrap();

    // Spawn a task to poll the connection, driving the HTTP state
    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            println!("Connection failed: {:?}", err);
        }
    });

    // Create an HTTP request with an empty body and a HOST header
    let req = Request::builder()
        .uri(url)
        .method("GET")
        .body(Empty::<Bytes>::new())
        .unwrap();

    println!("Req status: {:?}", req);

    // Await the response...
    let mut res = sender.send_request(req).await.unwrap();

    println!("Response status: {:?}", res);

    Ok(res.into_response())
}
