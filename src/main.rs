use axum::{
    extract::{rejection::JsonRejection, FromRequest, MatchedPath, Request, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use http_body_util::Empty;
use hyper::StatusCode;
use hyper_util::rt::TokioIo;
use oas3::Spec;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::Error as IOError;
use tokio::{net::TcpStream, stream};
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() {
    // this method needs to be inside main() method
    env::set_var("RUST_BACKTRACE", "1");

    let spec =
        oas3::from_path("./resources/OpenAPISpecExamples/pet_store.json".to_string()).unwrap();

    //Need to read the openapi spec and save the results into useable objects

    let app = Router::new()
        .route("/", get(handler_segments))
        .layer(
            TraceLayer::new_for_http()
                // Create our own span for the request and include the matched path. The matched
                // path is useful for figuring out which handler the request was routed to.
                .make_span_with(|req: &Request| {
                    let method = req.method();
                    let uri = req.uri();

                    // axum automatically adds this extension.
                    let matched_path = req
                        .extensions()
                        .get::<MatchedPath>()
                        .map(|matched_path| matched_path.as_str());

                    tracing::debug_span!("request", %method, %uri, matched_path)
                })
                // By default `TraceLayer` will log 5xx responses but we're doing our specific
                // logging of errors so disable that
                .on_failure(()),
        )
        .with_state(spec);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .unwrap();
    tracing::debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

async fn handler_segments(State(spec): State<Spec>, req: Request) -> Result<Response, AppError> {
    let path = req.uri().path();

    match spec.paths.contains_key(path) {
        true => println!("true-{}", &path),
        false => println!("false-{}", &path),
    }

    let url = "http://localhost:3000".parse::<hyper::Uri>().unwrap();

    let host = url.host().expect("uri has no host");
    let port = url.port_u16().unwrap_or(80);

    let address = format!("{}:{}", host, port);

    println!("Address: {}", address);

    //let stream = TcpStream::connect(address).await;

    // Use an adapter to access something implementing `tokio::io` traits as if they implement
    // `hyper::rt` IO traits.

    let mut stream = TcpStream::connect(address).await?;

    let io = TokioIo::new(stream);

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
    let mut res = match sender.send_request(req).await {
        Ok(value) => println!("Result:"),
        Err(error) => println!("Error: {}", error),
    };

    println!("Response status: {:?}", res);

    Ok(res.into_response())
}

// The kinds of errors we can hit in our application.
enum AppError {
    // The request body contained invalid JSON
    JsonRejection(JsonRejection),
    HyperRejection(hyper::Error),
    IORejection(IOError),
}

// Tell axum how `AppError` should be converted into a response.
//
// This is also a convenient place to log errors.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // How we want errors responses to be serialized
        #[derive(Serialize)]
        struct ErrorResponse {
            message: String,
        }

        let (status, message) = match self {
            AppError::JsonRejection(rejection) => {
                // This error is caused by bad user input so don't log it
                (rejection.status(), rejection.body_text())
            }
            AppError::HyperRejection(err) => {
                tracing::error!(%err, "error from hyper");

                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong".to_owned(),
                )
            }
            AppError::IORejection(err) => {
                tracing::error!(%err, "error from hyper");

                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong".to_owned(),
                )
            }
        };

        (status, AppJson(ErrorResponse { message })).into_response()
    }
}

impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        Self::JsonRejection(rejection)
    }
}

impl From<hyper::Error> for AppError {
    fn from(error: hyper::Error) -> Self {
        Self::HyperRejection(error)
    }
}

impl From<IOError> for AppError {
    fn from(error: IOError) -> Self {
        Self::IORejection(error)
    }
}

// Create our own JSON extractor by wrapping `axum::Json`. This makes it easy to override the
// rejection and provide our own which formats errors to match our application.
//
// `axum::Json` responds with plain text if the input is invalid

struct AppJson<T>(T);

impl<T> IntoResponse for AppJson<T>
where
    axum::Json<T>: IntoResponse,
{
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}
