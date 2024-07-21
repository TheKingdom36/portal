use axum::{
    extract::{rejection::JsonRejection, MatchedPath, Request, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use http_body_util::Empty;
use hyper::{Method, StatusCode};
use hyper_util::rt::TokioIo;
use oas3::Spec;
use serde::Serialize;
use std::{
    borrow::{Borrow, BorrowMut},
    env,
    io::Error as IOError,
};
use tokio::net::TcpStream;
use tower_http::trace::TraceLayer;
use tracing::{debug, info, instrument};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // this method needs to be inside main() method
    env::set_var("RUST_BACKTRACE", "full");

    let args: Vec<String> = env::args().collect();
    println!("{:?}", args);

    let spec =
        oas3::from_path("./resources/OpenAPISpecExamples/pet_store.json".to_string()).unwrap();

    //Need to read the openapi spec and save the results into useable objects

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .init();

    let app = Router::new()
        .route("/*key", get(handler_segments))
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

                    tracing::error_span!("request", %method, %uri, matched_path)
                })
                // By default `TraceLayer` will log 5xx responses but we're doing our specific
                // logging of errors so disable that
                .on_failure(()),
        )
        .with_state(spec);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .unwrap();
    debug!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await.unwrap();
}

#[instrument(skip_all)]
async fn handler_segments(State(spec): State<Spec>, req: Request) -> Result<Response, AppError> {
    let path = req.uri().path();
    let paths = spec.paths.unwrap_or_default();

    let path_item_option = paths.get(path);

    if path_item_option.is_none() {
        return Err(AppError::OpenApiRejection(OpenapiError {
            message: "No valid path was found".to_string(),
        }));
    }

    let path_item = path_item_option.unwrap();

    let url = path_item.extensions.get("forward-server");
    if url.is_none() {
        return Err(AppError::OpenApiRejection(OpenapiError {
            message: "No valid URL Provided".to_string(),
        }));
    }

    let mut url_string = "http://".to_owned();
    let u = url.unwrap().as_str().unwrap_or_default();
    url_string.push_str(u);

    let url_hyper = match url_string.parse::<hyper::Uri>() {
        Ok(value) => value,
        Err(err) => {
            println!("some error occured blah blah: {err}");
            return Err(AppError::OpenApiRejection(OpenapiError {
                message: err.to_string(),
            }));
        }
    };

    let host = url_hyper.host().expect("uri has no host");
    let port = url_hyper.port_u16().unwrap_or(80);

    let address = format!("{}:{}", host, port);

    tracing::debug!("Createing connection to: {:?}", address);

    tracing::debug!("Completed connection");

    // Open a TCP connection to the remote host
    let stream = TcpStream::connect(address).await?;

    // Use an adapter to access something implementing `tokio::io` traits as if they implement
    // `hyper::rt` IO traits.
    let io = TokioIo::new(stream);

    // Create the Hyper client
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    // Spawn a task to poll the connection, driving the HTTP state
    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            println!("Connection failed: {:?}", err);
        }
    });

    // Create an HTTP request with an empty body and a HOST header
    let hyper_req = Request::builder()
        .uri(req.uri())
        .method(req.method())
        .body(req.into_body())
        .unwrap();

    tracing::debug!("Req status: {:?}", hyper_req);

    // Await the response...
    let mut res = sender.send_request(hyper_req).await.unwrap();

    println!("Response status: {:?}", res);

    Ok(res.into_response())
}

#[derive(Debug)]
struct OpenapiError {
    message: String,
}

// The kinds of errors we can hit in our application.
#[derive(Debug)]
enum AppError {
    // The request body contained invalid JSON
    JsonRejection(JsonRejection),
    HyperRejection(hyper::Error),
    IORejection(IOError),
    OpenApiRejection(OpenapiError),
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
                    "There was an issue communicating with external services please try again later".to_owned(),
                )
            }
            AppError::IORejection(err) => {
                tracing::error!(%err, "error from IO");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong io ".to_owned(),
                )
            }
            AppError::OpenApiRejection(err) => {
                tracing::error!("{:?}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong OpenAPI ".to_owned(),
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

struct AppJson<T>(T);

impl<T> IntoResponse for AppJson<T>
where
    axum::Json<T>: IntoResponse,
{
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}
