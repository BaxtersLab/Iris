use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use iris_core::pipeline::prometheus_text;
use std::convert::Infallible;
use std::net::TcpListener;

#[tokio::test]
async fn metrics_endpoint_responds() {
    // ensure metric exists by incrementing the counter once
    // call into iris_core to increment the plain counter
    iris_core::rebase_count();

    // create a listener on an ephemeral port
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();

    let svc = make_service_fn(|_conn| async move {
        Ok::<_, Infallible>(service_fn(|req: Request<Body>| async move {
            if req.uri().path() == "/metrics" {
                let body = prometheus_text();
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/plain; version=0.0.4")
                        .body(Body::from(body))
                        .unwrap(),
                )
            } else {
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(404)
                        .body(Body::from("not found"))
                        .unwrap(),
                )
            }
        }))
    });

    let server = Server::from_tcp(listener).unwrap().serve(svc);
    let handle = tokio::spawn(async move {
        let _ = server.await;
    });

    // fetch /metrics via async reqwest client
    let url = format!("http://{}/metrics", addr);
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await.expect("request");
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("body");
    assert!(body.contains("iris_encoder_rebase_total"));

    // shutdown server by dropping handle
    handle.abort();
}
