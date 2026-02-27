mod api;
mod nodes;
mod types;

use std::sync::Arc;

#[tokio::main]
async fn main() {
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    let (engine_handle, engine_thread) = reactive_graph::start_engine();

    let state = Arc::new(api::AppState::new(engine_handle.clone(), redis_url));
    let app = api::router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await.unwrap();
    println!("FX Pricing server listening on {bind_addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    println!("Shutting down engine...");
    engine_handle.shutdown();
    engine_thread.join().unwrap();
    println!("Done.");
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.unwrap();
    println!("\nReceived Ctrl-C, shutting down...");
}
